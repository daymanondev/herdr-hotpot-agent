pub mod auth;
pub mod contracts;
pub mod credential_seeding;
pub mod guard;
pub mod hc;
pub mod hygiene;
pub mod inspector;
pub mod profile;
pub mod provisioner;
pub mod role_homes;
pub mod runtime;
pub mod seats;
pub mod skills;
pub mod slp;
pub mod status;

use contracts::RenderedContracts;
use guard::{Role, evaluate_pre_tool_hook};
use inspector::SessionInspection;
use provisioner::{
    PlanError, RUNTIME_STATE_DIR, base_home_plan, find_on_path, find_role_credential_files,
    guard_removal_root, removal_paths, resolve_hotpot_home, runtime_opt_in_from_env,
    runtime_state_active,
};
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

fn parse_flag(args: &[String], flag: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == flag).map(|w| w[1].clone())
}

/// The Hotpot Home a read-only subcommand reads from: `--state-dir`, else
/// `HOTPOT_HOME`, else the default `~/.hotpot` (same chain as `hotpot export`).
fn state_dir_from(args: &[String]) -> PathBuf {
    PathBuf::from(
        parse_flag(args, "--state-dir")
            .or_else(|| env::var("HOTPOT_HOME").ok())
            .unwrap_or_else(default_hotpot_home),
    )
}

/// The project ledger path under `home` for `slug` (opened read-only: the
/// projection must never create runtime state).
fn ledger_path_for(home: &Path, slug: &str) -> PathBuf {
    home.join("runtime")
        .join("v1")
        .join(slug)
        .join("ledger.jsonl")
}

/// `hotpot status [--workspace <path>]`: the live snapshot of the Project —
/// seats projected from the runtime ledger plus `hc` terminal labels, the
/// assignment master list, the due Attention Letters, and the mailbox — served
/// as JSON. Empty or absent state yields empty arrays, never mocked entries.
fn cmd_status(args: &[String]) {
    let workspace = parse_flag(args, "--workspace").unwrap_or_else(|| ".".to_string());
    let home = state_dir_from(args);
    let slug = runtime::project_slug(Path::new(&workspace));
    // A broken `hc` listing degrades to "no terminal info"; it must not take the
    // whole panel down (launch decisions are the strict path, not this read).
    let terminals = hc::list_terminals();
    let snapshot = status::snapshot(
        &home,
        &workspace,
        &slug,
        &terminals,
        runtime::ledger::now_secs(),
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&snapshot).unwrap_or_else(|_| "{}".to_string())
    );
}

