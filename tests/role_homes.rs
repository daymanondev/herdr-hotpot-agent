//! CLI-level integration tests for Role Home provisioning, role contracts, and seat login
//! (ticket 02 of the Hotpot Foundation spec).
//!
//! Seam (per `.scratch/hotpot-foundation/spec.md` "Testing Decisions → Highest-Level Testing
//! Seam"): invoke the `hotpot` binary with `HOTPOT_HOME` and `HOME` pointed at temp dirs and
//! fake vendor binaries on `PATH` that record their argv/env, then assert stdout/stderr text,
//! exit codes, and the file tree under the temp home. Zero network; the operator homes under
//! the fake `HOME` are read-only inputs.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Best-effort self-cleaning temp dir (the crate deliberately has no test-only dependencies).
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hotpot-rolehomes-{tag}-{}-{}",
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

/// All nine seats as `(agent, role)` pairs, in the spec's canonical order.
fn seats() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for agent in ["codex", "claude", "pi"] {
        for role in ["supervisor", "lead", "peer"] {
            out.push((agent, role));
        }
    }
    out
}

/// The parseable vendor config file each agent family must own inside its Role Home.
fn vendor_config_rel(agent: &str, role: &str) -> String {
    match agent {
        "codex" => format!("roles/codex/{role}/config.json"),
        "claude" => format!("roles/claude/{role}/mcp.json"),
        "pi" => format!("roles/pi/{role}/settings.json"),
        other => panic!("unknown agent {other}"),
    }
}

/// The agent's additive instruction artifact inside its Role Home (Codex carries the contract
/// inside `config.json`'s `developer_instructions`; Claude and Pi carry standalone files).
fn instruction_artifact_rel(agent: &str, role: &str) -> String {
    match agent {
        "codex" => format!("roles/codex/{role}/config.json"),
        "claude" => format!("roles/claude/{role}/CLAUDE.md"),
        "pi" => format!("roles/pi/{role}/APPEND_SYSTEM.md"),
        other => panic!("unknown agent {other}"),
    }
}

/// The config file that declares the seat's own credential and session-history paths
/// (Claude keeps MCP wiring in `mcp.json` and seat settings in `settings.json`).
fn declarations_config_rel(agent: &str, role: &str) -> String {
    match agent {
        "codex" => format!("roles/codex/{role}/config.json"),
        "claude" => format!("roles/claude/{role}/settings.json"),
        "pi" => format!("roles/pi/{role}/settings.json"),
        other => panic!("unknown agent {other}"),
    }
}

/// Minimal PATH so the real operator environment can never leak into these tests; fake vendor
/// and `hc` binaries live in a prepended temp bin dir.
fn sandbox_path(fake_bin: Option<&Path>) -> String {
    let base = "/usr/bin:/bin:/usr/sbin:/sbin".to_string();
    match fake_bin {
        Some(dir) => format!("{}:{base}", dir.display()),
        None => base,
    }
}

/// Plants byte-distinct sentinel credential fixtures in the fake operator homes.
fn plant_operator_sentinels(fake_home: &Path) -> Vec<PathBuf> {
    let sentinels = vec![
        fake_home.join(".codex/auth.json"),
        fake_home.join(".claude/.credentials.json"),
        fake_home.join(".pi/agent/auth.json"),
    ];
    for (i, path) in sentinels.iter().enumerate() {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("operator-secret-{i}-do-not-touch\n")).unwrap();
    }
    sentinels
}

fn assert_operator_sentinels_unchanged(sentinels: &[PathBuf]) {
    for (i, path) in sentinels.iter().enumerate() {
        let bytes = fs::read(path).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&bytes),
            format!("operator-secret-{i}-do-not-touch\n"),
            "operator home file {} was mutated by hotpot",
            path.display()
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn run_hotpot(
    home: &Path,
    fake_home: &Path,
    fake_bin: Option<&Path>,
    extra_env: &[(&str, &str)],
    args: &[&str],
) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hotpot"));
    cmd.args(args)
        .env("HOTPOT_HOME", home)
        .env("HOME", fake_home)
        .env("PATH", sandbox_path(fake_bin));
    for (key, value) in extra_env {
        cmd.env(key, value);
    }
    cmd.output().unwrap()
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn run_setup_apply(home: &Path, fake_home: &Path, extra_env: &[(&str, &str)]) -> Output {
    let out = run_hotpot(home, fake_home, None, extra_env, &["setup", "--apply"]);
    assert!(
        out.status.success(),
        "setup --apply must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    out
}

fn read_json(path: &Path) -> serde_json::Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("parse {} as JSON: {e}\n---\n{text}", path.display()))
}

