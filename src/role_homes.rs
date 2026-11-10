//! Role Home generation: per-seat vendor configs, additive instruction delivery, and seat
//! credential declarations.
//!
//! A Role Home (`<Hotpot Home>/roles/<agent>/<role>/`) wires the vendor to the role-scoped
//! `hotpot mcp` stdio server with a per-seat correlation id (the same wiring shape as the
//! clean-room homes in `src/guard/mod.rs`), delivers the additive role contract through the
//! vendor's append channel, and declares its own file-backed credential and session-history
//! paths — never symlinks into the operator's homes.

use crate::provisioner::PlanItem;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The Role Home directory of one seat under Hotpot Home.
pub fn role_home_dir(home: &Path, agent: &str, role: &str) -> PathBuf {
    home.join("roles").join(agent).join(role)
}

/// Deterministic per-seat MCP correlation id: `<agent>-<role>` (unique across the nine seats,
/// stable across `setup` re-runs).
pub fn seat_correlation(agent: &str, role: &str) -> String {
    format!("{agent}-{role}")
}

/// stdio server wiring for the role-scoped `hotpot mcp` process, mirroring the clean-room
/// `mcp.json` wiring shape (`src/guard/mod.rs` `materialize_clean_room_home`).
fn hotpot_mcp_server(hotpot_bin: &Path, home: &Path, agent: &str, role: &str) -> Value {
    json!({
        "type": "stdio",
        "command": hotpot_bin.to_string_lossy(),
        "args": [
            "mcp",
            "--role", role,
            "--correlation", seat_correlation(agent, role),
            "--state-dir", home.to_string_lossy(),
        ]
    })
}

/// Renders a JSON value as pretty-printed file contents (trailing newline).
fn json_file_contents(value: &Value) -> String {
    serde_json::to_string_pretty(value).unwrap() + "\n"
}

/// The seat's own credential file path (created by `hotpot auth login` / the vendor — never
/// planned as a symlink to operator credential files).
fn seat_credentials_file(role_home: &Path, agent: &str) -> PathBuf {
    match agent {
        // Vendor-native names: codex stores `auth.json`, claude `.credentials.json`.
        "claude" => role_home.join(".credentials.json"),
        _ => role_home.join("auth.json"),
    }
}

/// The seat's own session-history file path inside its Role Home.
fn seat_session_history_file(role_home: &Path, agent: &str) -> PathBuf {
    match agent {
        "claude" => role_home.join("history").join("projects.jsonl"),
        _ => role_home.join("history").join("sessions.jsonl"),
    }
}

/// Plan items for one seat's Role Home: the vendor config wiring the role-scoped `hotpot mcp`
/// server plus the agent's additive instruction artifacts. The same rendered `contract` is
/// delivered to every agent family for a given role.
pub fn seat_plan_items(
    home: &Path,
    hotpot_bin: &Path,
    agent: &str,
    role: &str,
    contract: &str,
) -> Vec<PlanItem> {
    let role_home = role_home_dir(home, agent, role);
    let mcp = hotpot_mcp_server(hotpot_bin, home, agent, role);
    let credentials_file = seat_credentials_file(&role_home, agent);
    let session_history_file = seat_session_history_file(&role_home, agent);
    let path = |name: &str| role_home.join(name);

    match agent {
        // Codex: everything lives in `config.json`; the contract rides the additive
        // `developer_instructions` field (no base system-prompt field is ever replaced).
        "codex" => {
            let config = json!({
                "mcpServers": { "hotpot": mcp },
                "developer_instructions": contract,
                "credentials_file": credentials_file,
                "session_history_file": session_history_file,
            });
            vec![PlanItem::File {
                path: path("config.json"),
                contents: json_file_contents(&config),
            }]
        }
        // Claude: `mcp.json` carries the MCP wiring; `settings.json` carries the
        // system-prompt append channel plus the seat declarations; `CLAUDE.md` is the
        // in-home fallback carrying the same contract.
        "claude" => {
            let mcp_json = json!({ "mcpServers": { "hotpot": mcp } });
            let settings = json!({
                "appendSystemPrompt": contract,
                "env": { "CLAUDE_CONFIG_DIR": role_home },
                "credentials_file": credentials_file,
                "session_history_file": session_history_file,
            });
            vec![
                PlanItem::File {
                    path: path("mcp.json"),
                    contents: json_file_contents(&mcp_json),
                },
                PlanItem::File {
                    path: path("settings.json"),
                    contents: json_file_contents(&settings),
                },
                PlanItem::File {
                    path: path("CLAUDE.md"),
                    contents: contract.to_string(),
                },
            ]
        }
        // Pi: `settings.json` carries the MCP wiring and seat declarations;
        // `APPEND_SYSTEM.md` is the additive system-prompt append file.
        "pi" => {
            let settings = json!({
                "mcpServers": { "hotpot": mcp },
                "append_system_prompt_file": "APPEND_SYSTEM.md",
                "credentials_file": credentials_file,
                "session_history_file": session_history_file,
            });
            vec![
                PlanItem::File {
                    path: path("settings.json"),
                    contents: json_file_contents(&settings),
                },
                PlanItem::File {
                    path: path("APPEND_SYSTEM.md"),
                    contents: contract.to_string(),
                },
            ]
        }
        other => unreachable!("seat_plan_items called for unknown agent {other}"),
    }
}

/// `AUTHENTICATION.md` at the Hotpot Home root: one exact, copy-pasteable
/// `hotpot auth login <agent> <role>` command per seat, and no secrets — hotpot never reads
/// credential file contents.
pub fn authentication_md() -> String {
    let mut commands = String::new();
    for agent in ["codex", "claude", "pi"] {
        for role in ["supervisor", "lead", "peer"] {
            commands.push_str(&format!("    hotpot auth login {agent} {role}\n"));
        }
    }
    format!(
        "# Hotpot Seat Authentication\n\
         \n\
         Every Seat owns its credentials and session history inside its Role Home under\n\
         Hotpot Home — never in the operator's `~/.codex`, `~/.claude`, or `~/.pi/agent`\n\
         directories, and never shared between Seats.\n\
         \n\
         Authenticate each Seat independently by running its command in a terminal:\n\
         \n\
         {commands}\
         \n\
         This file lists commands only; it stores no secrets."
    )
}