/// `hotpot inspect --agent <seat-id> [--workspace <path>]`: locate the seat's
/// real transcript `.jsonl` from its correlation / workspace records and serve
/// the parsed items (uncollapsed `thinking`, `tool_call`, `message`, and notice
/// kinds) plus the seat's segmented context. A seat with no transcript yet
/// answers with empty items, a stderr note, and exit 0.
fn cmd_inspect(args: &[String]) {
    let usage =
        "usage: hotpot inspect --agent <seat-id> [--workspace <path>] [--state-dir <dir>]";
    let Some(agent) = parse_flag(args, "--agent").filter(|a| !a.is_empty()) else {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    };
    // `--role` / `--terminal` are still sent by the current mods; the seat is
    // resolved by its `--agent` correlation id, so they are accepted and ignored.
    let workspace = parse_flag(args, "--workspace").unwrap_or_else(|| ".".to_string());
    let home = state_dir_from(args);
    let slug = runtime::project_slug(Path::new(&workspace));
    let events = runtime::Ledger::at(ledger_path_for(&home, &slug)).read_events();
    // The ledger knows every seat this runtime launched or dispatched; the `hc`
    // listing is only consulted when the ledger has no record (a seat launched
    // before its launches were recorded).
    let record = inspector::locate_seat_record(&events, &[], &agent).or_else(|| {
        let terminals = hc::list_terminals();
        inspector::locate_seat_record(&events, &terminals, &agent)
    });

    let Some(record) = record else {
        eprintln!(
            "hotpot inspect: no seat record for `{agent}` in project {slug}; nothing to inspect yet"
        );
        print_inspection(SessionInspection {
            agent_id: agent,
            role: String::new(),
            transcript: None,
            context: slp::ContextSegments::empty(),
            context_tokens: 0,
            items: Vec::new(),
        });
        return;
    };
    let ledger_context = || status::seat_context(&events, &record.seat, &record.role);

    let role_home = (!record.agent.is_empty() && !record.role.is_empty()).then(|| {
        role_homes::role_home_dir(&home, &record.agent, &record.role)
    });
    let transcript = inspector::locate_transcript(
        role_home.as_deref(),
        record.workspace.as_deref().map(Path::new),
        record.transcript.as_deref().map(Path::new),
    );

    let Some(transcript) = transcript else {
        let looked_at = role_home
            .as_ref()
            .map(|home| home.display().to_string())
            .unwrap_or_else(|| "the seat's workspace".to_string());
        eprintln!(
            "hotpot inspect: no transcript on disk yet for seat `{agent}` (looked under {looked_at})"
        );
        let context = ledger_context();
        print_inspection(SessionInspection {
            agent_id: record.seat,
            role: record.role,
            transcript: None,
            context,
            context_tokens: context.total,
            items: Vec::new(),
        });
        return;
    };

    let raw = fs::read_to_string(&transcript).unwrap_or_default();
    let (items, usage) = inspector::parse_session_jsonl(&raw);
    // Per-seat context segmentation: the transcript's own usage lines first, the
    // ledger's latest `context_usage` event for the seat as the fallback.
    let context = if usage.total() > 0 {
        slp::ContextSegments::from_parts(
            usage.cache_read,
            usage.fresh_input,
            usage.output,
            runtime::attention::ROTATION_MARK_TOKENS,
        )
    } else {
        ledger_context()
    };
    print_inspection(SessionInspection {
        agent_id: record.seat,
        role: record.role,
        transcript: Some(transcript.display().to_string()),
        context,
        context_tokens: context.total,
        items,
    });
}

