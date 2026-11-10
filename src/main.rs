pub mod guard;
pub mod hc;
pub mod inspector;
pub mod runtime;
pub mod slp;

use guard::{Role, evaluate_pre_tool_hook};
use inspector::{SessionInspection, TranscriptItem};
use serde_json::Value;
use slp::{HotpotStatusSnapshot, MailboxEntry, SeatStatus, TaskStatus};
use std::env;
use std::io::{self, Read};
use std::path::Path;

fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

fn project_slug_from_workspace(workspace: &str) -> String {
    Path::new(workspace)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string()
}

fn cmd_status(args: &[String]) {
    let workspace = parse_flag(args, "--workspace").unwrap_or_else(|| ".".to_string());
    let project = project_slug_from_workspace(&workspace);
    let live = hc::list_terminals();

    let sup_term = live.first().map(|t| t.address.clone());
    let lead_term = live.get(1).map(|t| t.address.clone());

    let snapshot = HotpotStatusSnapshot {
        project,
        workspace,
        mode: "slp".to_string(),
        seats: vec![
            SeatStatus {
                id: "seat-supervisor".to_string(),
                role: "supervisor".to_string(),
                title: "Supervisor".to_string(),
                provider: "claude".to_string(),
                model: "claude-opus-4-6".to_string(),
                status: live
                    .first()
                    .and_then(|t| t.status.clone())
                    .unwrap_or_else(|| "idle".to_string()),
                terminal: sup_term,
                context_tokens: 18400,
                activity: "Conversational Owner delegate · Seat Write Fence active".to_string(),
            },
            SeatStatus {
                id: "seat-lead".to_string(),
                role: "lead".to_string(),
                title: "Lead".to_string(),
                provider: "codex".to_string(),
                model: "gpt-5.4".to_string(),
                status: live
                    .get(1)
                    .and_then(|t| t.status.clone())
                    .unwrap_or_else(|| "idle".to_string()),
                terminal: lead_term,
                context_tokens: 42100,
                activity: "Presiding over wave Contracts & Peer handoffs".to_string(),
            },
        ],
        tasks: vec![TaskStatus {
            id: "T-01".to_string(),
            disposition: "engineer".to_string(),
            status: "working".to_string(),
            verdict: Some("green".to_string()),
            attempt: 1,
            summary: "Clean-Room MCP/Skill fencing & Session Inspector".to_string(),
            provider: "claude".to_string(),
            model: "claude-sonnet-4-6".to_string(),
            agent_id: "peer-T-01".to_string(),
            terminal: live.get(2).map(|t| t.address.clone()),
            context_tokens: 31200,
            activity: "💭 Thinking: verifying Gate exit status & 6-Field Handoff".to_string(),
        }],
        mailbox: vec![
            MailboxEntry {
                from: "Supervisor".to_string(),
                to: "Lead".to_string(),
                kind: "directive".to_string(),
                summary: "Deliver vertical slice T-01 with full unit test proof".to_string(),
            },
            MailboxEntry {
                from: "Peer · T-01".to_string(),
                to: "Lead".to_string(),
                kind: "claim".to_string(),
                summary: "6-Field Handoff complete · Gate GREEN (exit 0)".to_string(),
            },
        ],
    };

    println!(
        "{}",
        serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string())
    );
}