/// Every path under `root`, relative to it (recursive).
fn relative_tree(root: &Path) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let rel = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            set.insert(rel);
            if path.is_dir() && !path.is_symlink() {
                stack.push(path);
            }
        }
    }
    set
}

/// The rendered contract observed from one seat's delivered artifacts (the instruction
/// artifact text; for Codex, the `developer_instructions` field of its config).
fn delivered_contract(home: &Path, agent: &str, role: &str) -> String {
    match agent {
        "codex" => read_json(&home.join(vendor_config_rel(agent, role)))
            .get("developer_instructions")
            .and_then(|v| v.as_str())
            .expect("codex config carries developer_instructions")
            .to_string(),
        _ => fs::read_to_string(home.join(instruction_artifact_rel(agent, role)))
            .unwrap_or_else(|e| panic!("read instruction artifact: {e}")),
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 1: nine Role Homes with parseable vendor configs and instruction
// artifacts, all printed by the preceding dry run.
// ---------------------------------------------------------------------------

#[test]
fn setup_dry_run_prints_every_role_home_artifact_path_and_mutates_nothing() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let sentinels = plant_operator_sentinels(fake_home.path());

    let out = run_hotpot(home.path(), fake_home.path(), None, &[], &["setup"]);
    assert!(
        out.status.success(),
        "dry-run must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);

    let mut expected_rel = BTreeSet::new();
    for (agent, role) in seats() {
        expected_rel.insert(format!("roles/{agent}/{role}"));
        expected_rel.insert(vendor_config_rel(agent, role));
        expected_rel.insert(instruction_artifact_rel(agent, role));
        if agent == "claude" {
            expected_rel.insert(format!("roles/claude/{role}/settings.json"));
        }
    }
    expected_rel.insert("AUTHENTICATION.md".to_string());
    for rel in &expected_rel {
        let abs = home.path().join(rel);
        let prefix = if rel.contains('.') { "file" } else { "dir" };
        assert!(
            stdout.contains(&format!("plan: {prefix} {}", abs.display())),
            "dry-run stdout must print planned {prefix} {rel}:\n{stdout}"
        );
    }

    // No mutation anywhere: the temp home stays empty and operator sentinels are untouched.
    assert!(
        relative_tree(home.path()).is_empty(),
        "dry-run must not create anything under HOTPOT_HOME"
    );
    assert_operator_sentinels_unchanged(&sentinels);
}

#[test]
fn setup_apply_materializes_parseable_vendor_configs_and_instruction_artifacts() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let sentinels = plant_operator_sentinels(fake_home.path());

    run_setup_apply(home.path(), fake_home.path(), &[]);

    for (agent, role) in seats() {
        let role_dir = home.path().join("roles").join(agent).join(role);
        assert!(role_dir.is_dir(), "Role Home missing: {}", role_dir.display());

        let config = read_json(&home.path().join(vendor_config_rel(agent, role)));
        let server = &config["mcpServers"]["hotpot"];
        assert_eq!(
            server["type"].as_str(),
            Some("stdio"),
            "{agent}/{role} config must wire the hotpot mcp server over stdio"
        );
        let args: Vec<&str> = server["args"]
            .as_array()
            .expect("hotpot mcp args array")
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            args.contains(&"mcp"),
            "{agent}/{role} mcp wiring must invoke the hotpot mcp subcommand: {args:?}"
        );
        let role_flag = args
            .iter()
            .position(|a| *a == "--role")
            .expect("--role flag in mcp wiring");
        assert_eq!(args[role_flag + 1], role, "mcp wiring must scope to role {role}");
        let corr_flag = args
            .iter()
            .position(|a| *a == "--correlation")
            .expect("--correlation flag in mcp wiring");
        let correlation = args[corr_flag + 1];
        assert!(
            correlation.contains(agent) && correlation.contains(role),
            "per-seat correlation id `{correlation}` must identify the {agent}/{role} seat"
        );
        assert!(
            server["command"].as_str().is_some_and(|c| c.contains("hotpot")),
            "mcp wiring must launch the hotpot binary, got {:?}",
            server["command"]
        );

        let artifact = home.path().join(instruction_artifact_rel(agent, role));
        assert!(
            artifact.exists(),
            "{agent}/{role} instruction artifact missing: {}",
            artifact.display()
        );
    }

    // Operator homes remain strictly read-only inputs.
    assert_operator_sentinels_unchanged(&sentinels);
}

