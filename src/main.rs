pub mod auth;
pub mod contracts;
pub mod guard;
pub mod hc;
pub mod hygiene;
pub mod inspector;
pub mod provisioner;
pub mod role_homes;
pub mod runtime;
pub mod seats;
pub mod skills;
pub mod slp;

use contracts::RenderedContracts;
use guard::{Role, evaluate_pre_tool_hook};
use inspector::{SessionInspection, TranscriptItem};
use provisioner::{
    PlanError, RUNTIME_STATE_DIR, base_home_plan, find_on_path, find_role_credential_files,
    guard_removal_root, removal_paths, resolve_hotpot_home, runtime_opt_in_from_env,
    runtime_state_active,
};
use serde_json::{Value, json};
use slp::{HotpotStatusSnapshot, MailboxEntry, SeatStatus, TaskStatus};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

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
    let correlation = parse_flag(args, "--correlation")
        .or_else(|| env::var("HOTPOT_CORRELATION").ok());

    let mut input = String::new();
    let _ = io::stdin().read_to_string(&mut input);
    let payload: Value = serde_json::from_str(&input).unwrap_or(Value::Null);
    let response = evaluate_pre_tool_hook(role, &payload, &workspace, &state_dir, None);
    println!("{}", response);

    // Runtime evidence capture: allowed tool calls of a correlated peer seat are
    // recorded as minimal ledger events for its workspace, so a later `handoff`
    // can check that the Gate ran after the peer's last write (ticket 09).
    if let Some(correlation) = correlation.as_deref().filter(|c| !c.is_empty())
        && role == Role::Agent
        && !state_dir.is_empty()
    {
        let allowed = response["hookSpecificOutput"]["permissionDecision"] == "allow";
        runtime::evidence::record_hook_event(Path::new(&state_dir), correlation, &payload, allowed);
    }
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
    // The server-side session identity pin a peer seat derives its assignment
    // binding from; never taken from tool arguments.
    let correlation =
        parse_flag(args, "--correlation").or_else(|| env::var("HOTPOT_CORRELATION").ok());

    if let Err(e) = runtime::serve_mcp(
        role,
        Path::new(&project),
        Path::new(&state_dir),
        correlation.as_deref(),
    ) {
        eprintln!("hotpot mcp: {e}");
        std::process::exit(1);
    }
}

/// `hotpot setup [--apply]`: dry-run desired-state planning over Hotpot Home by default;
/// `--apply` materializes exactly the planned set (nine Role Homes with their vendor configs,
/// additive role contracts, hygiene closures, skill projections with the built-in
/// `hotpot-project-onboarding` skill, `AUTHENTICATION.md`, and the `hotpot.json` marker).
/// Both modes first run the Capability Hygiene gate
/// ([`hygiene::run_checks`] in [`hygiene::CheckMode::PreApply`]) and fail closed — nothing
/// applied, nothing printed — when existing seat state violates the closures or an operator
/// MCP config wires an orchestration backdoor.
fn cmd_setup(args: &[String]) -> Result<(), PlanError> {
    let apply = args.iter().any(|a| a == "--apply");
    let home = resolve_hotpot_home()?;
    let failures: Vec<_> = hygiene::run_checks(&home, hygiene::CheckMode::PreApply)
        .into_iter()
        .filter(|result| !result.ok)
        .collect();
    if !failures.is_empty() {
        for failure in &failures {
            eprintln!("{}", failure.render());
        }
        return Err(PlanError::new(
            "hygiene-gate",
            format!(
                "{} capability-hygiene check(s) failed; refusing to touch Hotpot Home \
                 (run `hotpot verify` for the full report)",
                failures.len()
            ),
        ));
    }
    let contracts = RenderedContracts::from_env();
    let runtime_opt_in = runtime_opt_in_from_env();
    let hotpot_bin = env::current_exe().map_err(|e| {
        PlanError::new("io", format!("failed to resolve the running hotpot binary: {e}"))
    })?;
    let hc_available = find_on_path("hc").is_some();
    // The operator's HOME is a read-only input: its per-vendor skill roots feed the
    // projections; nothing under it is ever written.
    let operator_home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let plan = base_home_plan(
        &home,
        &hotpot_bin,
        &contracts,
        runtime_opt_in,
        hc_available,
        &operator_home,
    )?;
    plan.validate()?;

    let stdout = io::stdout();
    let mut out = stdout.lock();
    if !apply {
        plan.print_dry_run(&mut out)
            .map_err(|e| PlanError::new("io", format!("failed to print dry-run plan: {e}")))?;
        return Ok(());
    }
    plan.apply(&mut out)
}

