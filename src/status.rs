//! Live `hotpot status` projection (ticket 12): the runtime ledger plus the live
//! `hc` terminal labels served as the Project snapshot the Herdr mods render.
//!
//! Everything is rebuilt from the event log on every call — seats (from
//! `seat_launched` events, assignment dispatch bindings, and `hc` labels), the
//! assignment master list, the mailbox, segmented context from `context_usage`
//! events, and the currently due Attention Letters (ticket 11, served read-only
//! so a polling panel never consumes a batch). Empty or absent state yields
//! empty arrays, never mocked entries.

use crate::hc::HcTerminalSummary;
use crate::runtime::{Ledger, RuntimeState, attention};
use crate::slp::{
    ContextSegments, HotpotStatusSnapshot, MailboxEntry, ProfileStatus, SeatStatus, TaskStatus,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

/// One seat accumulated from every source naming its correlation.
#[derive(Default)]
struct SeatAccum {
    id: String,
    role: Option<String>,
    agent: Option<String>,
    assignment: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
    terminal: Option<String>,
    terminal_status: Option<String>,
}

/// The role profile defaults in Hotpot Home (`<HOTPOT_HOME>/profiles.json`): the
/// per-role `agent` / `model` / `thinking` defaults the inline pill pickers of
/// tickets 13/14 update atomically; a missing file or role entry defaults to none.
#[derive(Default, Clone)]
struct RoleProfile {
    agent: Option<String>,
    model: Option<String>,
    thinking: Option<String>,
}

fn read_profiles(home: &Path) -> BTreeMap<String, RoleProfile> {
    let Ok(raw) = std::fs::read_to_string(home.join("profiles.json")) else {
        return BTreeMap::new();
    };
    let Ok(Value::Object(roles)) = serde_json::from_str::<Value>(&raw) else {
        return BTreeMap::new();
    };
    roles
        .into_iter()
        .map(|(role, fields)| {
            let profile = RoleProfile {
                agent: fields.get("agent").and_then(Value::as_str).map(str::to_string),
                model: fields.get("model").and_then(Value::as_str).map(str::to_string),
                thinking: fields.get("thinking").and_then(Value::as_str).map(str::to_string),
            };
            (role, profile)
        })
        .collect()
}

/// Builds the live snapshot of the Project at `slug` from the ledger under
/// `home/runtime/v1/<slug>/` plus the `hc` terminal listing.
pub fn snapshot(
    home: &Path,
    workspace: &str,
    slug: &str,
    terminals: &[HcTerminalSummary],
    now: u64,
) -> HotpotStatusSnapshot {
    let runtime_root = home.join("runtime").join("v1");
    let ledger = Ledger::at(runtime_root.join(slug).join("ledger.jsonl"));
    let events = ledger.read_events();
    let state = RuntimeState::from_events(&events);
    let profiles = read_profiles(home);

    // The due Attention Letters, read-only: status polls never record a delivery
    // (that stays the Supervisor's `room_status` call).
    let attention_items = attention::deliver_letters(
        &ledger,
        now,
        &attention::AttentionConfig::from_env(),
        false,
    )
    .unwrap_or_default();

    HotpotStatusSnapshot {
        project: slug.to_string(),
        workspace: workspace.to_string(),
        mode: "slp".to_string(),
        // Hotpot Home is provisioned once `hotpot setup --apply` wrote its
        // `hotpot.json` marker; the panel's Start action auto-provisions on it.
        provisioned: home.join("hotpot.json").exists(),
        has_workspace_protocol: Path::new(workspace)
            .join("WORKSPACE_PROTOCOL.md")
            .exists(),
        profiles: resolved_profiles(&profiles),
        // The watch target handed to the mods' `watchFolder`; absent until any
        // runtime state exists, so a mod keeps its timer fallback instead of
        // subscribing to a directory that is not there.
        runtime_dir: runtime_root
            .is_dir()
            .then(|| runtime_root.display().to_string()),
        generated_at: now,
        seats: project_seats(&events, &state, terminals, slug, &profiles),
        tasks: project_tasks(&events, &state, terminals),
        attention: attention_items,
        mailbox: project_mailbox(&events, &state),
    }
}

/// Each seat role's fully resolved default profile (ticket nui/01): the
/// built-in role defaults with every `<HOTPOT_HOME>/profiles.json` field the
/// role sets layered on top — a partial entry still resolves completely, and a
/// role with no entry resolves to the built-ins alone. Only the three
/// `PROFILE_ROLES` are served; unknown `profiles.json` keys stay private.
fn resolved_profiles(recorded: &BTreeMap<String, RoleProfile>) -> BTreeMap<String, ProfileStatus> {
    crate::profile::BUILTIN_ROLE_PROFILES
        .iter()
        .map(|builtin| {
            let resolve = |recorded: Option<&String>, fallback: &str| {
                recorded.cloned().unwrap_or_else(|| fallback.to_string())
            };
            let entry = recorded.get(builtin.role);
            let status = ProfileStatus {
                agent: resolve(entry.and_then(|p| p.agent.as_ref()), builtin.agent),
                model: resolve(entry.and_then(|p| p.model.as_ref()), builtin.model),
                thinking: resolve(entry.and_then(|p| p.thinking.as_ref()), builtin.thinking),
            };
            (builtin.role.to_string(), status)
        })
        .collect()
}

/// The segmented context of a seat: its latest `context_usage` event, matched by
/// correlation id first, else by role name for the single-seat roles
/// (`supervisor` / `lead`). Peer seats match only by correlation — a `seat:
/// "peer"` event is ambiguous across peers. Zero segments when the seat has no
/// usage events.
pub fn seat_context(events: &[Value], id: &str, role: &str) -> ContextSegments {
    let by_seat = |seat: &str| {
        events
            .iter()
            .rev()
            .find(|e| {
                e.get("type").and_then(Value::as_str) == Some("context_usage")
                    && e.get("seat").and_then(Value::as_str) == Some(seat)
            })
            .map(usage_segments)
    };
    by_seat(id)
        .or_else(|| match role {
            "supervisor" | "lead" => by_seat(role),
            _ => None,
        })
        .unwrap_or_default()
}

/// The segments of one `context_usage` event: its `cacheRead` / `freshInput` /
/// `output` fields with the total carried by `tokens` when present, else their
/// sum (see [`attention::usage_total`]).
pub fn usage_segments(event: &Value) -> ContextSegments {
    let cache_read = event
        .get("cacheRead")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let fresh_input = event
        .get("freshInput")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let output = event.get("output").and_then(Value::as_u64).unwrap_or(0);
    let total = attention::usage_total(event).unwrap_or(0);
    ContextSegments {
        cache_read,
        fresh_input,
        output,
        total,
        rotation_tick: total >= attention::ROTATION_MARK_TOKENS,
    }
}

/// Rank for seat ordering: Supervisor, Lead, then Peers by correlation id.
fn role_rank(role: &str) -> u8 {
    match role {
        "supervisor" => 0,
        "lead" => 1,
        "peer" => 2,
        _ => 3,
    }
}

fn project_seats(
    events: &[Value],
    state: &RuntimeState,
    terminals: &[HcTerminalSummary],
    slug: &str,
    profiles: &BTreeMap<String, RoleProfile>,
) -> Vec<SeatStatus> {
    let mut seats: BTreeMap<String, SeatAccum> = BTreeMap::new();
    fn entry<'a>(seats: &'a mut BTreeMap<String, SeatAccum>, id: &str) -> &'a mut SeatAccum {
        seats
            .entry(id.to_string())
            .or_insert_with(|| SeatAccum {
                id: id.to_string(),
                ..SeatAccum::default()
            })
    }

    for event in events {
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "seat_launched" => {
                if let Some(seat) = event.get("seat").and_then(Value::as_str) {
                    let record = entry(&mut seats, seat);
                    if let Some(role) = event.get("role").and_then(Value::as_str) {
                        record.role = Some(role.to_string());
                    }
                    if let Some(agent) = event.get("agent").and_then(Value::as_str) {
                        record.agent = Some(agent.to_string());
                    }
                    if let Some(assignment) = event.get("assignment").and_then(Value::as_str) {
                        record.assignment = Some(assignment.to_string());
                    }
                }
            }
            "assignment_dispatched" => {
                if let Some(correlation) = event.get("correlation").and_then(Value::as_str) {
                    let record = entry(&mut seats, correlation);
                    record.role = Some("peer".to_string());
                    if let Some(agent) = event.get("agent").and_then(Value::as_str) {
                        record.agent = Some(agent.to_string());
                    }
                    if let Some(model) = event.get("model").and_then(Value::as_str) {
                        record.model = Some(model.to_string());
                    }
                    if let Some(assignment) = event.get("assignment").and_then(Value::as_str) {
                        record.assignment = Some(assignment.to_string());
                        if let Some(thinking) =
                            state.assignment(assignment).and_then(|a| a.thinking.clone())
                        {
                            record.thinking = Some(thinking);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // The live `hc` terminals join by correlation label: they are the only source
    // of terminal addresses and statuses.
    for terminal in terminals
        .iter()
        .filter(|t| t.label(crate::seats::LABEL_V1) == Some("true"))
        .filter(|t| t.label("hotpot.project") == Some(slug))
    {
        let id = terminal
            .label("hotpot.correlation")
            .unwrap_or(&terminal.address);
        let record = entry(&mut seats, id);
        if let Some(role) = terminal.label("hotpot.role") {
            record.role = Some(role.to_string());
        }
        if let Some(agent) = terminal.label("hotpot.agent") {
            record.agent = Some(agent.to_string());
        }
        if let Some(assignment) = terminal.label("hotpot.assignment") {
            record.assignment = Some(assignment.to_string());
        }
        record.terminal = Some(terminal.address.clone());
        record.terminal_status = terminal.status.clone();
    }

    let mut projected: Vec<SeatStatus> = seats
        .into_values()
        .map(|record| {
            let role = record.role.clone().unwrap_or_default();
            let profile = profiles.get(&role);
            let agent = record
                .agent
                .clone()
                .or_else(|| profile.and_then(|p| p.agent.clone()))
                .unwrap_or_default();
            let model = record
                .model
                .clone()
                .or_else(|| profile.and_then(|p| p.model.clone()));
            let thinking = record
                .thinking
                .clone()
                .or_else(|| profile.and_then(|p| p.thinking.clone()));
            let status = match (&record.terminal, &record.terminal_status) {
                (Some(_), Some(status)) => status.clone(),
                (Some(_), None) => "idle".to_string(),
                (None, _) => "offline".to_string(),
            };
            let context = seat_context(events, &record.id, &role);
            let title = match role.as_str() {
                "supervisor" => "Supervisor".to_string(),
                "lead" => "Lead".to_string(),
                "peer" => record
                    .assignment
                    .as_ref()
                    .map(|a| format!("Peer · {a}"))
                    .unwrap_or_else(|| "Peer".to_string()),
                _ => record.id.clone(),
            };
            let activity = if role == "peer" {
                let assignment_state = record
                    .assignment
                    .as_deref()
                    .and_then(|id| state.assignment(id))
                    .map(|a| {
                        a.gate_verdict
                            .as_deref()
                            .map(|verdict| format!("{} · gate {verdict}", a.state))
                            .unwrap_or_else(|| a.state.clone())
                    })
                    .unwrap_or_else(|| "no assignment record".to_string());
                record
                    .assignment
                    .as_deref()
                    .map(|id| format!("{id} · {assignment_state}"))
                    .unwrap_or(assignment_state)
            } else {
                match (&record.terminal, &status) {
                    (Some(address), status) => format!("hc terminal {address} · {status}"),
                    (None, _) => "no live hc terminal".to_string(),
                }
            };
            SeatStatus {
                id: record.id,
                context_tokens: context.total,
                provider: agent.clone(),
                status,
                terminal: record.terminal,
                assignment: record.assignment,
                thinking,
                title,
                activity,
                role,
                agent,
                model,
                context,
            }
        })
        .collect();
    projected.sort_by_key(|seat| (role_rank(&seat.role), seat.id.clone()));
    projected
}

fn project_tasks(
    events: &[Value],
    state: &RuntimeState,
    terminals: &[HcTerminalSummary],
) -> Vec<TaskStatus> {
    state
        .assignments()
        .iter()
        .map(|a| {
            let handbacks = events
                .iter()
                .filter(|e| {
                    e.get("type").and_then(Value::as_str) == Some("assignment_handed_back")
                        && e.get("assignment").and_then(Value::as_str) == Some(a.id.as_str())
                })
                .count() as u32;
            let terminal = a.correlation.as_deref().and_then(|correlation| {
                terminals
                    .iter()
                    .find(|t| t.label("hotpot.correlation") == Some(correlation))
                    .map(|t| t.address.clone())
            });
            let context = a
                .correlation
                .as_deref()
                .map(|correlation| seat_context(events, correlation, "peer"))
                .unwrap_or_default();
            let activity = a
                .gate_verdict
                .as_deref()
                .map(|verdict| format!("{} · gate {verdict}", a.state))
                .unwrap_or_else(|| a.state.clone());
            TaskStatus {
                id: a.id.clone(),
                state: a.state.clone(),
                disposition: a.disposition.clone(),
                verdict: a.gate_verdict.clone(),
                attempt: handbacks.max(1),
                summary: a.outcome.clone(),
                correlation: a.correlation.clone(),
                agent_id: a.correlation.clone(),
                agent: a.agent.clone(),
                provider: a.agent.clone(),
                model: a.model.clone(),
                thinking: a.thinking.clone(),
                workspace: a.workspace.clone(),
                terminal,
                status: a.state.clone(),
                context_tokens: context.total,
                activity,
                context,
            }
        })
        .collect()
}

/// The live mailbox: every routed message event in ledger order — Supervisor
/// directives, Lead markers, and the ask / answer / handback traffic between
/// Lead and its Peers.
fn project_mailbox(events: &[Value], state: &RuntimeState) -> Vec<MailboxEntry> {
    let mut mailbox = Vec::new();
    for event in events {
        let entry = match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "message_to_lead" => MailboxEntry {
                from: event
                    .get("from")
                    .and_then(Value::as_str)
                    .unwrap_or("supervisor")
                    .to_string(),
                // The Supervisor→Lead routing events carry no `to` (the Lead is
                // the addressee by definition); the CLI `--prompt` directives of
                // ticket nui/04 name their target seat explicitly.
                to: event
                    .get("to")
                    .and_then(Value::as_str)
                    .unwrap_or("lead")
                    .to_string(),
                kind: "directive".to_string(),
                summary: event
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            "lead_message" => MailboxEntry {
                from: "lead".to_string(),
                to: "supervisor".to_string(),
                kind: "lead-message".to_string(),
                summary: event
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            "assignment_asked" => MailboxEntry {
                from: event
                    .get("correlation")
                    .or_else(|| event.get("assignment"))
                    .and_then(Value::as_str)
                    .unwrap_or("peer")
                    .to_string(),
                to: "lead".to_string(),
                kind: event
                    .get("signal")
                    .and_then(Value::as_str)
                    .unwrap_or("ask")
                    .to_string(),
                summary: event
                    .get("question")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            "assignment_answered" => MailboxEntry {
                from: "lead".to_string(),
                to: event
                    .get("assignment")
                    .and_then(Value::as_str)
                    .and_then(|id| state.assignment(id))
                    .and_then(|a| a.correlation.clone())
                    .unwrap_or_default(),
                kind: "answer".to_string(),
                summary: event
                    .get("answer")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            "assignment_handed_back" => MailboxEntry {
                from: event
                    .get("correlation")
                    .or_else(|| event.get("assignment"))
                    .and_then(Value::as_str)
                    .unwrap_or("peer")
                    .to_string(),
                to: "lead".to_string(),
                kind: "handback".to_string(),
                summary: event
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            _ => continue,
        };
        mailbox.push(entry);
    }
    mailbox
}