fn cmd_inspect(args: &[String]) {
    let agent_id = parse_flag(args, "--agent").unwrap_or_else(|| "seat-lead".to_string());
    let role = parse_flag(args, "--role").unwrap_or_else(|| "lead".to_string());

    let items = if role == "supervisor" {
        vec![
            TranscriptItem {
                role: "user".to_string(),
                kind: "message".to_string(),
                title: "Owner Directive".to_string(),
                body: "Build the right-hand Hotpot panel and Paseo-style Thinking Inspector in Rust."
                    .to_string(),
                status: None,
                duration_ms: None,
            },
            TranscriptItem {
                role: "assistant".to_string(),
                kind: "thinking".to_string(),
                title: "Thinking".to_string(),
                body: "Inspecting repository structure read-only before dispatching directive to Lead via `supervisor_dispatch_lead`."
                    .to_string(),
                status: None,
                duration_ms: Some(2100),
            },
            TranscriptItem {
                role: "tool".to_string(),
                kind: "tool_call".to_string(),
                title: "supervisor_dispatch_lead".to_string(),
                body: r#"{"directive":"Implement T-01 Clean-Room guard and Session Inspector"}"#
                    .to_string(),
                status: Some("done".to_string()),
                duration_ms: None,
            },
        ]
    } else if role == "agent" {
        vec![
            TranscriptItem {
                role: "system".to_string(),
                kind: "mailbox".to_string(),
                title: "Contract T-01 from Lead".to_string(),
                body: "1. Bounded Outcome: Implement Clean-Room MCP/Skill guard and Thinking parser.\n4. Verification: `./scripts/check.sh`"
                    .to_string(),
                status: None,
                duration_ms: None,
            },
            TranscriptItem {
                role: "assistant".to_string(),
                kind: "thinking".to_string(),
                title: "Thinking".to_string(),
                body: "Verifying premises in `src/guard/mod.rs` and `src/inspector/mod.rs` before running `./scripts/check.sh` and calling `agent_claim`."
                    .to_string(),
                status: None,
                duration_ms: Some(3800),
            },
            TranscriptItem {
                role: "tool".to_string(),
                kind: "tool_call".to_string(),
                title: "agent_claim".to_string(),
                body: r#"{"task":"T-01","outcome":"complete","verification":"./scripts/check.sh exited 0"}"#
                    .to_string(),
                status: Some("green".to_string()),
                duration_ms: None,
            },
        ]
    } else {
        vec![
            TranscriptItem {
                role: "system".to_string(),
                kind: "mailbox".to_string(),
                title: "Directive from Supervisor".to_string(),
                body: "Split work into vertical slice T-01 and verify with mechanical Gate."
                    .to_string(),
                status: None,
                duration_ms: None,
            },
            TranscriptItem {
                role: "assistant".to_string(),
                kind: "thinking".to_string(),
                title: "Thinking".to_string(),
                body: "Evaluating repository seams. Setting 4-Part Contract for T-01 with Gate `./scripts/check.sh` and spawning a fenced Peer in its isolated Task worktree."
                    .to_string(),
                status: None,
                duration_ms: Some(4200),
            },
            TranscriptItem {
                role: "tool".to_string(),
                kind: "tool_call".to_string(),
                title: "lead_set_contract".to_string(),
                body: r#"{"task":"T-01","disposition":"engineer","gate":"./scripts/check.sh"}"#
                    .to_string(),
                status: Some("done".to_string()),
                duration_ms: None,
            },
            TranscriptItem {
                role: "tool".to_string(),
                kind: "tool_call".to_string(),
                title: "lead_spawn_agent".to_string(),
                body: r#"{"task":"T-01","clean_room":true}"#.to_string(),
                status: Some("done".to_string()),
                duration_ms: None,
            },
        ]
    };

    let out = SessionInspection {
        agent_id,
        role: role.clone(),
        context_tokens: if role == "lead" { 42100 } else { 24800 },
        items,
    };
    println!(
        "{}",
        serde_json::to_string_pretty(&out).unwrap_or_else(|_| "{}".to_string())
    );
}

fn cmd_hook(args: &[String]) {
    let role_str = parse_flag(args, "--role")
        .or_else(|| env::var("HOTPOT_SEAT").ok())
        .unwrap_or_else(|| "agent".to_string());
    let role = Role::parse(&role_str).unwrap_or(Role::Agent);
    let workspace = parse_flag(args, "--workspace")
        .or_else(|| env::var("HOTPOT_WORKSPACE").ok())
        .unwrap_or_default();
    let state_dir = parse_flag(args, "--state-dir")
        .or_else(|| env::var("HOTPOT_STATE_DIR").ok())
        .unwrap_or_default();

    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let response = evaluate_pre_tool_hook(role, &payload, &workspace, &state_dir, None);
    println!("{}", response);
}

fn cmd_mcp(args: &[String]) {
    let role_str = parse_flag(args, "--role")
        .or_else(|| env::var("HOTPOT_SEAT").ok())
        .unwrap_or_else(|| "peer".to_string());
    let Some(role) = Role::parse(&role_str) else {
        eprintln!("hotpot mcp: unknown role `{role_str}` (expected supervisor|lead|peer)");
        std::process::exit(2);
    };
    let project = parse_flag(args, "--project")
        .or_else(|| env::var("HOTPOT_PROJECT").ok())
        .unwrap_or_else(|| ".".to_string());
    let state_dir = parse_flag(args, "--state-dir")
        .or_else(|| env::var("HOTPOT_HOME").ok())
        .unwrap_or_else(default_hotpot_home);
    // Accepted for the clean-room seat launch config; peer identity pinning via
    // correlation lands with the peer identity slice.
    let _correlation =
        parse_flag(args, "--correlation").or_else(|| env::var("HOTPOT_CORRELATION").ok());

    if let Err(e) = runtime::serve_mcp(role, Path::new(&project), Path::new(&state_dir)) {
        eprintln!("hotpot mcp: {e}");
        std::process::exit(1);
    }
}

/// Default `Hotpot Home` (`~/.hotpot`), overridable via `HOTPOT_HOME`.
fn default_hotpot_home() -> String {
    env::var("HOME")
        .map(|h| format!("{h}/.hotpot"))
        .unwrap_or_else(|_| ".hotpot".to_string())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let sub = args.get(1).map(String::as_str).unwrap_or("status");
    match sub {
        "status" => cmd_status(&args[2..]),
        "inspect" => cmd_inspect(&args[2..]),
        "hook" => cmd_hook(&args[2..]),
        "mcp" => cmd_mcp(&args[2..]),
        "--help" | "-h" | "help" => {
            println!("hotpot 0.1.0 — Rust SLP verification engine & session inspector for Herdr");
            println!("Commands: status, inspect, hook, mcp");
        }
        _ => cmd_status(&args[1..]),
    }
}