/// `hotpot verify`: runs every Capability Hygiene check ([`hygiene::run_checks`]) over the
/// Hotpot Home and the read-only operator inputs, then reports the per-seat authentication
/// diagnostics ([`auth::diagnostics`]) as non-blocking, secret-free, offline lines. Exits 0
/// with the report on stdout when all closures hold (diagnostics never affect the status);
/// exits non-zero naming each failing check on stderr otherwise.
fn cmd_verify() -> Result<(), PlanError> {
    let home = resolve_hotpot_home()?;
    let results = hygiene::run_checks(&home, hygiene::CheckMode::Verify);
    let mut failed = 0usize;
    for result in &results {
        if result.ok {
            println!("{}", result.render());
        } else {
            eprintln!("{}", result.render());
            failed += 1;
        }
    }
    println!("verify: auth diagnostics (non-blocking, offline, secret-free)");
    for diagnostic in auth::diagnostics(&home) {
        println!("{}", diagnostic.render());
    }
    if failed == 0 {
        println!("verify: all checks passed ({} ok)", results.len());
        Ok(())
    } else {
        Err(PlanError::new(
            "verify-failed",
            format!("{failed} capability-hygiene check(s) failed"),
        ))
    }
}

/// `hotpot remove [--apply]`: dry-run lists planned deletions and warns about role-owned
/// credentials and active runtime state; `--apply` tears down the whole Hotpot Home.
fn cmd_remove(args: &[String]) -> Result<(), PlanError> {
    let apply = args.iter().any(|a| a == "--apply");
    let home = resolve_hotpot_home()?;
    if !home.exists() {
        println!("nothing to remove: {}", home.display());
        return Ok(());
    }
    guard_removal_root(&home)?;

    // Warnings come before any removal listing, in dry-run and apply alike.
    for credential in find_role_credential_files(&home) {
        eprintln!(
            "warning: role-owned credential file would be deleted: {}",
            credential.display()
        );
    }
    if runtime_state_active(&home) {
        eprintln!(
            "warning: active runtime state present ({} is non-empty); it would be deleted",
            home.join(RUNTIME_STATE_DIR).display()
        );
    }

    let paths = removal_paths(&home);
    if !apply {
        for path in &paths {
            println!("would remove: {}", path.display());
        }
        println!(
            "dry-run: nothing removed (pass --apply to tear down {})",
            home.display()
        );
        return Ok(());
    }

    fs::remove_dir_all(&home).map_err(|e| {
        PlanError::new(
            "io",
            format!("failed to remove {}: {e}", home.display()),
        )
    })?;
    for path in paths.iter().rev() {
        println!("removed: {}", path.display());
    }
    Ok(())
}

/// Default `Hotpot Home` (`~/.hotpot`), overridable via `HOTPOT_HOME`.
fn default_hotpot_home() -> String {
    env::var("HOME")
        .map(|h| format!("{h}/.hotpot"))
        .unwrap_or_else(|_| ".hotpot".to_string())
}

