//! CLI-level integration tests for Capability Hygiene closures (ticket 03 of the Hotpot
//! Foundation spec): fail-closed multi-agent closures for Codex/Claude/Pi seats, operator MCP
//! inspection, and the `hotpot verify` subcommand.
//!
//! Seam (per `.scratch/hotpot-foundation/spec.md` "Testing Decisions → Highest-Level Testing
//! Seam"): invoke the `hotpot` binary with `HOTPOT_HOME` and `HOME` pointed at temp dirs, a
//! fixture operator home (`~/.codex`, `~/.claude`, `~/.pi/agent`), and fake vendor binaries on
//! `PATH` (the offline Pi RPC probe hits the fake binary, never the network), then assert exit
//! codes, stderr check names, config files under the temp home, and byte-unchanged operator
//! files. Zero network; operator homes are read-only inputs.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Best-effort self-cleaning temp dir (the crate deliberately has no test-only dependencies).
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hotpot-hygiene-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Independent sources of truth (spec ticket 03), deliberately written out as
// literals here instead of importing from `src/` so the tests can disagree.
// ---------------------------------------------------------------------------

/// The three roles every agent family is provisioned for.
const ROLES: [&str; 3] = ["supervisor", "lead", "peer"];

/// Claude's canonical native-orchestration deny list: `Task`, `Agent`, `Workflow`,
/// coordination, cron, and team tools (ticket 03 scope).
const CLAUDE_DENY_TOOLS: [&str; 6] = ["Task", "Agent", "Workflow", "Coordination", "Cron", "Team"];

/// The exact Pi launch args closing native extensions except the two builtins (ticket 03
/// scope): `--no-extensions --extension builtin:mcp --extension builtin:codemode --no-approve`.
const PI_LAUNCH_ARGS: [&str; 6] = [
    "--no-extensions",
    "--extension",
    "builtin:mcp",
    "--extension",
    "builtin:codemode",
    "--no-approve",
];

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Minimal PATH so the real operator environment can never leak into these tests; fake vendor
/// binaries live in a prepended temp bin dir.
fn sandbox_path(fake_bin: Option<&Path>) -> String {
    let base = "/usr/bin:/bin:/usr/sbin:/sbin".to_string();
    match fake_bin {
        Some(dir) => format!("{}:{base}", dir.display()),
        None => base,
    }
}

fn run_hotpot(
    home: &Path,
    fake_home: &Path,
    fake_bin: Option<&Path>,
    args: &[&str],
) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .args(args)
        .env("HOTPOT_HOME", home)
        .env("HOME", fake_home)
        .env("PATH", sandbox_path(fake_bin))
        .output()
        .unwrap()
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Runs a clean `setup --apply` and asserts it exits 0.
fn apply_clean(home: &Path, fake_home: &Path, fake_bin: Option<&Path>) {
    let out = run_hotpot(home, fake_home, fake_bin, &["setup", "--apply"]);
    assert!(
        out.status.success(),
        "clean setup --apply must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
}

fn read_json(path: &Path) -> serde_json::Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse {} as JSON: {e}\n---\n{text}", path.display()))
}

/// Writes a fake `pi` binary that answers the offline `/mcp` RPC probe with the given
/// extension list and exit status; the probe never touches the network.
fn write_fake_pi(bin_dir: &Path, extensions: &[&str], status: i32) {
    fs::create_dir_all(bin_dir).unwrap();
    let list = extensions
        .iter()
        .map(|e| format!("\"{e}\""))
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "#!/bin/sh\nprintf '{{\"path\":\"/mcp\",\"extensions\":[{list}]}}\\n'\nexit {status}\n"
    );
    let shim = bin_dir.join("pi");
    fs::write(&shim, script).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Full byte snapshot of every file under `root` (recursive) for "wrote nothing" assertions.
/// Symlinks (the Lead `skills` aggregates and the onboarding wiring planned by the skill
/// projection) are recorded by their link text and never followed, so a retargeted or
/// replaced link still changes the snapshot.
fn snapshot_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                stack.push(path);
                continue;
            }
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            let bytes = if path.is_symlink() {
                fs::read_link(&path)
                    .unwrap()
                    .as_os_str()
                    .as_bytes()
                    .to_vec()
            } else {
                fs::read(&path).unwrap()
            };
            out.insert(rel, bytes);
        }
    }
    out
}

