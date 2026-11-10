//! CLI-level integration tests for the Hotpot provisioner core (`hotpot setup` / `hotpot remove`).
//!
//! Seam (per `.scratch/hotpot-foundation/spec.md` "Testing Decisions → Highest-Level Testing
//! Seam"): invoke the `hotpot` binary with `HOTPOT_HOME` and `HOME` pointed at temp dirs and a
//! fake `hc` shim on PATH, then assert stdout/stderr text, exit codes, and the file tree under
//! the temp home. Zero network; the operator homes under the fake `HOME` are read-only inputs.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
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
            "hotpot-provisioner-{tag}-{}-{}",
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

/// Expected skeleton of the base Hotpot Home plan over an empty operator home, derived from
/// the spec's Provisioner Layer artifact list (`roles/<codex|claude|pi>/<supervisor|lead|peer>/`
/// with per-vendor configs and instruction artifacts, `skills/` with the built-in
/// `hotpot-project-onboarding` skill, `skill-projections/<agent>/lead/` aggregates wired into
/// each Lead's `skills` symlink, the `skill-projections/manifest.json` fence source,
/// `hotpot.json` marker, `AUTHENTICATION.md`) — the independent source of truth for the
/// dry-run assertions below.
///
/// Returns `relative path -> plan kind` (`dir` | `file` | `link`).
fn base_plan_items() -> BTreeMap<String, &'static str> {
    let mut items = BTreeMap::new();
    for agent in ["codex", "claude", "pi"] {
        for role in ["supervisor", "lead", "peer"] {
            items.insert(format!("roles/{agent}/{role}"), "dir");
            match agent {
                "codex" => {
                    items.insert(format!("roles/codex/{role}/config.json"), "file");
                }
                "claude" => {
                    items.insert(format!("roles/claude/{role}/mcp.json"), "file");
                    items.insert(format!("roles/claude/{role}/settings.json"), "file");
                    items.insert(format!("roles/claude/{role}/CLAUDE.md"), "file");
                }
                "pi" => {
                    items.insert(format!("roles/pi/{role}/settings.json"), "file");
                    items.insert(format!("roles/pi/{role}/APPEND_SYSTEM.md"), "file");
                }
                other => panic!("unknown agent {other}"),
            }
        }
        items.insert(format!("skill-projections/{agent}/lead"), "dir");
        // Lead skills wiring: the seat's skills directory points at the managed aggregate.
        items.insert(format!("roles/{agent}/lead/skills"), "link");
        // The aggregate always carries the built-in onboarding skill (symlinked to the
        // canonical asset under `skills/`).
        items.insert(
            format!("skill-projections/{agent}/lead/hotpot-project-onboarding"),
            "link",
        );
        items.insert(format!("roles/{agent}/peer/skills"), "dir");
    }
    items.insert("skills".to_string(), "dir");
    items.insert(
        "skills/hotpot-project-onboarding/SKILL.md".to_string(),
        "file",
    );
    items.insert("skill-projections/manifest.json".to_string(), "file");
    items.insert("hotpot.json".to_string(), "file");
    items.insert("AUTHENTICATION.md".to_string(), "file");
    items
}

/// Minimal PATH so the real operator environment (a possibly installed real `hc`) can never
/// leak into these tests; the fake `hc` bin dir is prepended when the test wants one.
fn sandbox_path(fake_bin: Option<&Path>) -> String {
    let base = "/usr/bin:/bin:/usr/sbin:/sbin".to_string();
    match fake_bin {
        Some(dir) => format!("{}:{base}", dir.display()),
        None => base,
    }
}

/// Writes a fake `hc` shim into `bin_dir` that appends its argv to `log` and exits 0.
fn write_fake_hc(bin_dir: &Path, log: &Path) {
    fs::create_dir_all(bin_dir).unwrap();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {}\nexit 0\n",
        log.display()
    );
    let shim = bin_dir.join("hc");
    fs::write(&shim, script).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
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

