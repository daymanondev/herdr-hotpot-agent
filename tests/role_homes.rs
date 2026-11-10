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
