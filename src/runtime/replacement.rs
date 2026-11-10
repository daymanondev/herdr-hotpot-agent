//! Quiet-point Lead replacement (spec user story 27; ticket 11).
//!
//! A Supervisor replaces a context-rotated Lead only through the structured
//! sequence `lead_replace_prepare` → review/edit → `lead_replace_confirm`
//! (or `lead_replace_cancel`); any path that skips it is out of scope by
//! design. A **quiet point** is a room where no assignment holds live work —
//! every assignment is closed or cleanly handed back (a `HandedBack`
//! assignment awaits the *successor's* integration, so it does not block).
//!
//! Ledger events written by this slice:
//! - `lead_replace_prepared  { replacement, draft, handoffText }`
//! - `lead_archived          { replacement, terminal? }` (also read by the
//!   ticket-06 launcher: an archived Lead terminal is never a reuse candidate)
//! - `lead_replace_confirmed { replacement, handoffText, seedRecord, successorAgent, successorCorrelation }`
//! - `lead_replace_cancelled { replacement }`

use super::{Assignment, Ledger, RuntimeState};
use serde_json::{Value, json};

/// Assignment states that hold live work: a dispatched (or reworking) peer, an
/// unanswered ask, or a gate still running. Everything else — `Draft` (never
/// dispatched), `HandedBack` (cleanly handed back), `Stopped`, `Accepted`,
/// `Rejected`, `Abandoned`, `Closed` — is quiet.
const LIVE_STATES: &[&str] = &["Dispatched", "Asking", "ReworkRequested", "GateRunning"];

/// Why the room is not at a quiet point, naming each blocking assignment.
pub fn quiet_point_blockers(state: &RuntimeState) -> Vec<(String, String)> {
    state
        .assignments()
        .iter()
        .filter(|a| LIVE_STATES.contains(&a.state.as_str()))
        .map(|a| {
            let reason = match a.state.as_str() {
                "Asking" => "unanswered ask".to_string(),
                other if a.write_scope.is_empty() => format!("{other} assignment (read-only work in flight)"),
                other => format!("{other} writable assignment"),
            };
            (a.id.clone(), reason)
        })
        .collect()
}

/// True when the room is at a quiet point (no live work).
pub fn is_quiet_point(state: &RuntimeState) -> bool {
    quiet_point_blockers(state).is_empty()
}

/// The next deterministic replacement id (`lrep_001`, `lrep_002`, …).
pub fn next_replacement_id(events: &[Value]) -> String {
    let count = events
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("lead_replace_prepared"))
        .count();
    format!("lrep_{:03}", count + 1)
}

/// The in-flight replacement (prepared, neither confirmed nor cancelled), if any.
pub fn in_flight_replacement(events: &[Value]) -> Option<String> {
    let mut in_flight: Option<String> = None;
    for event in events {
        match event.get("type").and_then(Value::as_str) {
            Some("lead_replace_prepared") => {
                in_flight = event
                    .get("replacement")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            Some("lead_replace_confirmed") | Some("lead_replace_cancelled") => {
                let done = event.get("replacement").and_then(Value::as_str);
                if done == in_flight.as_deref() {
                    in_flight = None;
                }
            }
            _ => {}
        }
    }
    in_flight
}

/// The prepared handoff draft of `replacement`, when it exists.
pub fn prepared_draft(events: &[Value], replacement: &str) -> Option<Value> {
    events
        .iter()
        .filter(|e| {
            e.get("type").and_then(Value::as_str) == Some("lead_replace_prepared")
                && e.get("replacement").and_then(Value::as_str) == Some(replacement)
        })
        .next_back()
        .and_then(|e| e.get("draft").cloned())
}

/// One row of the draft's assignment board.
fn assignment_row(assignment: &Assignment) -> Value {
    let awaiting = assignment.state == "HandedBack";
    let mut row = json!({
        "id": assignment.id,
        "state": assignment.state,
        "disposition": assignment.disposition,
        "awaitingIntegration": awaiting,
        "outcome": assignment.outcome,
    });
    if let Some(candidate) = &assignment.candidate {
        row["candidate"] = json!(candidate);
    }
    if let Some(verdict) = &assignment.gate_verdict {
        row["gateVerdict"] = json!(verdict);
    }
    row
}

/// Builds the structured, reviewable handoff draft of the current room: the
/// assignment board (flagging work awaiting the successor's integration), the
/// open attention letters, and the rendered `handoffText` the Supervisor
/// reviews and edits before `lead_replace_confirm` seeds the successor with it.
pub fn build_draft(
    slug: &str,
    state: &RuntimeState,
    letters: &[Value],
    now: u64,
) -> Value {
    let assignments: Vec<Value> = state
        .assignments()
        .iter()
        .map(|assignment| assignment_row(assignment))
        .collect();
    let awaiting: Vec<&str> = state
        .assignments()
        .iter()
        .filter(|a| a.state == "HandedBack")
        .map(|a| a.id.as_str())
        .collect();

    let mut text = format!("# Lead handoff — project {slug}\n\n");
    text.push_str("You are the successor Lead. Read `WORKSPACE_PROTOCOL.md` at the project\n");
    text.push_str("root when present, then take the board below as your starting state.\n\n");
    text.push_str("## Assignments\n\n");
    if assignments.is_empty() {
        text.push_str("- (none recorded)\n");
    }
    for assignment in &assignments {
        let id = assignment["id"].as_str().unwrap_or("?");
        let state_name = assignment["state"].as_str().unwrap_or("?");
        text.push_str(&format!("- `{id}` — {state_name}"));
        if assignment["awaitingIntegration"] == json!(true) {
            text.push_str(" — **awaiting your integration** (verify, integrate, accept)");
        }
        text.push('\n');
    }
    text.push_str("\n## Open attention\n\n");
    if letters.is_empty() {
        text.push_str("- all quiet\n");
    }
    for letter in letters {
        text.push_str(&format!(
            "- {}\n",
            letter.get("text").and_then(Value::as_str).unwrap_or("?")
        ));
    }

    json!({
        "project": slug,
        "preparedAt": now,
        "quietPoint": {"verified": true},
        "assignments": assignments,
        "awaitingIntegration": awaiting,
        "openAttention": letters,
        "handoffText": text,
    })
}

/// The successor's seed record path:
/// `<project runtime dir>/lead-replacements/<replacement>-seed.md` — where the
/// reviewed (possibly edited) handoff text the successor Lead is seeded with is
/// durably observable.
pub fn seed_record_path(ledger: &Ledger, replacement: &str) -> std::path::PathBuf {
    ledger
        .project_dir()
        .join("lead-replacements")
        .join(format!("{replacement}-seed.md"))
}
