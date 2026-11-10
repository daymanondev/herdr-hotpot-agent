//! Real-time Paseo-style Session & Thinking Inspector (`hotpot inspect`).
//! Parses on-disk `.jsonl` transcripts from Claude Code, Codex, Pi, and Antigravity
//! into structured `Thinking`, `Tool Call`, `Message`, and notice blocks, and
//! locates a seat's newest transcript from its correlation/workspace records
//! (ticket 12).

use crate::hc::HcTerminalSummary;
use crate::slp::ContextSegments;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptItem {
    pub role: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub status: Option<String>,
    pub duration_ms: Option<u64>,
}

/// The context-window occupancy of one parsed transcript: the latest usage line's
/// three segments (cache read, fresh input, output).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContextUsage {
    pub cache_read: u64,
    pub fresh_input: u64,
    pub output: u64,
}

impl ContextUsage {
    pub fn total(&self) -> u64 {
        self.cache_read + self.fresh_input + self.output
    }

    fn carried(&self) -> bool {
        self.total() > 0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInspection {
    /// The inspected seat's correlation id.
    pub agent_id: String,
    pub role: String,
    /// The transcript file the items were parsed from (`null` when none exists).
    pub transcript: Option<String>,
    pub context: ContextSegments,
    /// Compat alias of `context.total`.
    pub context_tokens: u64,
    pub items: Vec<TranscriptItem>,
}

/// Parses a JSONL stream from Claude Code, Codex, or Pi into structured `TranscriptItem` blocks,
/// preserving full uncollapsed `thinking` / `reasoning` blocks and tool calls.
///
/// Kinds served: `thinking` (body preserved in full), `tool_call` (status `done`
/// once a matching result line answers its call id, else `running`), `message`,
/// and `notice` (task/session status lines such as Codex `task_started` /
/// `task_complete` and Claude summary lines).
pub fn parse_session_jsonl(jsonl: &str) -> (Vec<TranscriptItem>, ContextUsage) {
    let mut items: Vec<TranscriptItem> = Vec::new();
    let mut latest_usage = ContextUsage::default();
    // Tool calls whose results have not been seen yet: item index -> call id.
    let mut pending_calls: Vec<(usize, String)> = Vec::new();
    let mut answered_calls: HashSet<String> = HashSet::new();

    for raw_line in jsonl.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(val) = serde_json::from_str::<Value>(line) else {
            continue;
        };

        // Extract usage / context window occupancy when present
        if let Some(usage) = val
            .get("message")
            .and_then(|m| m.get("usage"))
            .or_else(|| val.get("usage"))
        {
            let input = usage
                .get("input_tokens")
                .or_else(|| usage.get("input"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let cache_read = usage
                .get("cache_read_input_tokens")
                .or_else(|| usage.get("cacheRead"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let output = usage
                .get("output_tokens")
                .or_else(|| usage.get("output"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            let usage = ContextUsage {
                cache_read,
                fresh_input: input,
                output,
            };
            if usage.carried() {
                latest_usage = usage;
            }
        }

        // Claude / Pi content array inside `message.content`
        if let Some(content_arr) = val
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        {
            let msg_role = val
                .get("message")
                .and_then(|m| m.get("role"))
                .and_then(Value::as_str)
                .unwrap_or("assistant");

            for block in content_arr {
                let block_type = block.get("type").and_then(Value::as_str).unwrap_or("");
                match block_type {
                    "thinking" => {
                        let text = block
                            .get("thinking")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim();
                        if !text.is_empty() {
                            items.push(TranscriptItem {
                                role: "assistant".to_string(),
                                kind: "thinking".to_string(),
                                title: "Thinking".to_string(),
                                body: text.to_string(),
                                status: None,
                                duration_ms: block.get("duration_ms").and_then(Value::as_u64),
                            });
                        }
                    }
                    "tool_use" | "toolCall" => {
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool");
                        let input = block
                            .get("input")
                            .or_else(|| block.get("arguments"))
                            .map(Value::to_string)
                            .unwrap_or_default();
                        let index = items.len();
                        items.push(TranscriptItem {
                            role: "tool".to_string(),
                            kind: "tool_call".to_string(),
                            title: name.to_string(),
                            body: input,
                            status: Some("done".to_string()),
                            duration_ms: None,
                        });
                        // A call id makes the status live: answered -> done,
                        // never answered -> running.
                        if let Some(call_id) = block.get("id").and_then(Value::as_str) {
                            pending_calls.push((index, call_id.to_string()));
                        }
                    }
                    "tool_result" | "toolResult" => {
                        // The answer to an earlier tool call: mark it done. The
                        // result payload itself is not transcript content.
                        if let Some(call_id) = block
                            .get("tool_use_id")
                            .or_else(|| block.get("call_id"))
                            .and_then(Value::as_str)
                        {
                            answered_calls.insert(call_id.to_string());
                        }
                    }
                    "text" => {
                        let text = block.get("text").and_then(Value::as_str).unwrap_or("").trim();
                        if !text.is_empty() {
                            items.push(TranscriptItem {
                                role: msg_role.to_string(),
                                kind: "message".to_string(),
                                title: if msg_role == "user" {
                                    "Directive / Prompt".to_string()
                                } else {
                                    "Assistant".to_string()
                                },
                                body: text.to_string(),
                                status: None,
                                duration_ms: None,
                            });
                        }
                    }
                    _ => {}
                }
            }
            continue;
        }

        // Claude session summary lines surface as notices.
        if val.get("type").and_then(Value::as_str) == Some("summary")
            && let Some(summary) = val.get("summary").and_then(Value::as_str)
            && !summary.trim().is_empty()
        {
            items.push(TranscriptItem {
                role: "system".to_string(),
                kind: "notice".to_string(),
                title: "Session summary".to_string(),
                body: summary.trim().to_string(),
                status: None,
                duration_ms: None,
            });
            continue;
        }

        // Codex rollout events: `reasoning`, `function_call`, `message`, and the
        // task-status notices, all inside `payload`.
        if let Some(payload) = val.get("payload") {
            let ptype = payload.get("type").and_then(Value::as_str).unwrap_or("");
            match ptype {
                "reasoning" => {
                    if let Some(summary) = payload.get("summary").and_then(Value::as_array) {
                        let combined: Vec<&str> = summary
                            .iter()
                            .filter_map(|s| s.get("text").and_then(Value::as_str))
                            .collect();
                        if !combined.is_empty() {
                            items.push(TranscriptItem {
                                role: "assistant".to_string(),
                                kind: "thinking".to_string(),
                                title: "Reasoning".to_string(),
                                body: combined.join("\n"),
                                status: None,
                                duration_ms: None,
                            });
                        }
                    }
                }
                "function_call" => {
                    let name = payload
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("function_call");
                    let args = payload
                        .get("arguments")
                        .and_then(Value::as_str)
                        .unwrap_or("{}");
                    let index = items.len();
                    items.push(TranscriptItem {
                        role: "tool".to_string(),
                        kind: "tool_call".to_string(),
                        title: name.to_string(),
                        body: args.to_string(),
                        status: Some("done".to_string()),
                        duration_ms: None,
                    });
                    if let Some(call_id) = payload.get("call_id").and_then(Value::as_str) {
                        pending_calls.push((index, call_id.to_string()));
                    }
                }
                "function_call_output" | "local_shell_call_output" => {
                    if let Some(call_id) = payload.get("call_id").and_then(Value::as_str) {
                        answered_calls.insert(call_id.to_string());
                    }
                }
                "message" => {
                    if let Some(blocks) = payload.get("content").and_then(Value::as_array) {
                        let msg_role = payload
                            .get("role")
                            .and_then(Value::as_str)
                            .unwrap_or("user");
                        for block in blocks {
                            let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                            if !text.trim().is_empty() {
                                items.push(TranscriptItem {
                                    role: msg_role.to_string(),
                                    kind: "message".to_string(),
                                    title: if msg_role == "user" {
                                        "Directive / Prompt".to_string()
                                    } else {
                                        "Assistant".to_string()
                                    },
                                    body: text.trim().to_string(),
                                    status: None,
                                    duration_ms: None,
                                });
                            }
                        }
                    }
                }
                "task_started" | "task_complete" => {
                    let body = payload
                        .get("last_agent_message")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    items.push(TranscriptItem {
                        role: "system".to_string(),
                        kind: "notice".to_string(),
                        title: if ptype == "task_started" {
                            "Task started".to_string()
                        } else {
                            "Task complete".to_string()
                        },
                        body: body.to_string(),
                        status: None,
                        duration_ms: None,
                    });
                }
                _ => {}
            }
        }
    }

    // Resolve call statuses: every result seen marks its call done; the rest are
    // still running (the live turn's last calls may not have answers yet).
    for (index, call_id) in pending_calls {
        if !answered_calls.contains(&call_id)
            && let Some(item) = items.get_mut(index)
        {
            item.status = Some("running".to_string());
        }
    }

    (items, latest_usage)
}

// ---------------------------------------------------------------------------
// Transcript discovery (ticket 12)
// ---------------------------------------------------------------------------

/// Where a seat's correlation / workspace records say its transcripts live:
/// the vendor transcript roots under the Role Home (Claude Code's `projects/`,
/// Codex's `sessions/`), plus the assignment workspace root itself.
const ROLE_HOME_TRANSCRIPT_ROOTS: &[&str] = &["projects", "sessions"];

/// One seat record resolved from its correlation: where to find the seat's Role
/// Home, workspace, and an explicit transcript path when the runtime recorded one.
#[derive(Debug, Clone, Default)]
pub struct SeatRecord {
    pub seat: String,
    pub role: String,
    pub agent: String,
    pub workspace: Option<String>,
    /// An explicit transcript path carried by the record (the escape hatch for
    /// vendors whose transcripts sit outside the canonical Role Home roots).
    pub transcript: Option<String>,
}

/// Resolves a seat's record by its correlation id: the latest `seat_launched`
/// ledger event, else the latest `assignment_dispatched` binding, else the live
/// `hc` terminal carrying the correlation label. `None` when no record names it.
pub fn locate_seat_record(events: &[Value], terminals: &[HcTerminalSummary], seat: &str) -> Option<SeatRecord> {
    let launched = events.iter().rev().find(|e| {
        e.get("type").and_then(Value::as_str) == Some("seat_launched")
            && e.get("seat").and_then(Value::as_str) == Some(seat)
    });
    if let Some(event) = launched {
        return Some(SeatRecord {
            seat: seat.to_string(),
            role: event
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            agent: event
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            workspace: None,
            transcript: event
                .get("transcript")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    let dispatched = events.iter().rev().find(|e| {
        e.get("type").and_then(Value::as_str) == Some("assignment_dispatched")
            && e.get("correlation").and_then(Value::as_str) == Some(seat)
    });
    if let Some(event) = dispatched {
        return Some(SeatRecord {
            seat: seat.to_string(),
            role: "peer".to_string(),
            agent: event
                .get("agent")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            workspace: event
                .get("workspace")
                .and_then(Value::as_str)
                .map(str::to_string),
            transcript: event
                .get("transcript")
                .and_then(Value::as_str)
                .map(str::to_string),
        });
    }

    terminals
        .iter()
        .find(|t| t.label("hotpot.correlation") == Some(seat))
        .map(|t| SeatRecord {
            seat: seat.to_string(),
            role: t.label("hotpot.role").unwrap_or_default().to_string(),
            agent: t.label("hotpot.agent").unwrap_or_default().to_string(),
            workspace: None,
            transcript: None,
        })
}

/// Locates the seat's newest transcript `.jsonl`: the record's explicit
/// `transcript` path when it exists, else the newest `.jsonl` under the Role
/// Home's vendor transcript roots, else the newest `.jsonl` directly in the
/// assignment workspace root. `None` when the seat has no transcript yet.
pub fn locate_transcript(
    role_home: Option<&Path>,
    workspace: Option<&Path>,
    hint: Option<&Path>,
) -> Option<PathBuf> {
    if let Some(hint) = hint.filter(|p| p.is_file()) {
        return Some(hint.to_path_buf());
    }
    if let Some(role_home) = role_home {
        for root in ROLE_HOME_TRANSCRIPT_ROOTS {
            if let Some(newest) = newest_jsonl_recursive(&role_home.join(root)) {
                return Some(newest);
            }
        }
    }
    workspace.and_then(newest_jsonl_shallow)
}

/// The most recently modified `.jsonl` under `dir`, searched recursively
/// (symlinked directories are not followed).
fn newest_jsonl_recursive(dir: &Path) -> Option<PathBuf> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if let Some(candidate) = newest_jsonl_recursive(&path) {
                newest = max_by_mtime(newest, candidate);
            }
        } else if path.extension().is_some_and(|ext| ext == "jsonl") {
            newest = max_by_mtime(newest, path);
        }
    }
    newest.map(|(_, path)| path)
}

/// The most recently modified `.jsonl` directly inside `dir` (a workspace is a
/// repository: no recursive walk through vendor trees).
fn newest_jsonl_shallow(dir: &Path) -> Option<PathBuf> {
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            newest = max_by_mtime(newest, path);
        }
    }
    newest.map(|(_, path)| path)
}

/// Keeps whichever of `current` and `candidate` is newer (mtime, path as the tiebreak).
fn max_by_mtime(current: Option<(SystemTime, PathBuf)>, candidate: PathBuf) -> Option<(SystemTime, PathBuf)> {
    let mtime = candidate
        .metadata()
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    match current {
        Some((best_time, best_path)) if best_time >= mtime => Some((best_time, best_path)),
        _ => Some((mtime, candidate)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_thinking_tool_calls_and_context_tokens_from_session_jsonl() {
        let sample = r#"
{"message":{"role":"user","content":[{"type":"text","text":"Split OAuth flow into T-01"}]}}
{"message":{"role":"assistant","usage":{"input_tokens":12000,"cache_read_input_tokens":30000,"output_tokens":900},"content":[{"type":"thinking","thinking":"Inspecting repository boundaries before setting T-01 Contract...","duration_ms":3400},{"type":"tool_use","name":"lead_set_contract","input":{"task":"T-01","gate":"cargo test"}},{"type":"text","text":"Contract set for T-01 and Peer spawned."}]}}
"#;
        let (items, usage) = parse_session_jsonl(sample);
        assert_eq!(usage.total(), 42900);
        assert_eq!(usage.cache_read, 30000);
        assert_eq!(usage.fresh_input, 12000);
        assert_eq!(usage.output, 900);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].kind, "message");
        assert_eq!(items[1].kind, "thinking");
        assert!(items[1].body.contains("Inspecting repository boundaries"));
        assert_eq!(items[2].kind, "tool_call");
        assert_eq!(items[2].title, "lead_set_contract");
        assert_eq!(items[3].kind, "message");
    }
}