/// Recursively asserts that no object anywhere under `value` carries `multi_agent_version`.
fn assert_no_multi_agent_version(value: &serde_json::Value, where_: &str) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, inner) in map {
                assert_ne!(
                    key, "multi_agent_version",
                    "{where_} still carries the multi_agent_version key"
                );
                assert_no_multi_agent_version(inner, where_);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                assert_no_multi_agent_version(item, where_);
            }
        }
        _ => {}
    }
}

/// The `features` scope of a Codex config scope must disable every `multi_agent*` flag, and at
/// least one such flag must exist so the closure is verifiably pinned.
fn assert_multi_agent_flags_disabled(features: &serde_json::Value, where_: &str) {
    let obj = features
        .as_object()
        .unwrap_or_else(|| panic!("{where_} features scope must be a JSON object"));
    let flags: Vec<(&String, &serde_json::Value)> = obj
        .iter()
        .filter(|(k, _)| k.starts_with("multi_agent"))
        .collect();
    assert!(
        !flags.is_empty(),
        "{where_} must pin at least one features.multi_agent* flag to false"
    );
    for (key, value) in flags {
        assert_eq!(
            value, &serde_json::Value::Bool(false),
            "{where_} features.{key} must be disabled"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 3 (generation half): Claude seats pin the full deny list and the
// Agent View / dynamic workflows / claude.ai connectors disable flags.
// ---------------------------------------------------------------------------

#[test]
fn setup_apply_pins_the_claude_deny_list_and_disable_flags_in_every_seat() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);

    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    for role in ROLES {
        let role_dir = home.path().join("roles/claude").join(role);

        // settings.json: the full canonical deny list plus the three disable flags.
        let settings = read_json(&role_dir.join("settings.json"));
        let denied: Vec<&str> = settings["disallowedTools"]
            .as_array()
            .expect("disallowedTools array in settings.json")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for tool in CLAUDE_DENY_TOOLS {
            assert!(
                denied.contains(&tool),
                "claude/{role} settings.json must deny the native-orchestration tool {tool}: {denied:?}"
            );
        }
        for flag in ["enableAgentView", "enableDynamicWorkflows", "enableClaudeAiConnectors"] {
            assert_eq!(
                settings[flag].as_bool(),
                Some(false),
                "claude/{role} settings.json must carry {flag}: false"
            );
        }

        // The launch flags carry the same deny entries, one `--disallow-tool <Tool>` pair
        // per tool.
        let launch = read_json(&role_dir.join("launch.json"));
        let args: Vec<&str> = launch["args"]
            .as_array()
            .expect("args array in launch.json")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for tool in CLAUDE_DENY_TOOLS {
            let pair_position = args
                .windows(2)
                .position(|w| w[0] == "--disallow-tool" && w[1] == tool);
            assert!(
                pair_position.is_some(),
                "claude/{role} launch flags must carry --disallow-tool {tool}: {args:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 4 (generation half): Pi seats launch with exactly the closed
// extension args, recorded in each Role Home launch config.
// ---------------------------------------------------------------------------

#[test]
fn setup_apply_records_exactly_the_closed_pi_launch_args_in_every_seat() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);

    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    for role in ROLES {
        let launch = read_json(&home.path().join(format!("roles/pi/{role}/launch.json")));
        let args: Vec<&str> = launch["args"]
            .as_array()
            .expect("args array in pi launch.json")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            args,
            PI_LAUNCH_ARGS.to_vec(),
            "pi/{role} launch config must carry exactly the closed extension args"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 1 (verify half): `hotpot verify` exits 0 with a per-check report when
// every closure holds, and fails closed on an unprovisioned home.
// ---------------------------------------------------------------------------

/// The full set of hygiene check names `hotpot verify` reports (one line per check instance;
/// seat checks repeat per Role Home). Independent source of truth for the report assertions.
const CHECK_NAMES: [&str; 7] = [
    "codex-catalog-scrubbed",
    "codex-agents-disabled",
    "claude-deny-list",
    "claude-disable-flags",
    "pi-launch-args",
    "pi-mcp-probe",
    "operator-mcp-clean",
];

#[test]
fn verify_exits_zero_with_a_per_check_report_on_a_clean_home() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    plant_operator_codex_catalog(fake_home.path());
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);

    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &["verify"]);
    assert!(
        out.status.success(),
        "verify must exit 0 on a cleanly provisioned home\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    for name in CHECK_NAMES {
        assert!(
            stdout.contains(&format!("verify: ok {name} ")),
            "verify report must carry an ok line for the {name} check:\n{stdout}"
        );
    }
    assert!(
        stderr_of(&out).trim().is_empty(),
        "a green verify must print nothing to stderr:\n{}",
        stderr_of(&out)
    );
}

#[test]
fn verify_fails_closed_on_an_unprovisioned_home() {
    let home = TempDir::new("empty");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);

    // No `setup --apply` first: nothing is verifiably closed, so verify must fail naming
    // the seat checks whose closure files are missing.
    let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &["verify"]);
    assert!(
        !out.status.success(),
        "verify on an unprovisioned home must exit non-zero"
    );
    let stderr = stderr_of(&out);
    for name in [
        "codex-catalog-scrubbed",
        "codex-agents-disabled",
        "claude-deny-list",
        "pi-launch-args",
    ] {
        assert!(
            stderr.contains(name),
            "verify must name the failing {name} check:\n{stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 2: sabotaged Codex fixtures fail setup and verify closed, with the
// failing check name on stderr and nothing applied.
// ---------------------------------------------------------------------------

/// Reads a JSON file, applies `edit`, and writes it back pretty-printed.
fn edit_json(path: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut value = read_json(path);
    edit(&mut value);
    fs::write(path, serde_json::to_string_pretty(&value).unwrap() + "\n").unwrap();
}

/// Provisions a clean home, sabotages one seat file, then asserts that `setup` (dry-run and
/// `--apply`) and `verify` all exit non-zero with every `expected` substring (the check name
/// first, plus whatever the check names — a removed flag, a server name) on stderr, and that
/// the failed `setup --apply` wrote nothing (the tree is byte-identical, sabotage included).
fn assert_sabotage_fails_setup_and_verify(
    tag: &str,
    expected: &[&str],
    sabotage: impl FnOnce(&Path),
) {
    let home = TempDir::new(tag);
    let fake_home = TempDir::new("fakehome");
    plant_operator_codex_catalog(fake_home.path());
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    sabotage(home.path());
    let before = snapshot_tree(home.path());

    for args in [&["setup"][..], &["setup", "--apply"][..]] {
        let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), args);
        assert!(
            !out.status.success(),
            "`hotpot {}` must exit non-zero on the sabotaged {tag} fixture",
            args.join(" ")
        );
        let stderr = stderr_of(&out);
        for expected in expected {
            assert!(
                stderr.contains(expected),
                "`hotpot {}` stderr must contain {expected}:\n{stderr}",
                args.join(" ")
            );
        }
    }
    assert_eq!(
        snapshot_tree(home.path()),
        before,
        "the failed setup --apply must write nothing"
    );

    let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &["verify"]);
    assert!(!out.status.success(), "verify must exit non-zero on {tag}");
    let stderr = stderr_of(&out);
    for expected in expected {
        assert!(
            stderr.contains(expected),
            "verify stderr must contain {expected}:\n{stderr}"
        );
    }
}

