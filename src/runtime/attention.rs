//! Attention Letters: detection, batched rate-limited delivery, and feedback
//! (spec user story 26; ticket 11).
//!
//! The Supervisor surface detects five attention conditions over runtime ledger
//! events, evaluated against the injectable runtime clock
//! ([`super::ledger::now_secs`]):
//!
//! 1. **stalled permission** — a `permission_requested { permission, assignment?,
//!    detail? }` event with no later `permission_resolved { permission }` for the
//!    same permission, older than [`STALLED_PERMISSION_AFTER_SECS`] (5 minutes);
//! 2. **unread handback** — an assignment still sitting in `HandedBack` (handed
//!    back, no Lead action since) older than [`UNREAD_HANDBACK_AFTER_SECS`]
//!    (10 minutes);
//! 3. **repeated failures** — a trailing run of consecutive red `gate_completed`
//!    events for one assignment reaching the configurable failure threshold;
//! 4. **`NEEDS-HUMAN:` / `INCIDENT:` lines** — a `lead_message { text }` event
//!    whose text carries a line starting with either marker;
//! 5. **context rotation** — the Lead's latest `context_usage { seat, tokens }`
//!    event at or past the rotation mark ([`ROTATION_MARK_TOKENS`], 150k tokens).
//!
//! Delivery is batched and rate-limited: every letter carries a stable
//! `att_<kind>_<discriminator>` key, renders as `[hotpot attention <key>] …`,
//! and `room_status` records each delivered key (`attention_delivered`) so the
//! same key is not re-emitted within the rate window. `attention_feedback`
//! events (`mute` | `snooze` | `downrank`, by key) are honored on every
//! subsequent batch.

use super::{Ledger, RuntimeState};
use serde_json::{Value, json};

/// A permission older than this (5 minutes) is stalled.
pub const STALLED_PERMISSION_AFTER_SECS: u64 = 300;
/// A peer handback still unread after this (10 minutes) is an attention letter.
pub const UNREAD_HANDBACK_AFTER_SECS: u64 = 600;
/// The Lead context rotation mark (the 30% report mark of the spec's UI/UX
/// ergonomics: rotation ticks apply from 150k tokens).
pub const ROTATION_MARK_TOKENS: u64 = 150_000;
/// Default trailing red-gate run that raises a repeated-failure letter.
pub const DEFAULT_FAILURE_THRESHOLD: usize = 3;
/// Default window during which a delivered letter key is not re-emitted.
pub const DEFAULT_RATE_WINDOW_SECS: u64 = 600;
/// Environment override for the repeated-failure threshold.
pub const FAILURE_THRESHOLD_ENV: &str = "HOTPOT_ATTENTION_FAILURE_THRESHOLD";
/// Environment override (unix seconds) for the per-key rate window.
pub const RATE_WINDOW_ENV: &str = "HOTPOT_ATTENTION_RATE_WINDOW";

/// Tunables of the attention detection, with environment overrides for tests
/// and operators (`HOTPOT_ATTENTION_FAILURE_THRESHOLD`,
/// `HOTPOT_ATTENTION_RATE_WINDOW`).
#[derive(Debug, Clone, Copy)]
pub struct AttentionConfig {
    pub stalled_permission_after_secs: u64,
    pub unread_handback_after_secs: u64,
    pub rotation_mark_tokens: u64,
    pub failure_threshold: usize,
    pub rate_window_secs: u64,
}

impl Default for AttentionConfig {
    fn default() -> Self {
        Self {
            stalled_permission_after_secs: STALLED_PERMISSION_AFTER_SECS,
            unread_handback_after_secs: UNREAD_HANDBACK_AFTER_SECS,
            rotation_mark_tokens: ROTATION_MARK_TOKENS,
            failure_threshold: DEFAULT_FAILURE_THRESHOLD,
            rate_window_secs: DEFAULT_RATE_WINDOW_SECS,
        }
    }
}

impl AttentionConfig {
    pub fn from_env() -> Self {
        let mut config = Self::default();
        if let Some(raw) = std::env::var(FAILURE_THRESHOLD_ENV).ok()
            && let Ok(threshold) = raw.trim().parse::<usize>()
            && threshold > 0
        {
            config.failure_threshold = threshold;
        }
        if let Some(raw) = std::env::var(RATE_WINDOW_ENV).ok()
            && let Ok(window) = raw.trim().parse::<u64>()
        {
            config.rate_window_secs = window;
        }
        config
    }
}

/// One detected Attention Letter. `key` is the stable identity feedback and the
/// rate limiter address; `text` renders with the canonical
/// `[hotpot attention <key>]` prefix.
#[derive(Debug, Clone)]
pub struct Letter {
    pub key: String,
    pub kind: &'static str,
    pub text: String,
    /// Additional evidence fields merged into the letter's JSON.
    pub detail: Value,
}