// ---------------------------------------------------------------------------
// Acceptance criterion 2: additive delivery of the 3-layer role contracts.
// ---------------------------------------------------------------------------

#[test]
fn role_contracts_carry_all_layers_plus_role_body_in_every_delivery_channel() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    run_setup_apply(home.path(), fake_home.path(), &[]);

    for (_, role) in seats() {
        let contract = delivered_contract(home.path(), "claude", role);
        for layer in ["shared-authority", "shared-seat-identity", "challenge-signals"] {
            assert!(
                contract.contains(layer),
                "{role} contract must contain the {layer} layer:\n{contract}"
            );
        }
        assert!(
            contract.contains(&format!("role-{role}")),
            "{role} contract must contain its role-specific body marker:\n{contract}"
        );
    }

    // Claude: the append channel inside settings.json carries the same contract as CLAUDE.md.
    for (_, role) in seats() {
        let settings = read_json(&home.path().join(format!("roles/claude/{role}/settings.json")));
        let appended = settings["appendSystemPrompt"].as_str().unwrap_or_else(|| {
            panic!("claude {role} settings.json must carry the append-system-prompt channel")
        });
        assert_eq!(
            appended,
            delivered_contract(home.path(), "claude", role),
            "claude {role} append channel and CLAUDE.md must carry the identical contract"
        );
    }

    // The same role contract reaches every agent family for a given role.
    for (_, role) in seats() {
        let codex = delivered_contract(home.path(), "codex", role);
        let claude = delivered_contract(home.path(), "claude", role);
        let pi = delivered_contract(home.path(), "pi", role);
        assert_eq!(codex, claude, "{role} contract must be identical across codex/claude");
        assert_eq!(claude, pi, "{role} contract must be identical across claude/pi");
    }
}

#[test]
fn codex_config_is_additive_and_replaces_no_base_system_prompt_field() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    run_setup_apply(home.path(), fake_home.path(), &[]);

    // Fields that would *replace* the vendor's base system prompt (per the additivity law,
    // story 9). The exhaustive list is the test's independent source of truth: any key whose
    // job is to swap out the base prompt must be absent — only additive channels may exist.
    let replacement_keys = [
        "system_prompt",
        "systemPrompt",
        "instructions",
        "base_instructions",
        "model_instructions_file",
        "prompt",
    ];
    for (_, role) in seats() {
        let config = read_json(&home.path().join(format!("roles/codex/{role}/config.json")));
        let obj = config.as_object().expect("codex config is a JSON object");
        for key in replacement_keys {
            assert!(
                !obj.contains_key(key),
                "codex {role} config must not carry base-prompt replacement field `{key}`"
            );
        }
        assert!(
            obj.contains_key("developer_instructions"),
            "codex {role} config must carry the additive developer_instructions channel"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 3: per-seat file-backed credentials and session history
// (story 3).
// ---------------------------------------------------------------------------

#[test]
fn each_seat_declares_distinct_file_backed_credentials_inside_its_role_home() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    run_setup_apply(home.path(), fake_home.path(), &[]);

    let mut credential_paths: BTreeSet<String> = BTreeSet::new();
    let mut history_paths: BTreeSet<String> = BTreeSet::new();
    for (agent, role) in seats() {
        let config = read_json(&home.path().join(declarations_config_rel(agent, role)));
        let role_home = home.path().join("roles").join(agent).join(role);
        let credential = config["credentials_file"].as_str().unwrap_or_else(|| {
            panic!("{agent}/{role} config must declare its own credentials_file path")
        });
        let history = config["session_history_file"].as_str().unwrap_or_else(|| {
            panic!("{agent}/{role} config must declare its own session_history_file path")
        });

        assert!(
            Path::new(credential).starts_with(&role_home),
            "{agent}/{role} credential path {credential} must live inside its own Role Home"
        );
        assert!(
            Path::new(history).starts_with(&role_home),
            "{agent}/{role} session-history path {history} must live inside its own Role Home"
        );
        credential_paths.insert(credential.to_string());
        history_paths.insert(history.to_string());
    }

    assert_eq!(
        credential_paths.len(),
        9,
        "the nine credential paths must be pairwise distinct: {credential_paths:?}"
    );
    assert_eq!(
        history_paths.len(),
        9,
        "the nine session-history paths must be pairwise distinct: {history_paths:?}"
    );
}

#[test]
fn no_symlink_under_roles_ever_targets_an_operator_home_file() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let sentinels = plant_operator_sentinels(fake_home.path());
    run_setup_apply(home.path(), fake_home.path(), &[]);

    // Walk every entry under roles/; any symlink must not resolve into the fake operator
    // home (mutable operator credential files are never symlinked into a Role Home).
    let mut stack = vec![home.path().join("roles")];
    let mut symlink_count = 0usize;
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() && !path.is_symlink() {
                stack.push(path);
                continue;
            }
            if path.is_symlink() {
                symlink_count += 1;
                let target = fs::read_link(&path).unwrap();
                let resolved = if target.is_absolute() {
                    target.clone()
                } else {
                    path.parent().unwrap().join(target)
                };
                assert!(
                    !resolved.starts_with(fake_home.path()),
                    "symlink {} targets the operator home ({})",
                    path.display(),
                    resolved.display()
                );
            }
        }
    }
    // The provisioned roles/ tree is entirely regular files and directories.
    assert_eq!(
        symlink_count, 0,
        "setup must not plant any symlink under roles/"
    );
    assert_operator_sentinels_unchanged(&sentinels);
}