#[test]
fn sabotaged_codex_catalog_fails_setup_and_verify_naming_the_check() {
    assert_sabotage_fails_setup_and_verify("catalog", &["codex-catalog-scrubbed"], |home| {
        edit_json(&home.join("roles/codex/lead/models.json"), |catalog| {
            catalog["models"][0]["multi_agent_version"] = serde_json::json!(2);
        });
    });
}

#[test]
fn reenabled_codex_agents_in_either_scope_fails_setup_and_verify() {
    // Top-level `agents.enabled = true`.
    assert_sabotage_fails_setup_and_verify("agents-top", &["codex-agents-disabled"], |home| {
        edit_json(&home.join("roles/codex/supervisor/config.json"), |config| {
            config["agents"]["enabled"] = serde_json::json!(true);
        });
    });
    // A re-enabled extra `features.multi_agent*` flag in the top-level scope.
    assert_sabotage_fails_setup_and_verify("flag-top", &["codex-agents-disabled"], |home| {
        edit_json(&home.join("roles/codex/peer/config.json"), |config| {
            config["features"]["multi_agent_tools"] = serde_json::json!(true);
        });
    });
    // The active profile scope re-enables agents.
    assert_sabotage_fails_setup_and_verify("agents-profile", &["codex-agents-disabled"], |home| {
        edit_json(&home.join("roles/codex/lead/config.json"), |config| {
            let active = config["profile"].as_str().unwrap().to_string();
            config["profiles"][&active]["agents"]["enabled"] = serde_json::json!(true);
        });
    });
}

