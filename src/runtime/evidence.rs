//! Runtime evidence capture for peer workspaces (ticket 09).
//!
//! "The Gate ran after the last write" must be checkable before a writable
//! `handoff` is believed. The existing `hotpot hook` PreToolUse seam already
//! observes every tool call a seat makes, so it also records minimal ledger
//! events for the correlated peer's workspace:
//!
//! - `workspace_write` { assignment, correlation, workspace, tool } — an allowed
//!   file-mutating or shell tool call in the workspace.
//! - `workspace_gate`  { assignment, correlation, workspace, gate } — an allowed
//!   shell invocation of the assignment's recorded Gate command (an explicit
//!   gate-run record).
//!
//! Events carry the ledger's strictly increasing `seq`, so "gate after last
//! write" is a deterministic `seq` comparison at `handoff` time. Recording is
//! best-effort: an unbound correlation or a missing state dir records nothing
//! and never changes the hook's verdict.

use super::{Assignment, Ledger, RuntimeState};
use serde_json::{Value, json};
use std::path::Path;

/// Ledger event kind: a write-capable tool call in the peer's workspace.
pub const WRITE_EVENT: &str = "workspace_write";
/// Ledger event kind: the peer invoked the assignment's Gate command.
pub const GATE_EVENT: &str = "workspace_gate";

/// File-mutating tool names whose allowed use is recorded as a peer write.
const WRITE_TOOLS: &[&str] = &[
    "Write",
    "Edit",
    "MultiEdit",
    "NotebookEdit",
    "write_file",
    "edit_file",
];

/// Shell tool names. A shell run of the assignment Gate is a gate-run record;
/// any other shell run in the workspace counts as a (potentially mutating) write.
const SHELL_TOOLS: &[&str] = &["Bash", "bash", "run_command"];

/// Records evidence from one allowed `hotpot hook` PreToolUse evaluation of a
/// correlated peer seat. `payload` is the hook's stdin JSON (`tool_name`/`tool`
/// and `tool_input`/`input`).
pub fn record_hook_event(state_dir: &Path, correlation: &str, payload: &Value, allowed: bool) {
    if !allowed {
        return; // a denied tool call wrote nothing, so it is not evidence.
    }
    let tool = payload
        .get("tool_name")
        .or_else(|| payload.get("tool"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !WRITE_TOOLS.contains(&tool) && !SHELL_TOOLS.contains(&tool) {
        return;
    }
    let Some((ledger, asg)) = resolve_correlation_binding(state_dir, correlation) else {
        return; // best-effort: an unbound seat has no workspace evidence to keep.
    };

    let tool_input = payload
        .get("tool_input")
        .or_else(|| payload.get("input"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let shell_command = if SHELL_TOOLS.contains(&tool) {
        tool_input
            .get("command")
            .or_else(|| tool_input.get("CommandLine"))
            .and_then(Value::as_str)
            .map(str::trim)
    } else {
        None
    };
    let gate = asg.gate.trim();
    let event = if shell_command.is_some_and(|cmd| !gate.is_empty() && cmd.contains(gate)) {
        json!({
            "type": GATE_EVENT,
            "assignment": asg.id,
            "correlation": correlation,
            "workspace": asg.workspace.clone().unwrap_or_default(),
            "gate": asg.gate,
        })
    } else {
        json!({
            "type": WRITE_EVENT,
            "assignment": asg.id,
            "correlation": correlation,
            "workspace": asg.workspace.clone().unwrap_or_default(),
            "tool": tool,
        })
    };
    let _ = ledger.append(event);
}

/// Finds the ledger and assignment the correlation is bound to by scanning the
/// project ledgers under `<state_dir>/runtime/v1/` (the hook is not launched
/// with a project root). The most recently dispatched binding wins.
fn resolve_correlation_binding(state_dir: &Path, correlation: &str) -> Option<(Ledger, Assignment)> {
    let v1 = state_dir.join("runtime").join("v1");
    let entries = std::fs::read_dir(&v1).ok()?;
    let mut best: Option<(u64, Ledger, Assignment)> = None;
    for entry in entries.flatten() {
        let ledger = Ledger::at(entry.path().join("ledger.jsonl"));
        let events = ledger.read_events();
        let Some(seq) = events
            .iter()
            .rev()
            .find(|e| {
                e.get("type").and_then(Value::as_str) == Some("assignment_dispatched")
                    && e.get("correlation").and_then(Value::as_str) == Some(correlation)
            })
            .and_then(|e| e.get("seq").and_then(Value::as_u64))
        else {
            continue;
        };
        if best.as_ref().is_some_and(|(best_seq, ..)| *best_seq >= seq) {
            continue;
        }
        let state = RuntimeState::from_events(&events);
        let assignment_id = events
            .iter()
            .rev()
            .find(|e| {
                e.get("type").and_then(Value::as_str) == Some("assignment_dispatched")
                    && e.get("correlation").and_then(Value::as_str) == Some(correlation)
            })
            .and_then(|e| e.get("assignment").and_then(Value::as_str))
            .unwrap_or_default();
        if let Some(asg) = state.assignment(assignment_id) {
            best = Some((seq, ledger, asg.clone()));
        }
    }
    best.map(|(_, ledger, asg)| (ledger, asg))
}

/// The highest `seq` among `kind` evidence events recorded for `assignment`.
pub fn latest_evidence_seq(events: &[Value], assignment: &str, kind: &str) -> Option<u64> {
    events
        .iter()
        .filter(|e| {
            e.get("type").and_then(Value::as_str) == Some(kind)
                && e.get("assignment").and_then(Value::as_str) == Some(assignment)
        })
        .filter_map(|e| e.get("seq").and_then(Value::as_u64))
        .max()
}