/// Runs `hotpot` with both `HOTPOT_HOME` and `HOME` unset (home-unresolvable failure mode).
fn run_hotpot_without_home_env(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .args(args)
        .env_remove("HOTPOT_HOME")
        .env_remove("HOME")
        .env("PATH", sandbox_path(None))
        .output()
        .unwrap()
}

fn stdout_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Every path under `root`, relative to it (recursive).
fn relative_tree(root: &Path) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            set.insert(rel);
            if path.is_dir() && !path.is_symlink() {
                stack.push(path);
            }
        }
    }
    set
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

#[test]
fn setup_dry_run_prints_every_planned_path_and_mutates_nothing() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let sentinels = plant_operator_sentinels(fake_home.path());

    let out = run_hotpot(home.path(), fake_home.path(), None, &["setup"]);

    assert!(
        out.status.success(),
        "dry-run must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    for (rel, kind) in base_plan_items() {
        let abs = home.path().join(&rel);
        assert!(
            stdout.contains(&format!("plan: {kind} {}", abs.display())),
            "dry-run stdout must print planned {kind} {rel}:\n{stdout}"
        );
    }
    let plan_line_count = stdout
        .lines()
        .filter(|l| l.starts_with("plan: "))
        .count();
    assert_eq!(
        plan_line_count,
        base_plan_items().len(),
        "dry-run must print exactly the base plan items, no more:\n{stdout}"
    );
    assert!(
        stdout.to_lowercase().contains("dry-run"),
        "dry-run output must say it did not apply anything:\n{stdout}"
    );

    // No mutation anywhere: the temp home stays empty and operator sentinels are untouched.
    assert!(
        relative_tree(home.path()).is_empty(),
        "dry-run must not create anything under HOTPOT_HOME"
    );
    assert_operator_sentinels_unchanged(&sentinels);
}

#[test]
fn setup_dry_run_prints_hc_profile_change_when_fake_hc_is_on_path() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    let hclog_dir = TempDir::new("hclog");
    let hc_log = hclog_dir.path().join("hc.log");
    write_fake_hc(bin.path(), &hc_log);

    let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &["setup"]);

    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains("plan: hc profile set"),
        "dry-run must print the planned hc profile change when hc is on PATH:\n{stdout}"
    );
    // Dry-run must not invoke hc at all.
    assert!(
        !hc_log.exists(),
        "dry-run must not invoke hc; log was written: {:?}",
        fs::read_to_string(&hc_log).unwrap_or_default()
    );
    assert!(relative_tree(home.path()).is_empty());
}

/// Parses the absolute `plan: dir|file|link <path>` entries out of a dry-run stdout
/// (`link` lines carry ` <path> -> <target>`; only the path is kept).
fn planned_paths_from_dry_run(stdout: &str) -> Vec<PathBuf> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("plan: "))
        .filter_map(|rest| {
            let (kind, path) = rest.split_once(' ')?;
            let path = path.split_once(" -> ").map(|(p, _)| p).unwrap_or(path);
            matches!(kind, "dir" | "file" | "link").then(|| PathBuf::from(path))
        })
        .collect()
}

#[test]
fn setup_apply_materializes_exactly_the_planned_set() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let sentinels = plant_operator_sentinels(fake_home.path());

    let dry = run_hotpot(home.path(), fake_home.path(), None, &["setup"]);
    assert!(dry.status.success());
    let planned = planned_paths_from_dry_run(&stdout_of(&dry));
    assert!(!planned.is_empty());

    let out = run_hotpot(home.path(), fake_home.path(), None, &["setup", "--apply"]);
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );

    // Exactly the planned set: every planned path (and only the ancestors it implies)
    // exists under the temp home, and nothing else does.
    let mut expected = BTreeSet::new();
    for path in &planned {
        let mut rel = path.strip_prefix(home.path()).unwrap().to_path_buf();
        while !rel.as_os_str().is_empty() {
            expected.insert(rel.to_string_lossy().into_owned());
            if !rel.pop() {
                break;
            }
        }
    }
    assert_eq!(
        relative_tree(home.path()),
        expected,
        "apply must materialize exactly the planned paths"
    );
    for path in &planned {
        assert!(path.exists(), "planned path missing after apply: {path:?}");
    }

    // Operator homes are strictly read-only inputs, and the default ~/.hotpot under the
    // fake HOME must never be created while HOTPOT_HOME points elsewhere.
    assert_operator_sentinels_unchanged(&sentinels);
    assert!(
        !fake_home.path().join(".hotpot").exists(),
        "nothing may be written under the default ~/.hotpot when HOTPOT_HOME is set"
    );
}