#[test]
fn removing_any_claude_flag_fails_setup_and_verify_naming_it() {
    // A deny entry removed from `settings.json`.
    assert_sabotage_fails_setup_and_verify(
        "claude-entry",
        &["claude-deny-list", "Cron"],
        |home| {
            edit_json(&home.join("roles/claude/supervisor/settings.json"), |settings| {
                let denied: Vec<serde_json::Value> = settings["disallowedTools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|tool| tool.as_str() != Some("Cron"))
                    .cloned()
                    .collect();
                settings["disallowedTools"] = serde_json::Value::Array(denied);
            });
        },
    );
    // The matching launch flag removed from `launch.json`.
    assert_sabotage_fails_setup_and_verify(
        "claude-flag",
        &["claude-deny-list", "Team"],
        |home| {
            edit_json(&home.join("roles/claude/peer/launch.json"), |launch| {
                let args: Vec<serde_json::Value> = launch["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .enumerate()
                    .filter(|(i, arg)| !(arg.as_str().unwrap_or("") == "Team" && i % 2 == 1))
                    .map(|(_, arg)| arg.clone())
                    .collect();
                launch["args"] = serde_json::Value::Array(args);
            });
        },
    );
    // A disable flag flipped back on.
    assert_sabotage_fails_setup_and_verify(
        "claude-agentview",
        &["claude-disable-flags", "enableAgentView"],
        |home| {
            edit_json(&home.join("roles/claude/lead/settings.json"), |settings| {
                settings["enableAgentView"] = serde_json::json!(true);
            });
        },
    );
}

#[test]
fn unparseable_operator_catalog_fails_setup_closed_before_writing() {
    let home = TempDir::new("badcatalog");
    let fake_home = TempDir::new("fakehome");
    let path = fake_home.path().join(".codex/models.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{ not json ]").unwrap();

    for args in [&["setup"][..], &["setup", "--apply"][..]] {
        let out = run_hotpot(home.path(), fake_home.path(), None, args);
        assert!(
            !out.status.success(),
            "`hotpot {}` must fail closed when the operator catalog cannot be produced scrubbed",
            args.join(" ")
        );
        let stderr = stderr_of(&out);
        assert!(
            stderr.contains("codex-catalog-scrub"),
            "the diagnostic must carry the codex-catalog-scrub failure code:\n{stderr}"
        );
    }
    assert!(
        snapshot_tree(home.path()).is_empty(),
        "nothing may be written under Hotpot Home when the catalog cannot be scrubbed"
    );
    // The operator catalog is a read-only input even on failure.
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json ]");
}

// ---------------------------------------------------------------------------
// Acceptance criterion 4 (sabotage half): tampered Pi launch args and an RPC probe
// resolving /mcp beyond builtin:mcp both fail closed.
// ---------------------------------------------------------------------------

#[test]
fn tampered_pi_launch_args_fail_setup_and_verify() {
    // `--no-approve` removed from one seat's launch config.
    assert_sabotage_fails_setup_and_verify(
        "pi-launch",
        &["pi-launch-args", "--no-approve"],
        |home| {
            edit_json(&home.join("roles/pi/lead/launch.json"), |launch| {
                let args: Vec<serde_json::Value> = launch["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|arg| arg.as_str() != Some("--no-approve"))
                    .cloned()
                    .collect();
                launch["args"] = serde_json::Value::Array(args);
            });
        },
    );
}

#[test]
fn pi_probe_resolving_mcp_to_an_extra_extension_fails_verify() {
    let home = TempDir::new("probe");
    let fake_home = TempDir::new("fakehome");
    plant_operator_codex_catalog(fake_home.path());
    let good_bin = TempDir::new("goodbin");
    write_fake_pi(good_bin.path(), &["builtin:mcp"], 0);
    apply_clean(home.path(), fake_home.path(), Some(good_bin.path()));

    // The offline RPC probe hits the fake `pi` binary; one reporting /mcp with an extra
    // extension must fail verify naming the probe check and the extension.
    let bad_bin = TempDir::new("badbin");
    write_fake_pi(bad_bin.path(), &["builtin:mcp", "community:orchestrator"], 0);
    let out = run_hotpot(home.path(), fake_home.path(), Some(bad_bin.path()), &["verify"]);
    assert!(
        !out.status.success(),
        "verify must exit non-zero when /mcp resolves beyond builtin:mcp"
    );
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("pi-mcp-probe"),
        "verify stderr must name the failing pi-mcp-probe check:\n{stderr}"
    );
    assert!(
        stderr.contains("community:orchestrator"),
        "the probe failure must name the extra extension:\n{stderr}"
    );

    // The same home verifies green against a pi resolving strictly to builtin:mcp.
    let out = run_hotpot(home.path(), fake_home.path(), Some(good_bin.path()), &["verify"]);
    assert!(
        out.status.success(),
        "verify must exit 0 when /mcp resolves strictly to builtin:mcp\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
}

#[test]
fn verify_fails_closed_when_pi_is_unavailable_for_the_probe() {
    let home = TempDir::new("nopi");
    let fake_home = TempDir::new("fakehome");
    plant_operator_codex_catalog(fake_home.path());
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    // No `pi` on PATH: the /mcp resolution cannot be verified, so verify fails closed.
    let out = run_hotpot(home.path(), fake_home.path(), None, &["verify"]);
    assert!(
        !out.status.success(),
        "verify must exit non-zero when the pi RPC probe cannot run"
    );
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("pi-mcp-probe"),
        "verify stderr must name the pi-mcp-probe check:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 5: an operator MCP server recognizably wired to hotpot/paseo fails
// setup and verify before applying, naming the server, without mutating the operator file.
// ---------------------------------------------------------------------------

/// Plants an operator MCP config at `rel` (relative to the fake HOME) with the given
/// `mcpServers` JSON text and returns its path.
fn plant_operator_mcp(fake_home: &Path, rel: &str, servers: &str) -> PathBuf {
    let path = fake_home.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, format!("{{\"mcpServers\": {servers}}}\n")).unwrap();
    path
}

#[test]
fn operator_mcp_hotpot_server_fails_setup_and_verify_without_mutation() {
    let home = TempDir::new("opmcp");
    let fake_home = TempDir::new("fakehome");
    let mcp_json = plant_operator_mcp(
        fake_home.path(),
        ".codex/mcp.json",
        r#"{"helper": {"type": "stdio", "command": "hotpot", "args": ["mcp"]}}"#,
    );
    let bytes_before = fs::read(&mcp_json).unwrap();

    // setup fails before applying — in dry-run and apply alike — naming the check and the
    // recognizable server; nothing lands under Hotpot Home.
    for args in [&["setup"][..], &["setup", "--apply"][..]] {
        let out = run_hotpot(home.path(), fake_home.path(), None, args);
        assert!(
            !out.status.success(),
            "`hotpot {}` must fail on an operator hotpot MCP server",
            args.join(" ")
        );
        let stderr = stderr_of(&out);
        for expected in ["operator-mcp-clean", "helper"] {
            assert!(
                stderr.contains(expected),
                "`hotpot {}` stderr must name {expected}:\n{stderr}",
                args.join(" ")
            );
        }
    }
    assert!(
        snapshot_tree(home.path()).is_empty(),
        "setup must fail before applying anything"
    );

    let out = run_hotpot(home.path(), fake_home.path(), None, &["verify"]);
    assert!(!out.status.success(), "verify must fail on the operator backdoor");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("operator-mcp-clean") && stderr.contains("helper"),
        "verify stderr must name the check and the server:\n{stderr}"
    );

    // The operator MCP config is inspected read-only: byte-identical afterwards.
    assert_eq!(fs::read(&mcp_json).unwrap(), bytes_before);
}

#[test]
fn operator_mcp_paseo_named_server_fails_setup_and_verify() {
    let home = TempDir::new("oppaseo");
    let fake_home = TempDir::new("fakehome");
    plant_operator_mcp(
        fake_home.path(),
        ".claude/mcp.json",
        r#"{"paseo-room": {"type": "stdio", "command": "node", "args": ["server.js"]}}"#,
    );

    let out = run_hotpot(home.path(), fake_home.path(), None, &["setup", "--apply"]);
    assert!(!out.status.success(), "setup must fail on a paseo-named server");
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("operator-mcp-clean") && stderr.contains("paseo-room"),
        "the diagnostic must name the check and the paseo server:\n{stderr}"
    );
    assert!(snapshot_tree(home.path()).is_empty());

    let out = run_hotpot(home.path(), fake_home.path(), None, &["verify"]);
    assert!(!out.status.success());
    assert!(
        stderr_of(&out).contains("paseo-room"),
        "verify must name the paseo server:\n{}",
        stderr_of(&out)
    );
}