/// Serves one inspection as pretty JSON on stdout.
fn print_inspection(inspection: SessionInspection) {
    println!(
        "{}",
        serde_json::to_string_pretty(&inspection).unwrap_or_else(|_| "{}".to_string())
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
    let state_dir = state_dir_from(args);

    // The project is either a repository path (canonicalized to its runtime
    // slug) or the slug itself.
    let slug = if Path::new(project).is_dir() {
        runtime::project_slug(Path::new(project))
    } else {
        project.to_string()
    };
    let ledger = runtime::Ledger::at(ledger_path_for(&state_dir, &slug));
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

/// `hotpot profile set <codex|claude|pi> <supervisor|lead|peer> <model>
/// --thinking <low|medium|high>`: the minimal write path behind the panel's
/// inline pills and named presets — atomically updates the role profile
/// defaults in Hotpot Home (`<HOTPOT_HOME>/profiles.json`), the file
/// `hotpot status` serves back to the seats.
fn cmd_profile(args: &[String]) {
    let usage = "usage: hotpot profile set <codex|claude|pi> <supervisor|lead|peer> <model> \
                 --thinking <low|medium|high>";
    if args.first().map(String::as_str) != Some("set") || args.len() != 6 {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    }
    let (agent, role, model) = (args[1].as_str(), args[2].as_str(), args[3].as_str());
    let Some(thinking) = parse_flag(args, "--thinking").filter(|t| !t.is_empty()) else {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    };
    let home = match resolve_hotpot_home() {
        Ok(home) => home,
        Err(err) => {
            eprintln!("hotpot: {err}");
            std::process::exit(1);
        }
    };
    if let Err(err) = profile::set(&home, agent, role, model, &thinking) {
        eprintln!("hotpot: {err}");
        std::process::exit(1);
    }
    println!("profile: set {agent}/{role} {model} · thinking {thinking}");
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

/// `hotpot attention feedback <att_id> <useful|noisy> [--workspace <path>] [--state-dir <dir>]`:
/// the UI-facing write path for Attention Letter feedback (the panel's Ack /
/// Dismiss buttons) — records one `attention_feedback` event into the Project
/// ledger without an open MCP stdio session, mapping the panel's vocabulary onto
/// the runtime's existing actions so every subsequent `hotpot status` batch and
/// `room_status` delivery honors it: `noisy` → `mute` (silence the key),
/// `useful` → `snooze` (acknowledged; suppressed for the rate window).
fn cmd_attention(args: &[String]) {
    let usage = "usage: hotpot attention feedback <att_id> <useful|noisy> \
                 [--workspace <path>] [--state-dir <dir>]";
    if args.first().map(String::as_str) != Some("feedback") {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    }
    // Positionals ride between value-taking flags; anything else is a usage
    // error so unknown forms fail closed like the rest of the CLI.
    let value_flags = ["--workspace", "--state-dir"];
    let mut positionals: Vec<&str> = Vec::new();
    let mut rest: &[String] = &args[1..];
    while let Some((arg, tail)) = rest.split_first() {
        if !arg.starts_with("--") {
            positionals.push(arg);
            rest = tail;
        } else if !value_flags.contains(&arg.as_str()) {
            eprintln!("hotpot: [usage] {usage} (unknown flag `{arg}`)");
            std::process::exit(2);
        } else if tail.is_empty() {
            eprintln!("hotpot: [usage] {usage} (`{arg}` requires a value)");
            std::process::exit(2);
        } else {
            rest = &tail[1..];
        }
    }
    if positionals.len() != 2 {
        eprintln!("hotpot: [usage] {usage}");
        std::process::exit(2);
    }
    let (att_id, action) = (positionals[0], positionals[1]);
    let mapped = match action {
        "noisy" => "mute",
        "useful" => "snooze",
        _ => {
            eprintln!("hotpot: [usage] {usage}");
            std::process::exit(2);
        }
    };
    // The letter-key shape every detection stamps: `att_<kind>_<discriminator>`.
    if att_id.strip_prefix("att_").is_none_or(|id| {
        id.is_empty() || id.chars().any(char::is_whitespace)
    }) {
        eprintln!("hotpot: [usage] {usage} (`{att_id}` is not an att_<id> letter key)");
        std::process::exit(2);
    }

    let workspace = parse_flag(args, "--workspace").unwrap_or_else(|| ".".to_string());
    let home = state_dir_from(args);
    let slug = runtime::project_slug(Path::new(&workspace));
    let ledger = runtime::Ledger::at(ledger_path_for(&home, &slug));
    if let Err(e) = ledger
        .ensure_parent_dir()
        .and_then(|_| ledger.append(json!({"type": "attention_feedback", "key": att_id, "action": mapped})))
    {
        eprintln!("hotpot: failed to record the feedback event: {e}");
        std::process::exit(1);
    }
    println!("attention: {att_id} marked {action} ({mapped})");
}

/// `hotpot seat launch <supervisor|lead|peer> --agent <codex|claude|pi> --project <path>
/// [--assignment <id> --parent <lead-terminal>] [--prompt <text>]`: creates an `hc`
/// terminal running the vendor CLI scoped to the seat's Role Home with the full canonical
/// `hotpot.*` label set, reusing the Project's existing live or idle/resumable Lead and
/// halting on duplicates. `--prompt` additionally delivers an initial directive into the
/// launched or reused seat's terminal (`hc send`, best-effort and honestly reported) and
/// records it in the runtime ledger so the next `hotpot status` mailbox and `hotpot
/// inspect` surface it (ticket nui/04).
fn cmd_seat(args: &[String]) {
    let usage = "usage: hotpot seat launch <supervisor|lead|peer> --agent <codex|claude|pi> \
                 --project <path> [--assignment <id> --parent <lead-terminal>] \
                 [--prompt <text>]";
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
    // An empty directive is a usage error, not a silent no-op.
    let prompt = parse_flag(args, "--prompt");
    if args.iter().any(|a| a == "--prompt")
        && prompt.as_deref().map(str::trim).unwrap_or("").is_empty()
    {
        eprintln!("hotpot: [usage] {usage} (--prompt requires a non-empty directive text)");
        std::process::exit(2);
    }
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
    let outcome = match seats::launch_seat(&home, &role, &agent, Path::new(&project), binding.clone()) {
        Ok(seats::LaunchOutcome::Launched) => {
            println!("seat: launched {agent} {role} terminal for project {slug}");
            seats::LaunchOutcome::Launched
        }
        Ok(seats::LaunchOutcome::Reused { address }) => {
            println!(
                "seat: reusing existing lead terminal {address} for project {slug}; \
                 no new terminal created"
            );
            seats::LaunchOutcome::Reused { address }
        }
        Err(err) => {
            eprintln!("hotpot: {err}");
            std::process::exit(1);
        }
    };
    // The initial directive (ticket nui/04): recorded durably, then delivered
    // into the seat's terminal best-effort — a failed delivery never fails the
    // launch, it is reported honestly instead.
    if let Some(prompt) = prompt.as_deref() {
        match seats::dispatch_prompt(&home, &slug, &role, &agent, &binding, &outcome, prompt) {
            Ok(dispatch) if dispatch.delivered => {
                let target = dispatch.target.as_deref().unwrap_or_default();
                println!("seat: directive delivered to {target} via hc send");
            }
            Ok(dispatch) => eprintln!(
                "seat: directive not delivered ({}); the directive stays recorded in \
                 the project ledger",
                dispatch.reason.as_deref().unwrap_or("hc send failed")
            ),
            Err(err) => {
                eprintln!("hotpot: {err}");
                std::process::exit(1);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    // A bare `hotpot` means `hotpot status`; `rest` is the subcommand's own
    // argv (empty for the bare form — `args[2..]` would panic on a 1-element
    // argv, which is exactly what the bare form has).
    let (sub, rest): (&str, &[String]) = match args.get(1) {
        Some(sub) => (sub.as_str(), &args[2..]),
        None => ("status", &[]),
    };
    match sub {
        "status" => cmd_status(rest),
        "inspect" => cmd_inspect(rest),
        "hook" => cmd_hook(rest),
        "mcp" => cmd_mcp(rest),
        "setup" => {
            if let Err(err) = cmd_setup(rest) {
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
            if let Err(err) = cmd_remove(rest) {
                eprintln!("hotpot: {err}");
                std::process::exit(1);
            }
        }
        "auth" => cmd_auth(rest),
        "profile" => cmd_profile(rest),
        "attention" => cmd_attention(rest),
        "seat" => cmd_seat(rest),
        "export" => cmd_export(rest),
        "gate-runner" => cmd_gate_runner(rest),
        "--help" | "-h" | "help" => {
            println!("hotpot 0.1.0 — Rust SLP verification engine & session inspector for Herdr");
            println!("Commands: status, inspect, hook, mcp, setup [--apply], verify, remove [--apply],");
            println!("           auth login <codex|claude|pi> <supervisor|lead|peer>,");
            println!("           profile set <codex|claude|pi> <supervisor|lead|peer> <model> --thinking <low|medium|high>,");
            println!("           attention feedback <att_id> <useful|noisy> [--workspace <path>] [--state-dir <dir>],");
            println!("           seat launch <supervisor|lead|peer> --agent <codex|claude|pi> --project <path> [--prompt <text>],");
            println!("           export <project> --out <path>");
        }
        // Fail closed like every other subcommand: an unknown name (a typo such
        // as `hotpot setu`) must not silently answer with the status JSON and
        // exit 0. A bare `hotpot` still means `hotpot status` through the
        // `None => ("status", &[])` arm above.
        _ => {
            eprintln!("hotpot: unknown command `{sub}` — try `hotpot --help`");
            std::process::exit(2);
        }
    }
}