/// `hotpot export <project> --out <path>`: write a bundle of one Project's
/// runtime state — assignment projections, the ledger event log (the runtime
/// transcript of asks, handoffs, and gate completions), and the gate tails it
/// references — with secret masking applied to the whole bundle before it is
/// written (spec: "Secret Masking on Gate Tails & Exports").
fn cmd_export(args: &[String]) {
    let usage = "usage: hotpot export <project-path-or-slug> --out <path> [--state-dir <dir>]";
    let Some(project) = args.first().map(String::as_str).filter(|s| !s.is_empty()) else {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    };
    let Some(out) = parse_flag(args, "--out").filter(|s| !s.is_empty()) else {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    };
    let state_dir = PathBuf::from(
        parse_flag(args, "--state-dir")
            .or_else(|| env::var("HOTPOT_HOME").ok())
            .unwrap_or_else(default_hotpot_home),
    );

    // The project is either a repository path (canonicalized to its runtime
    // slug) or the slug itself.
    let slug = if Path::new(project).is_dir() {
        runtime::project_slug(Path::new(project))
    } else {
        project.to_string()
    };
    let ledger = runtime::Ledger::at(
        state_dir
            .join("runtime")
            .join("v1")
            .join(&slug)
            .join("ledger.jsonl"),
    );
    if !ledger.path().exists() {
        eprintln!(
            "hotpot export: no runtime ledger for project `{slug}` under {}; run the runtime coordinator first",
            state_dir.display()
        );
        std::process::exit(1);
    }
    let events = ledger.read_events();
    let assignments: Vec<Value> = runtime::RuntimeState::from_events(&events)
        .assignments()
        .iter()
        .map(|a| a.to_json())
        .collect();
    let assignments_len = assignments.len();
    // Gate tails referenced by events travel with the bundle (already masked
    // on write; masking the bundle again is a no-op for them).
    let mut tails = serde_json::Map::new();
    for event in &events {
        if let Some(tail) = event.get("tail").and_then(Value::as_str)
            && !tails.contains_key(tail)
            && let Ok(content) = fs::read_to_string(tail)
        {
            tails.insert(tail.to_string(), Value::String(content));
        }
    }
    let exported_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let bundle = json!({
        "project": slug,
        "exportedAt": exported_at,
        "assignments": assignments,
        "events": events,
        "gateTails": Value::Object(tails),
    });
    let masked = runtime::masking::mask_text(
        &serde_json::to_string_pretty(&bundle).unwrap_or_else(|_| "{}".to_string()),
    );
    if let Some(parent) = Path::new(&out).parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(e) = fs::write(&out, masked) {
        eprintln!("hotpot export: failed to write {}: {e}", out);
        std::process::exit(1);
    }
    println!(
        "export: {} ({} events, {} assignments; secrets masked)",
        out,
        events.len(),
        assignments_len
    );
}

/// `hotpot gate-runner`: internal entry point of the detached background Gate,
/// spawned by the `gate_run` MCP tool. Runs one assignment's Gate against its
/// candidate commit to completion and appends the `gate_completed` ledger
/// event (green | red with a masked tail | uncertain).
fn cmd_gate_runner(args: &[String]) {
    let usage =
        "usage: hotpot gate-runner --state-dir <dir> --project <root> --assignment <id> [--timeout-ms <n>]";
    let Some(state_dir) = parse_flag(args, "--state-dir") else {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    };
    let Some(project) = parse_flag(args, "--project") else {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    };
    let Some(assignment) = parse_flag(args, "--assignment") else {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    };
    let timeout_ms = parse_flag(args, "--timeout-ms")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .unwrap_or(runtime::gate::DEFAULT_TIMEOUT_MS);
    if let Err(e) = runtime::gate::run(
        Path::new(&state_dir),
        Path::new(&project),
        &assignment,
        timeout_ms,
    ) {
        eprintln!("hotpot gate-runner: {e}");
        std::process::exit(1);
    }
}

/// `hotpot auth login <codex|claude|pi> <supervisor|lead|peer>`: interactive vendor login
/// scoped to exactly one Seat's Role Home (credentials land inside that seat only).
fn cmd_auth(args: &[String]) {
    let usage = "usage: hotpot auth login <codex|claude|pi> <supervisor|lead|peer>";
    if args.first().map(String::as_str) != Some("login") || args.len() != 3 {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    }
    let (agent, role) = (args[1].as_str(), args[2].as_str());
    let home = match resolve_hotpot_home() {
        Ok(home) => home,
        Err(err) => {
            eprintln!("hotpot: {err}");
            std::process::exit(1);
        }
    };
    match auth::login(&home, agent, role) {
        Ok(code) if code != 0 => {
            eprintln!(
                "hotpot: `{agent} login` for the {agent}/{role} seat exited with {code}"
            );
            std::process::exit(code);
        }
        Ok(_) => println!("auth: {agent} {role} login session scoped to {}", role_homes::role_home_dir(&home, agent, role).display()),
        Err(err) => {
            eprintln!("hotpot: {err}");
            std::process::exit(1);
        }
    }
}