#[test]
fn operator_mcp_config_with_only_foreign_servers_stays_green() {
    let home = TempDir::new("opok");
    let fake_home = TempDir::new("fakehome");
    plant_operator_mcp(
        fake_home.path(),
        ".pi/agent/mcp.json",
        r#"{"weather": {"type": "stdio", "command": "uvx", "args": ["mcp-weather"]}}"#,
    );
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);

    // A foreign server is none of hotpot's business: setup applies and verify stays green.
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));
    let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &["verify"]);
    assert!(
        out.status.success(),
        "verify must stay green on foreign operator MCP servers\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 1: clean setup writes closed Codex seat configs and verify exits 0.
// ---------------------------------------------------------------------------

/// Operator catalog fixture carrying `multi_agent_version` (the realistic operator state the
/// scrubbed projection must clean): two models plus a nested colluding entry.
fn plant_operator_codex_catalog(fake_home: &Path) {
    let catalog = r#"{
  "version": 4,
  "models": [
    {"id": "gpt-5.4-codex", "label": "GPT 5.4 Codex", "multi_agent_version": 3},
    {"id": "gpt-5.4-mini", "label": "GPT 5.4 Mini"}
  ],
  "experimental": {"collusion": {"multi_agent_version": 1}}
}
"#;
    let path = fake_home.join(".codex/models.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, catalog).unwrap();
}

#[test]
fn setup_apply_scrubs_the_operator_catalog_and_disables_codex_multi_agent_in_both_scopes() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    plant_operator_codex_catalog(fake_home.path());
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);

    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    for role in ROLES {
        // The scrubbed catalog projection: parseable, content preserved, and no
        // multi_agent_version key anywhere.
        let catalog_path = home.path().join(format!("roles/codex/{role}/models.json"));
        let catalog = read_json(&catalog_path);
        assert_no_multi_agent_version(&catalog, &catalog_path.display().to_string());
        let ids: Vec<&str> = catalog["models"]
            .as_array()
            .expect("models array survives the scrub")
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec!["gpt-5.4-codex", "gpt-5.4-mini"],
            "the scrub must preserve the operator's models, only removing the closure key"
        );
        assert_eq!(
            catalog["version"].as_i64(),
            Some(4),
            "unrelated catalog fields survive the scrub"
        );

        // The seat config closes agents and every features.multi_agent* flag in both the
        // top-level scope and the active profile scope.
        let config_path = home.path().join(format!("roles/codex/{role}/config.json"));
        let config = read_json(&config_path);
        assert_eq!(
            config["agents"]["enabled"].as_bool(),
            Some(false),
            "top-level agents.enabled must be false for codex/{role}"
        );
        assert_multi_agent_flags_disabled(&config["features"], "top-level scope");

        let active = config["profile"]
            .as_str()
            .unwrap_or_else(|| panic!("codex/{role} config must declare its active profile"))
            .to_string();
        let profile = &config["profiles"][&active];
        assert!(
            profile.is_object(),
            "codex/{role} active profile `{active}` must exist in profiles"
        );
        assert_eq!(
            profile["agents"]["enabled"].as_bool(),
            Some(false),
            "profile `{active}` agents.enabled must be false for codex/{role}"
        );
        assert_multi_agent_flags_disabled(&profile["features"], &format!("profile `{active}`"));
    }
}
