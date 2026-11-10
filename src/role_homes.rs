//! Role Home generation: per-seat vendor configs, additive instruction delivery, and seat
//! credential declarations.
//!
//! A Role Home (`<Hotpot Home>/roles/<agent>/<role>/`) wires the vendor to the role-scoped
//! `hotpot mcp` stdio server with a per-seat correlation id (the same wiring shape as the
//! clean-room homes in `src/guard/mod.rs`), delivers the additive role contract through the
//! vendor's append channel, and declares its own file-backed credential and session-history
//! paths — never symlinks into the operator's homes.

use crate::hygiene;
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

/// Merges `overlay`'s top-level keys into the JSON object `base` (later keys win).
fn merge_objects(base: &mut Value, overlay: &Value) {
    if let (Some(base_obj), Some(overlay_obj)) = (base.as_object_mut(), overlay.as_object()) {
        for (key, value) in overlay_obj {
            base_obj.insert(key.clone(), value.clone());
        }
    }
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
/// server, the agent's additive instruction artifacts, and the Capability Hygiene closures
/// (see [`hygiene`]). The same rendered `contract` is delivered to every agent family for a
/// given role; `codex_catalog` is the already-scrubbed model catalog projected into Codex
/// seats (ignored by the other families).
pub fn seat_plan_items(
    home: &Path,
    hotpot_bin: &Path,
    agent: &str,
    role: &str,
    contract: &str,
    codex_catalog: &str,
) -> Vec<PlanItem> {
    let role_home = role_home_dir(home, agent, role);
    let mcp = hotpot_mcp_server(hotpot_bin, home, agent, role);
    let credentials_file = seat_credentials_file(&role_home, agent);
    let session_history_file = seat_session_history_file(&role_home, agent);
    let path = |name: &str| role_home.join(name);

    match agent {
        // Codex: everything lives in `config.json`; the contract rides the additive
        // `developer_instructions` field (no base system-prompt field is ever replaced), and
        // the hygiene closures disable agents/multi_agent in both config scopes. The
        // scrubbed model catalog is projected alongside as `models.json`.
        "codex" => {
            let mut config = json!({
                "mcpServers": { "hotpot": mcp },
                "developer_instructions": contract,
                "credentials_file": credentials_file,
                "session_history_file": session_history_file,
            });
            merge_objects(&mut config, &hygiene::codex_closure_settings());
            vec![
                PlanItem::File {
                    path: path("config.json"),
                    contents: json_file_contents(&config),
                },
                PlanItem::File {
                    path: path("models.json"),
                    contents: codex_catalog.to_string(),
                },
            ]
        }
        // Claude: `mcp.json` carries the MCP wiring; `settings.json` carries the
        // system-prompt append channel, the seat declarations, and the hygiene closures (deny
        // list + disable flags); `CLAUDE.md` is the in-home fallback carrying the same
        // contract; `launch.json` records the seat's launch flags (deny entries repeated).
        "claude" => {
            let mcp_json = json!({ "mcpServers": { "hotpot": mcp } });
            let mut settings = json!({
                "appendSystemPrompt": contract,
                "env": { "CLAUDE_CONFIG_DIR": role_home },
                "credentials_file": credentials_file,
                "session_history_file": session_history_file,
            });
            merge_objects(&mut settings, &hygiene::claude_closure_settings());
            let launch = json!({ "args": hygiene::claude_deny_launch_args() });
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
                PlanItem::File {
                    path: path("launch.json"),
                    contents: json_file_contents(&launch),
                },
            ]
        }
        // Pi: `settings.json` carries the MCP wiring and seat declarations;
        // `APPEND_SYSTEM.md` is the additive system-prompt append file; `launch.json`
        // records the closed extension launch args.
        "pi" => {
            let settings = json!({
                "mcpServers": { "hotpot": mcp },
                "append_system_prompt_file": "APPEND_SYSTEM.md",
                "credentials_file": credentials_file,
                "session_history_file": session_history_file,
            });
            vec![
                PlanItem::File {
                    path: path("launch.json"),
                    contents: hygiene::pi_launch_config_contents(),
                },
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