#[test]
fn setup_apply_invokes_hc_with_the_planned_profile_change() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    let hclog_dir = TempDir::new("hclog");
    let hc_log = hclog_dir.path().join("hc.log");
    write_fake_hc(bin.path(), &hc_log);

    let out = run_hotpot(
        home.path(),
        fake_home.path(),
        Some(bin.path()),
        &["setup", "--apply"],
    );
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );

    let log = fs::read_to_string(&hc_log).unwrap_or_default();
    assert_eq!(
        log,
        "profile set hotpot hotpot.v1 true\n",
        "apply must invoke hc with exactly the planned profile change"
    );
    assert!(home.path().join("hotpot.json").exists());
}

#[test]
fn setup_apply_preserves_preexisting_runtime_state() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let ledger = home.path().join("runtime/v1/ledger.jsonl");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    fs::write(&ledger, "{\"seq\":1,\"type\":\"room_opened\"}\n").unwrap();

    let dry = run_hotpot(home.path(), fake_home.path(), None, &["setup"]);
    assert!(dry.status.success());
    let stdout = stdout_of(&dry);
    assert!(
        !stdout.contains("runtime"),
        "runtime/ is preserve-only and must never appear in the plan:\n{stdout}"
    );

    let out = run_hotpot(home.path(), fake_home.path(), None, &["setup", "--apply"]);
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );

    let bytes = fs::read(&ledger).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&bytes),
        "{\"seq\":1,\"type\":\"room_opened\"}\n",
        "a pre-existing runtime/v1/ledger.jsonl must survive setup --apply byte-identically"
    );
}

#[test]
fn remove_dry_run_warns_about_credentials_and_runtime_and_deletes_nothing() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");

    // Provisioned home plus planted active runtime state and a role-owned credential fixture.
    let out = run_hotpot(home.path(), fake_home.path(), None, &["setup", "--apply"]);
    assert!(out.status.success());
    let ledger = home.path().join("runtime/v1/ledger.jsonl");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    fs::write(&ledger, "{\"seq\":1,\"type\":\"room_opened\"}\n").unwrap();
    let credential = home.path().join("roles/codex/supervisor/auth.json");
    fs::create_dir_all(credential.parent().unwrap()).unwrap();
    fs::write(&credential, "{\"token\":\"seat-secret\"}\n").unwrap();
    let tree_before = relative_tree(home.path());

    let out = run_hotpot(home.path(), fake_home.path(), None, &["remove"]);
    assert!(
        out.status.success(),
        "remove dry-run must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    let stderr = stderr_of(&out);
    assert!(
        stdout.contains("would remove"),
        "remove dry-run must list planned deletions:\n{stdout}"
    );
    assert!(
        stdout.contains(&ledger.display().to_string()),
        "remove dry-run must list the runtime ledger among deletions:\n{stdout}"
    );
    assert!(
        stderr.to_lowercase().contains("credential"),
        "remove must warn about role-owned credentials:\n{stderr}"
    );
    assert!(
        stderr.contains(&credential.display().to_string()),
        "the credential warning must name the credential file:\n{stderr}"
    );
    assert!(
        stderr.to_lowercase().contains("active runtime state"),
        "remove must warn about active runtime state when runtime/v1/ is non-empty:\n{stderr}"
    );
    assert!(
        stderr.contains("runtime/v1"),
        "the runtime warning must name runtime/v1:\n{stderr}"
    );

    // Dry-run deletes nothing.
    assert_eq!(
        relative_tree(home.path()),
        tree_before,
        "remove dry-run must not delete anything"
    );
    assert!(credential.exists() && ledger.exists());
}