// ---------------------------------------------------------------------------
// Acceptance criterion 4: hotpot.json carries a deterministic contract digest.
// ---------------------------------------------------------------------------

/// Reads `role_contract_digest` out of a provisioned `hotpot.json`.
fn marker_digest(home: &Path) -> String {
    let marker = read_json(&home.join("hotpot.json"));
    assert_eq!(marker["marker"].as_str(), Some("hotpot"));
    marker["role_contract_digest"]
        .as_str()
        .expect("hotpot.json carries role_contract_digest")
        .to_string()
}

#[test]
fn hotpot_json_digest_is_deterministic_and_tweak_sensitive() {
    let home_a = TempDir::new("dig1");
    let home_b = TempDir::new("dig2");
    let fake_home = TempDir::new("fakehome");

    // Same contract input (identical built-in contracts) → identical digest, valid hex.
    run_setup_apply(home_a.path(), fake_home.path(), &[]);
    run_setup_apply(home_b.path(), fake_home.path(), &[]);
    let digest_a = marker_digest(home_a.path());
    let digest_b = marker_digest(home_b.path());
    assert_eq!(digest_a, digest_b, "same contract input must yield the same digest");
    assert!(
        !digest_a.is_empty() && digest_a.chars().all(|c| c.is_ascii_hexdigit()),
        "digest must be non-empty hex: {digest_a}"
    );

    // A tweaked contract body (via HOTPOT_CONTRACTS_DIR) → a different digest.
    let contracts_dir = TempDir::new("contracts");
    fs::write(
        contracts_dir.path().join("peer.md"),
        "## role-peer\n\nTweaked peer body: escalated challenge discipline for this test.\n",
    )
    .unwrap();
    let tweaked = TempDir::new("dig3");
    run_setup_apply(
        tweaked.path(),
        fake_home.path(),
        &[("HOTPOT_CONTRACTS_DIR", &contracts_dir.path().to_string_lossy())],
    );
    let digest_tweaked = marker_digest(tweaked.path());
    assert_ne!(
        digest_tweaked, digest_a,
        "a tweaked contract body must change the digest"
    );

    // The override replaces only the role body: shared layers are still delivered, and the
    // untweaked roles keep the baseline digest input (their artifacts are byte-identical).
    let peer_contract = delivered_contract(tweaked.path(), "pi", "peer");
    assert!(
        peer_contract.contains("Tweaked peer body"),
        "the overridden body must be delivered to the seat:\n{peer_contract}"
    );
    for layer in ["shared-authority", "shared-seat-identity", "challenge-signals"] {
        assert!(
            peer_contract.contains(layer),
            "the tweaked contract must still carry the {layer} layer:\n{peer_contract}"
        );
    }
    assert_eq!(
        delivered_contract(tweaked.path(), "codex", "supervisor"),
        delivered_contract(home_a.path(), "codex", "supervisor"),
        "roles without an override keep the built-in contract"
    );
}