impl Letter {
    /// The letter as surfaced in `room_status` / `runtime_findings`.
    pub fn to_json(&self) -> Value {
        let mut letter = json!({
            "key": self.key,
            "kind": self.kind,
            "text": self.text,
        });
        if let (Some(base), Some(extra)) = (letter.as_object_mut(), self.detail.as_object()) {
            for (field, value) in extra {
                base.insert(field.clone(), value.clone());
            }
        }
        letter
    }
}

/// Renders the canonical letter text: `[hotpot attention <key>] <summary>`.
fn letter_text(key: &str, summary: &str) -> String {
    format!("[hotpot attention {key}] {summary}")
}

/// Detects every currently-triggered Attention Letter from the ledger events.
/// Detection is pure: it evaluates the room as of `now` and never mutates the
/// ledger (delivery, suppression, and feedback live in [`deliver_letters`]).
pub fn detect_letters(
    events: &[Value],
    state: &RuntimeState,
    now: u64,
    config: &AttentionConfig,
) -> Vec<Letter> {
    let mut letters = Vec::new();
    letters.extend(stalled_permissions(events, now, config));
    letters.extend(unread_handbacks(events, state, now, config));
    letters.extend(lead_markers(events));
    letters.extend(lead_rotation(events, config));
    letters.extend(repeated_failures(events, config));
    letters
}

/// Letters for assignments whose trailing run of consecutive red
/// `gate_completed` verdicts reaches the failure threshold (`uncertain` is not
/// a failure; any non-red verdict resets the run). One letter per assignment
/// regardless of how far past the threshold the run reaches.
fn repeated_failures(events: &[Value], config: &AttentionConfig) -> Vec<Letter> {
    let mut assignments: Vec<&str> = Vec::new();
    for event in events
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("gate_completed"))
    {
        if let Some(assignment) = event.get("assignment").and_then(Value::as_str)
            && !assignments.contains(&assignment)
        {
            assignments.push(assignment);
        }
    }
    assignments
        .into_iter()
        .filter(|assignment| trailing_consecutive_reds(events, assignment) >= config.failure_threshold)
        .map(|assignment| {
            let trailing = trailing_consecutive_reds(events, assignment);
            let key = format!("att_repeated_failure_{assignment}");
            let summary = format!(
                "gate failed {trailing} times in a row on {assignment} (threshold {})",
                config.failure_threshold
            );
            Letter {
                text: letter_text(&key, &summary),
                key,
                kind: "repeated-failure",
                detail: json!({
                    "assignment": assignment,
                    "consecutiveReds": trailing,
                    "threshold": config.failure_threshold,
                }),
            }
        })
        .collect()
}

/// The trailing run of consecutive red gate verdicts for `assignment`.
fn trailing_consecutive_reds(events: &[Value], assignment: &str) -> usize {
    events
        .iter()
        .filter(|e| {
            e.get("type").and_then(Value::as_str) == Some("gate_completed")
                && e.get("assignment").and_then(Value::as_str) == Some(assignment)
        })
        .rev()
        .map(|e| e.get("verdict").and_then(Value::as_str) == Some("red"))
        .take_while(|red| *red)
        .count()
}

/// The rotation letter for the Lead's latest `context_usage` event once it sits
/// at or past the rotation mark. The letter clears itself when a later usage
/// event reports the Lead back under the mark.
fn lead_rotation(events: &[Value], config: &AttentionConfig) -> Vec<Letter> {
    let latest = events
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("context_usage"))
        .filter(|e| e.get("seat").and_then(Value::as_str) == Some("lead"))
        .filter_map(|e| {
            let tokens = e.get("tokens").and_then(Value::as_u64)?;
            Some((e, tokens))
        })
        .next_back();
    let Some((_event, tokens)) = latest.filter(|(_, tokens)| *tokens >= config.rotation_mark_tokens)
    else {
        return Vec::new();
    };
    let key = "att_lead_context_rotation".to_string();
    let summary = format!(
        "Lead context at {tokens} tokens crossed the rotation mark ({}); consider a quiet-point replacement",
        config.rotation_mark_tokens
    );
    vec![Letter {
        text: letter_text(&key, &summary),
        key,
        kind: "lead-context-rotation",
        detail: json!({
            "tokens": tokens,
            "rotationMarkTokens": config.rotation_mark_tokens,
            "reportMark": "30%",
        }),
    }]
}

/// The attention markers a Lead message line may start with.
const NEEDS_HUMAN_MARKER: &str = "NEEDS-HUMAN:";
const INCIDENT_MARKER: &str = "INCIDENT:";

