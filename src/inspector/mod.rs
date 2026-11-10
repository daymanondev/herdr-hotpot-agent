//! Real-time Paseo-style Session & Thinking Inspector (`hotpot inspect`).
//! Parses on-disk `.jsonl` transcripts from Claude Code, Codex, Pi, and Antigravity
//! into structured `Thinking`, `Tool Call`, and `Message` blocks.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptItem {
    pub role: String,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub status: Option<String>,
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionInspection {
    pub agent_id: String,
    pub role: String,
    pub context_tokens: u64,
    pub items: Vec<TranscriptItem>,
}

/// Parses a JSONL stream from Claude Code, Codex, or Pi into structured `TranscriptItem` blocks,
/// preserving full uncollapsed `thinking` / `reasoning` blocks and tool calls.
pub fn parse_session_jsonl(jsonl: &str) -> (Vec<TranscriptItem>, u64) {
    let mut items = Vec::new();
    let mut latest_context_tokens: u64 = 0;

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
            if input + cache_read + output > 0 {
                latest_context_tokens = input + cache_read + output;
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
                        items.push(TranscriptItem {
                            role: "tool".to_string(),
                            kind: "tool_call".to_string(),
                            title: name.to_string(),
                            body: input,
                            status: Some("done".to_string()),
                            duration_ms: None,
                        });
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

        // Codex rollout `reasoning` / `function_call` events
        if let Some(payload) = val.get("payload") {
            let ptype = payload.get("type").and_then(Value::as_str).unwrap_or("");
            if ptype == "reasoning" {
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
            } else if ptype == "function_call" {
                let name = payload
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("function_call");
                let args = payload
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}");
                items.push(TranscriptItem {
                    role: "tool".to_string(),
                    kind: "tool_call".to_string(),
                    title: name.to_string(),
                    body: args.to_string(),
                    status: Some("done".to_string()),
                    duration_ms: None,
                });
            }
        }
    }

    (items, latest_context_tokens)
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
        let (items, tokens) = parse_session_jsonl(sample);
        assert_eq!(tokens, 42900);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0].kind, "message");
        assert_eq!(items[1].kind, "thinking");
        assert!(items[1].body.contains("Inspecting repository boundaries"));
        assert_eq!(items[2].kind, "tool_call");
        assert_eq!(items[2].title, "lead_set_contract");
        assert_eq!(items[3].kind, "message");
    }
}