#[test]
fn remove_apply_tears_down_hotpot_home() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    let out = run_hotpot(home.path(), fake_home.path(), None, &["setup", "--apply"]);
    assert!(out.status.success());
    let ledger = home.path().join("runtime/v1/ledger.jsonl");
    fs::create_dir_all(ledger.parent().unwrap()).unwrap();
    fs::write(&ledger, "{\"seq\":1}\n").unwrap();

    let out = run_hotpot(home.path(), fake_home.path(), None, &["remove", "--apply"]);
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(
        !home.path().join("hotpot.json").exists(),
        "remove --apply must remove the provisioned Hotpot Home contents"
    );
    assert!(
        !home.path().join("roles").exists() && !ledger.exists(),
        "remove --apply must tear down the whole Hotpot Home"
    );
}

#[test]
fn remove_on_missing_home_is_a_clean_no_op() {
    let fake_home = TempDir::new("fakehome");
    let missing = fake_home.path().join("no-such-hotpot-home");

    let out = run_hotpot(&missing, fake_home.path(), None, &["remove"]);
    assert!(
        out.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    assert!(!missing.exists());
}

#[test]
fn setup_apply_refuses_directory_occupied_path_with_no_partial_state() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");
    // Pre-occupy the planned `hotpot.json` path with a directory holding a file.
    let occupied = home.path().join("hotpot.json");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("inner.txt"), "occupied\n").unwrap();
    let tree_before = relative_tree(home.path());

    let out = run_hotpot(home.path(), fake_home.path(), None, &["setup", "--apply"]);
    assert!(
        !out.status.success(),
        "setup --apply must exit non-zero when a planned file path is a directory"
    );
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("hotpot.json"),
        "the refusal diagnostic must name the conflicting path:\n{stderr}"
    );

    // No partial state: nothing else was provisioned and no temporary siblings remain.
    assert_eq!(
        relative_tree(home.path()),
        tree_before,
        "refusal must happen before any disk mutation"
    );
    assert!(
        occupied.join("inner.txt").exists(),
        "the occupied directory must be left untouched"
    );
    assert!(
        relative_tree(home.path())
            .iter()
            .all(|rel| !rel.contains("hotpot-tmp")),
        "no temporary sibling files may remain anywhere under the temp home"
    );
}

#[test]
fn setup_apply_replaces_existing_files_atomically_on_re_run() {
    let home = TempDir::new("home");
    let fake_home = TempDir::new("fakehome");

    let first = run_hotpot(home.path(), fake_home.path(), None, &["setup", "--apply"]);
    assert!(first.status.success());
    let marker = home.path().join("hotpot.json");
    let rendered = fs::read_to_string(&marker).unwrap();
    assert!(!rendered.trim().is_empty());

    // Corrupt the provisioned file, then re-run: the final content must be exactly the new
    // rendering, with no temporary/partial siblings left behind.
    fs::write(&marker, "corrupted half-written").unwrap();
    let second = run_hotpot(home.path(), fake_home.path(), None, &["setup", "--apply"]);
    assert!(
        second.status.success(),
        "re-running setup --apply must succeed:\n{}",
        stderr_of(&second)
    );
    assert_eq!(
        fs::read_to_string(&marker).unwrap(),
        rendered,
        "re-apply must atomically restore exactly the new rendering"
    );
    assert!(
        relative_tree(home.path())
            .iter()
            .all(|rel| !rel.contains("hotpot-tmp")),
        "no .tmp/partial siblings may remain after re-apply"
    );
}

#[test]
fn setup_and_remove_exit_nonzero_with_stderr_when_home_cannot_be_resolved() {
    for subcommand in ["setup", "remove"] {
        let out = run_hotpot_without_home_env(&[subcommand]);
        assert!(
            !out.status.success(),
            "`hotpot {subcommand}` without HOTPOT_HOME or HOME must exit non-zero"
        );
        assert!(
            !stderr_of(&out).trim().is_empty(),
            "`hotpot {subcommand}` must print a stderr diagnostic"
        );
    }
}