/// The first line of `text` starting with `marker` (trimmed), if any.
fn marker_line<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    text.lines().map(str::trim).find(|l| l.starts_with(marker))
}

/// Letters for `lead_message { text }` events whose text carries a line
/// starting with `NEEDS-HUMAN:` or `INCIDENT:` — one letter per marker kind
/// the message raises; ordinary Lead messages raise nothing.
fn lead_markers(events: &[Value]) -> Vec<Letter> {
    let mut letters = Vec::new();
    for event in events
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("lead_message"))
    {
        let Some(text) = event.get("text").and_then(Value::as_str) else {
            continue;
        };
        let seq = event.get("seq").and_then(Value::as_u64).unwrap_or(0);
        for (marker, kind, slug) in [
            (NEEDS_HUMAN_MARKER, "needs-human", "needs_human"),
            (INCIDENT_MARKER, "incident", "incident"),
        ] {
            let Some(line) = marker_line(text, marker) else {
                continue;
            };
            let key = format!("att_{slug}_seq{seq}");
            letters.push(Letter {
                text: letter_text(&key, &format!("Lead raised {marker} {line}")),
                key,
                kind,
                detail: json!({"marker": marker, "line": line, "seq": seq}),
            });
        }
    }
    letters
}

/// Lead events that count as having read a handback (the gate and every
/// terminal disposition are Lead actions on the handed-back work).
const HANDBACK_READ_EVENTS: &[&str] = &[
    "gate_started",
    "assignment_accepted",
    "assignment_rework_requested",
    "assignment_rejected",
    "assignment_abandoned",
];

/// Letters for assignments whose latest handback no Lead action has followed
/// (still unread) past the unread mark. Gate activity counts as reading: a
/// Lead that ran the gate has plainly seen the handback, even though the state
/// machine returns the assignment to `HandedBack` while the gate completes.
fn unread_handbacks(events: &[Value], state: &RuntimeState, now: u64, config: &AttentionConfig) -> Vec<Letter> {
    state
        .assignments()
        .iter()
        .filter_map(|a| {
            let handback = events
                .iter()
                .filter(|e| {
                    e.get("type").and_then(Value::as_str) == Some("assignment_handed_back")
                        && e.get("assignment").and_then(Value::as_str) == Some(a.id.as_str())
                })
                .next_back()?;
            let handback_seq = handback.get("seq").and_then(Value::as_u64)?;
            let read = events.iter().any(|e| {
                HANDBACK_READ_EVENTS.contains(&e.get("type").and_then(Value::as_str).unwrap_or(""))
                    && e.get("assignment").and_then(Value::as_str) == Some(a.id.as_str())
                    && e.get("seq").and_then(Value::as_u64).is_some_and(|seq| seq > handback_seq)
            });
            (!read).then_some((a, handback))
        })
        .filter_map(|(a, handback)| {
            let ts = handback.get("ts").and_then(Value::as_u64)?;
            Some((a, handback, now.saturating_sub(ts)))
        })
        .filter(|(.., age)| *age > config.unread_handback_after_secs)
        .map(|(a, handback, age)| {
            let key = format!("att_unread_handback_{}", a.id);
            let summary = format!(
                "peer handback on {} awaits Lead review for {age}s (candidate `{}`)",
                a.id,
                a.candidate.as_deref().unwrap_or("none")
            );
            Letter {
                text: letter_text(&key, &summary),
                key,
                kind: "unread-handback",
                detail: json!({
                    "assignment": a.id,
                    "correlation": a.correlation,
                    "candidate": a.candidate,
                    "ageSeconds": age,
                    "handedBackAt": handback.get("ts").cloned().unwrap_or(Value::Null),
                }),
            }
        })
        .collect()
}

/// Letters for `permission_requested` events with no matching
/// `permission_resolved` event, older than the stall mark.
fn stalled_permissions(events: &[Value], now: u64, config: &AttentionConfig) -> Vec<Letter> {
    events
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("permission_requested"))
        .filter(|e| {
            let id = e.get("permission").and_then(Value::as_str).unwrap_or("");
            !events.iter().any(|r| {
                r.get("type").and_then(Value::as_str) == Some("permission_resolved")
                    && r.get("permission").and_then(Value::as_str) == Some(id)
            })
        })
        .filter_map(|e| {
            let ts = e.get("ts").and_then(Value::as_u64)?;
            let age = now.saturating_sub(ts);
            (age > config.stalled_permission_after_secs).then_some((e, age))
        })
        .map(|(e, age)| {
            let permission = e
                .get("permission")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("seq{}", e.get("seq").and_then(Value::as_u64).unwrap_or(0)));
            let assignment = e.get("assignment").and_then(Value::as_str).unwrap_or("");
            let detail = e.get("detail").and_then(Value::as_str).unwrap_or("");
            let key = format!("att_stalled_permission_{permission}");
            let mut summary = format!("permission `{permission}` awaiting a decision for {age}s");
            if !assignment.is_empty() {
                summary.push_str(&format!(" on {assignment}"));
            }
            if !detail.is_empty() {
                summary.push_str(&format!(": {detail}"));
            }
            let mut detail_json = json!({
                "permission": permission,
                "ageSeconds": age,
                "since": e.get("ts").cloned().unwrap_or(Value::Null),
            });
            if !assignment.is_empty() {
                detail_json["assignment"] = json!(assignment);
            }
            Letter {
                text: letter_text(&key, &summary),
                key,
                kind: "stalled-permission",
                detail: detail_json,
            }
        })
        .collect()
}