#[test]
fn hotpot_json_records_runtime_opt_in_state() {
    let home = TempDir::new("optin");
    let fake_home = TempDir::new("fakehome");

    run_setup_apply(home.path(), fake_home.path(), &[]);
    let marker = read_json(&home.path().join("hotpot.json"));
    assert_eq!(
        marker["runtime_opt_in"].as_bool(),
        Some(false),
        "runtime coordination must be opt-in (off by default)"
    );

    let opted_in = TempDir::new("optin2");
    run_setup_apply(opted_in.path(), fake_home.path(), &[("HOTPOT_RUNTIME_OPT_IN", "1")]);
    let marker = read_json(&opted_in.path().join("hotpot.json"));
    assert_eq!(marker["runtime_opt_in"].as_bool(), Some(true));
}

// ---------------------------------------------------------------------------
// Acceptance criterion 5: AUTHENTICATION.md lists every seat's login command, no secrets.
// ---------------------------------------------------------------------------

#[test]
fn authentication_md_lists_one_login_command_per_seat_without_secrets() {
    let home = TempDir::new("authmd");
    let fake_home = TempDir::new("fakehome");
    plant_operator_sentinels(fake_home.path());
    run_setup_apply(home.path(), fake_home.path(), &[]);

    let text = fs::read_to_string(home.path().join("AUTHENTICATION.md"))
        .expect("AUTHENTICATION.md exists at the Hotpot Home root");

    let commands: BTreeSet<String> = text
        .lines()
        .map(|l| l.trim())
        .filter_map(|l| l.strip_prefix("hotpot auth login ").map(|rest| rest.to_string()))
        .map(|seat| format!("hotpot auth login {seat}"))
        .collect();
    let expected: BTreeSet<String> = seats()
        .into_iter()
        .map(|(agent, role)| format!("hotpot auth login {agent} {role}"))
        .collect();
    assert_eq!(
        commands, expected,
        "AUTHENTICATION.md must list exactly one login command per seat"
    );

    // No secrets: none of the planted operator credential fixture contents may appear.
    for i in 0..3 {
        assert!(
            !text.contains(&format!("operator-secret-{i}")),
            "AUTHENTICATION.md must not leak credential contents"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance criteria 6 & 7: `hotpot auth login` scoped vendor launch and
// invalid-seat diagnostics.
// ---------------------------------------------------------------------------

/// Writes a fake vendor binary that appends its argv, cwd, and sorted environment to `log`,
/// drops a marker file under its `$HOME`, and exits `status`.
fn write_fake_vendor(bin_dir: &Path, name: &str, log: &Path, status: i32) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\n\
         {{\n\
         \x20 printf 'argv: %s\\n' \"$*\"\n\
         \x20 printf 'cwd: %s\\n' \"$(pwd)\"\n\
         \x20 env | LC_ALL=C sort\n\
         }} >> {log}\n\
         mkdir -p \"$HOME/.seat-login-marker\"\n\
         printf 'marker\\n' > \"$HOME/.seat-login-marker/marker.txt\"\n\
         exit {status}\n",
        log = log.display(),
        status = status,
    );
    let shim = bin_dir.join(name);
    fs::write(&shim, script).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn auth_login_scopes_the_vendor_process_to_exactly_one_role_home() {
    let home = TempDir::new("login");
    let fake_home = TempDir::new("fakehome");
    let sentinels = plant_operator_sentinels(fake_home.path());
    let bin = TempDir::new("bin");
    let log_dir = TempDir::new("loginlog");
    let log = log_dir.path().join("pi-login.log");
    write_fake_vendor(bin.path(), "pi", &log, 0);
    run_setup_apply(home.path(), fake_home.path(), &[]);

    let out = run_hotpot(
        home.path(),
        fake_home.path(),
        Some(bin.path()),
        &[],
        &["auth", "login", "pi", "peer"],
    );
    assert!(
        out.status.success(),
        "auth login pi peer must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );

    let recorded = fs::read_to_string(&log).expect("fake pi binary must have been invoked");
    let role_home = home.path().join("roles/pi/peer");
    // `pwd` in the shim reports the physical path (/private/var/... on macOS), so compare
    // against the canonicalized Role Home spelling.
    let role_home_physical = fs::canonicalize(&role_home).unwrap_or_else(|_| role_home.clone());
    assert!(
        recorded.contains(&"argv: login".to_string()),
        "the vendor binary must be invoked as `login`:\n{recorded}"
    );
    assert!(
        recorded.contains(&format!("cwd: {}", role_home_physical.display())),
        "the vendor process must run with cwd scoped to the Role Home:\n{recorded}"
    );
    assert!(
        recorded.contains(&format!("HOME={}", role_home.display())),
        "the vendor process must see HOME scoped to the Role Home:\n{recorded}"
    );

    // The fake wrote its marker under its $HOME — proving that $HOME really was the Role Home.
    let marker = role_home.join(".seat-login-marker/marker.txt");
    assert!(
        marker.exists(),
        "the fake vendor's $HOME marker must land inside the Role Home: {}",
        marker.display()
    );

    // No operator-home paths anywhere in the recorded environment.
    for line in recorded.lines() {
        assert!(
            !line.contains(fake_home.path().to_string_lossy().as_ref()),
            "recorded vendor environment must contain no operator-home paths: {line}"
        );
    }

    // Exactly one seat is affected: no other Role Home received a marker.
    for (agent, role) in seats() {
        if (agent, role) != ("pi", "peer") {
            assert!(
                !home
                    .path()
                    .join(format!("roles/{agent}/{role}/.seat-login-marker"))
                    .exists(),
                "auth login must not touch the {agent}/{role} seat"
            );
        }
    }
    assert_operator_sentinels_unchanged(&sentinels);
}

#[test]
fn auth_login_propagates_vendor_failure_exit_status() {
    let home = TempDir::new("loginfail");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    let log_dir = TempDir::new("loginfaillog");
    write_fake_vendor(bin.path(), "codex", &log_dir.path().join("codex-login.log"), 3);
    run_setup_apply(home.path(), fake_home.path(), &[]);

    let out = run_hotpot(
        home.path(),
        fake_home.path(),
        Some(bin.path()),
        &[],
        &["auth", "login", "codex", "supervisor"],
    );
    assert_eq!(
        out.status.code(),
        Some(3),
        "a failing vendor login must surface its exit status"
    );
}

#[test]
fn auth_login_rejects_invalid_role_and_agent_on_stderr() {
    let home = TempDir::new("loginbad");
    let fake_home = TempDir::new("fakehome");
    run_setup_apply(home.path(), fake_home.path(), &[]);

    let out = run_hotpot(
        home.path(),
        fake_home.path(),
        None,
        &[],
        &["auth", "login", "codex", "wizard"],
    );
    assert!(
        !out.status.success(),
        "an invalid role must exit non-zero"
    );
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("wizard"),
        "the diagnostic must name the invalid role:\n{stderr}"
    );
    assert!(
        stderr.to_lowercase().contains("role"),
        "the diagnostic must say what was invalid:\n{stderr}"
    );

    let out = run_hotpot(
        home.path(),
        fake_home.path(),
        None,
        &[],
        &["auth", "login", "sonnet", "lead"],
    );
    assert!(
        !out.status.success(),
        "an invalid agent must exit non-zero"
    );
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("sonnet"),
        "the diagnostic must name the invalid agent:\n{stderr}"
    );
}

#[test]
fn auth_login_fails_closed_when_the_seat_or_vendor_is_missing() {
    let home = TempDir::new("loginmiss");
    let fake_home = TempDir::new("fakehome");

    // No setup --apply first: the Role Home does not exist yet.
    let out = run_hotpot(
        home.path(),
        fake_home.path(),
        None,
        &[],
        &["auth", "login", "pi", "peer"],
    );
    assert!(!out.status.success(), "login into an unprovisioned seat must fail");
    assert!(
        stderr_of(&out).to_lowercase().contains("setup"),
        "the diagnostic must point at `hotpot setup --apply`:\n{}",
        stderr_of(&out)
    );

    // Provisioned home, but no vendor binary on PATH.
    run_setup_apply(home.path(), fake_home.path(), &[]);
    let out = run_hotpot(
        home.path(),
        fake_home.path(),
        None,
        &[],
        &["auth", "login", "pi", "peer"],
    );
    assert!(!out.status.success(), "login without the vendor CLI must fail");
    assert!(
        stderr_of(&out).contains("pi"),
        "the diagnostic must name the missing vendor binary:\n{}",
        stderr_of(&out)
    );
}
