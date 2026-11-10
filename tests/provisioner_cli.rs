//! CLI-level integration tests for the Hotpot provisioner core (`hotpot setup` / `hotpot remove`).
//!
//! Seam (per `.scratch/hotpot-foundation/spec.md` "Testing Decisions → Highest-Level Testing
//! Seam"): invoke the `hotpot` binary with `HOTPOT_HOME` and `HOME` pointed at temp dirs and a
//! fake `hc` shim on PATH, then assert stdout/stderr text, exit codes, and the file tree under
//! the temp home. Zero network; the operator homes under the fake `HOME` are read-only inputs.

use std::collections::BTreeSet;
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

/// Expected skeleton of the base Hotpot Home plan, derived from the spec's Provisioner Layer
/// artifact list (`roles/<codex|claude|pi>/<supervisor|lead|peer>/`, `skills/`,
/// `skill-projections/<agent>/lead/`, `hotpot.json` marker) — the independent source of truth
/// for the dry-run assertions below.
fn base_plan_relative_paths() -> BTreeSet<String> {
    let mut paths = BTreeSet::new();
    for agent in ["codex", "claude", "pi"] {
        for role in ["supervisor", "lead", "peer"] {
            paths.insert(format!("roles/{agent}/{role}"));
        }
        paths.insert(format!("skill-projections/{agent}/lead"));
    }
    paths.insert("skills".to_string());
    paths.insert("hotpot.json".to_string());
    paths
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
    for rel in base_plan_relative_paths() {
        let abs = home.path().join(&rel);
        let prefix = if rel == "hotpot.json" { "file" } else { "dir" };
        assert!(
            stdout.contains(&format!("plan: {prefix} {}", abs.display())),
            "dry-run stdout must print planned {prefix} {rel}:\n{stdout}"
        );
    }
    let plan_line_count = stdout
        .lines()
        .filter(|l| l.starts_with("plan: "))
        .count();
    assert_eq!(
        plan_line_count,
        base_plan_relative_paths().len(),
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
    let hc_log = TempDir::new("hclog").path().join("hc.log");
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