/// The feedback actions `attention_feedback` accepts, by letter key.
pub const FEEDBACK_ACTIONS: &[&str] = &["mute", "snooze", "downrank"];

/// Collects the currently due letter batch from the ledger as of `now` and —
/// when `record` is set (the `room_status` delivery path) — appends one
/// `attention_delivered { key }` event per delivered letter so the same key is
/// not re-emitted within the rate window. `record: false` serves read-only
/// surfaces (`runtime_findings`) the same batch without consuming it.
///
/// A key is due when its condition currently holds, no feedback suppresses it
/// (see [`feedback_effect`]), and it was not already delivered less than
/// `rate_window_secs` ago. Downranked keys still surface — flagged
/// `downranked: true` and sorted last.
pub fn deliver_letters(
    ledger: &Ledger,
    now: u64,
    config: &AttentionConfig,
    record: bool,
) -> Result<Vec<Value>, String> {
    let events = ledger.read_events();
    let state = RuntimeState::from_events(&events);
    let letters = detect_letters(&events, &state, now, config);

    let mut due: Vec<(bool, &Letter)> = letters // (downranked, letter)
        .iter()
        .filter_map(|letter| match feedback_effect(&events, &letter.key, now, config) {
            FeedbackEffect::Muted | FeedbackEffect::Snoozed { .. } => None,
            FeedbackEffect::Downranked => Some((true, letter)),
            FeedbackEffect::None => Some((false, letter)),
        })
        .filter(|(_, letter)| {
            let last_delivered = events
                .iter()
                .filter(|e| {
                    e.get("type").and_then(Value::as_str) == Some("attention_delivered")
                        && e.get("key").and_then(Value::as_str) == Some(letter.key.as_str())
                })
                .filter_map(|e| e.get("ts").and_then(Value::as_u64))
                .max()
                .unwrap_or(0);
            now >= last_delivered.saturating_add(config.rate_window_secs)
        })
        .collect();
    due.sort_by_key(|(downranked, _)| *downranked);

    if record {
        for (_, letter) in &due {
            ledger
                .append(json!({"type": "attention_delivered", "key": letter.key}))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(due
        .into_iter()
        .map(|(downranked, letter)| {
            let mut json = letter.to_json();
            if downranked {
                json["downranked"] = json!(true);
            }
            json
        })
        .collect())
}

/// What the latest `attention_feedback` event for a key means for the batch.
#[derive(Debug, PartialEq, Eq)]
pub enum FeedbackEffect {
    /// No feedback recorded for the key.
    None,
    /// `mute`: the key is silenced until feedback changes.
    Muted,
    /// `snooze`: the key is suppressed until its snooze elapses.
    Snoozed { until: u64 },
    /// `downrank`: the key still surfaces (sorted last, flagged).
    Downranked,
}

/// Resolves the effect of the most recent `attention_feedback { key, action }`
/// event for `key` as of `now` (latest feedback wins).
pub fn feedback_effect(
    events: &[Value],
    key: &str,
    now: u64,
    config: &AttentionConfig,
) -> FeedbackEffect {
    let Some(feedback) = events
        .iter()
        .filter(|e| {
            e.get("type").and_then(Value::as_str) == Some("attention_feedback")
                && e.get("key").and_then(Value::as_str) == Some(key)
        })
        .next_back()
    else {
        return FeedbackEffect::None;
    };
    match feedback.get("action").and_then(Value::as_str) {
        Some("mute") => FeedbackEffect::Muted,
        Some("snooze") => {
            let for_secs = feedback
                .get("forSeconds")
                .and_then(Value::as_u64)
                .unwrap_or(config.rate_window_secs);
            let until = feedback.get("ts").and_then(Value::as_u64).unwrap_or(0) + for_secs;
            if now < until {
                FeedbackEffect::Snoozed { until }
            } else {
                FeedbackEffect::None
            }
        }
        Some("downrank") => FeedbackEffect::Downranked,
        _ => FeedbackEffect::None,
    }
}