/// `hotpot seat launch <supervisor|lead|peer> --agent <codex|claude|pi> --project <path>
/// [--assignment <id> --parent <lead-terminal>]`: creates an `hc` terminal running the
/// vendor CLI scoped to the seat's Role Home with the full canonical `hotpot.*` label set,
/// reusing the Project's existing live or idle/resumable Lead and halting on duplicates.
fn cmd_seat(args: &[String]) {
    let usage = "usage: hotpot seat launch <supervisor|lead|peer> --agent <codex|claude|pi> \
                 --project <path> [--assignment <id> --parent <lead-terminal>]";
    if args.first().map(String::as_str) != Some("launch") {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    }
    let role = match args.get(1) {
        Some(role) => role.clone(),
        None => {
            eprintln!("hotpot: [usage] {usage}");
            std::process::exit(2);
        }
    };
    let Some(agent) = parse_flag(args, "--agent") else {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    };
    let Some(project) = parse_flag(args, "--project") else {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    };
    let assignment = parse_flag(args, "--assignment");
    let parent = parse_flag(args, "--parent");
    let binding = match (assignment, parent) {
        (Some(assignment), Some(parent)) => Some(seats::PeerBinding { assignment, parent }),
        (None, None) => None,
        _ => {
            eprintln!("hotpot: --assignment and --parent go together (both or neither); {usage}");
            std::process::exit(2);
        }
    };

    let home = match resolve_hotpot_home() {
        Ok(home) => home,
        Err(err) => {
            eprintln!("hotpot: {err}");
            std::process::exit(1);
        }
    };
    let slug = runtime::project_slug(Path::new(&project));
    match seats::launch_seat(&home, &role, &agent, Path::new(&project), binding) {
        Ok(seats::LaunchOutcome::Launched) => {
            println!("seat: launched {agent} {role} terminal for project {slug}");
        }
        Ok(seats::LaunchOutcome::Reused { address }) => {
            println!(
                "seat: reusing existing lead terminal {address} for project {slug}; \
                 no new terminal created"
            );
        }
        Err(err) => {
            eprintln!("hotpot: {err}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let sub = args.get(1).map(String::as_str).unwrap_or("status");
    match sub {
        "status" => cmd_status(&args[2..]),
        "inspect" => cmd_inspect(&args[2..]),
        "hook" => cmd_hook(&args[2..]),
        "mcp" => cmd_mcp(&args[2..]),
        "setup" => {
            if let Err(err) = cmd_setup(&args[2..]) {
                eprintln!("hotpot: {err}");
                std::process::exit(1);
            }
        }
        "verify" => {
            if let Err(err) = cmd_verify() {
                eprintln!("hotpot: {err}");
                std::process::exit(1);
            }
        }
        "remove" => {
            if let Err(err) = cmd_remove(&args[2..]) {
                eprintln!("hotpot: {err}");
                std::process::exit(1);
            }
        }
        "auth" => cmd_auth(&args[2..]),
        "seat" => cmd_seat(&args[2..]),
        "export" => cmd_export(&args[2..]),
        "gate-runner" => cmd_gate_runner(&args[2..]),
        "--help" | "-h" | "help" => {
            println!("hotpot 0.1.0 — Rust SLP verification engine & session inspector for Herdr");
            println!("Commands: status, inspect, hook, mcp, setup [--apply], verify, remove [--apply],");
            println!("           auth login <codex|claude|pi> <supervisor|lead|peer>,");
            println!("           seat launch <supervisor|lead|peer> --agent <codex|claude|pi> --project <path>,");
            println!("           export <project> --out <path>");
        }
        _ => cmd_status(&args[1..]),
    }
}
