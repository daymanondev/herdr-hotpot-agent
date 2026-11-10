//! CLI-level integration tests for ticket 02 of the Hotpot Native UI spec: operator
//! credential seeding into Role Homes.
//!
//! Seam (per `.scratch/hotpot-native-ui/spec.md` "Testing Decisions → Highest-Level
//! Testing Seam" 1): invoke the built `hotpot setup` / `hotpot verify` / `hotpot seat
//! launch` binaries against temporary `HOTPOT_HOME`s, fake operator `HOME` directories
//! carrying credential fixtures, and temporary git repositories. Assert seeded files are
//! independent regular files (content equal, `nlink == 1`, not symlinks), operator files
//! untouched, pre-existing seat credentials preserved across re-runs, and the resulting
//! verify diagnostics. Zero network; no token values ever asserted in or printed to any
//! output — fixtures carry sentinel values that must stay inside file bytes.

mod common;

use common::{TempDir, init_git_repo};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Sentinel secret planted inside operator credential fixtures: it must ride along into
/// the seeded copies (byte equality) but never surface in any `hotpot` output.
const SENTINEL: &str = "SEED-SENTINEL-t0k3n-do-not-print";

/// The agent families provisioned by `hotpot setup`.
const AGENTS: [&str; 3] = ["codex", "claude", "pi"];

/// The three roles every agent family is provisioned for.
const ROLES: [&str; 3] = ["supervisor", "lead", "peer"];

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Minimal PATH so the real operator environment can never leak into these tests; fake
/// binaries (`hc`, vendors, `pi` probe) live in a prepended temp bin dir.
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

/// Runs a clean `setup --apply` and asserts it exits 0.
fn apply_clean(home: &Path, fake_home: &Path, fake_bin: Option<&Path>) {
    let out = run_hotpot(home, fake_home, fake_bin, &[], &["setup", "--apply"]);
    assert!(
        out.status.success(),
        "clean setup --apply must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
}

/// Every path under `root`, relative to it (recursive).
fn relative_tree(root: &Path) -> std::collections::BTreeSet<String> {
    let mut set = std::collections::BTreeSet::new();
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

/// The seat credential file path for one seat (mirroring the vendor names: codex/pi
/// `auth.json`, claude `.credentials.json`).
fn seat_credential(home: &Path, agent: &str, role: &str) -> PathBuf {
    let name = match agent {
        "claude" => ".credentials.json",
        _ => "auth.json",
    };
    home.join("roles").join(agent).join(role).join(name)
}

/// The shape-record sidecar path beside one seat's credential.
fn seat_shape_record(home: &Path, agent: &str, role: &str) -> PathBuf {
    seat_credential(home, agent, role)
        .parent()
        .unwrap()
        .join("credentials.preserved.json")
}

/// Plants structurally distinct, byte-distinct JSON credential fixtures in the fake
/// operator homes (each carrying the sentinel as a JSON value only) and returns the
/// `(path, contents)` pairs.
fn plant_operator_credentials(fake_home: &Path) -> Vec<(PathBuf, String)> {
    let fixtures = vec![
        (
            fake_home.join(".codex/auth.json"),
            format!("{{\"tokens\": {{\"access_token\": \"{SENTINEL}-codex\"}}}}\n"),
        ),
        (
            fake_home.join(".claude/.credentials.json"),
            format!(
                "{{\"tokens\": {{\"access_token\": \"{SENTINEL}-claude\"}}, \
                 \"last_refresh\": \"2026-11-10T00:00:00Z\"}}\n"
            ),
        ),
        (
            fake_home.join(".pi/agent/auth.json"),
            format!("{{\"account\": \"{SENTINEL}-pi\", \"keys\": {{\"api\": \"{SENTINEL}\"}}}}\n"),
        ),
    ];
    for (path, contents) in &fixtures {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    fixtures
}

/// The operator credential fixture contents for `agent` (the plan order: codex, claude, pi).
fn operator_fixture<'a>(fixtures: &'a [(PathBuf, String)], agent: &str) -> &'a str {
    let index = AGENTS
        .iter()
        .position(|a| *a == agent)
        .unwrap_or_else(|| panic!("no fixture for {agent}"));
    &fixtures[index].1
}

/// Writes a fake `pi` binary that answers verify's offline `/mcp` RPC probe (the probe
/// never touches the network).
fn write_fake_pi(bin_dir: &Path) {
    fs::create_dir_all(bin_dir).unwrap();
    let script = "#!/bin/sh\nprintf '{\"path\":\"/mcp\",\"extensions\":[\"builtin:mcp\"]}\\n'\nexit 0\n";
    let shim = bin_dir.join("pi");
    fs::write(&shim, script).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

// ---------------------------------------------------------------------------
// Acceptance criterion 1: setup plans and applies the seeding; criterion 2: the
// operator home stays strictly read-only.
// ---------------------------------------------------------------------------

#[test]
fn setup_apply_seeds_independent_copies_into_every_role_home_that_lacks_one() {
    let home = TempDir::new("seed");
    let fake_home = TempDir::new("seed-op");
    let fixtures = plant_operator_credentials(fake_home.path());
    let operator_tree_before = relative_tree(fake_home.path());

    // The dry-run plans one credential copy plus one shape sidecar per seat.
    let dry = run_hotpot(home.path(), fake_home.path(), None, &[], &["setup"]);
    assert!(
        dry.status.success(),
        "dry-run must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&dry),
        stderr_of(&dry)
    );
    let dry_stdout = stdout_of(&dry);
    for agent in AGENTS {
        for role in ROLES {
            assert!(
                dry_stdout.contains(&format!(
                    "plan: file {}",
                    seat_credential(home.path(), agent, role).display()
                )),
                "dry-run must plan the credential seed for {agent}/{role}:\n{dry_stdout}"
            );
            assert!(
                dry_stdout.contains(&format!(
                    "plan: file {}",
                    seat_shape_record(home.path(), agent, role).display()
                )),
                "dry-run must plan the shape sidecar for {agent}/{role}:\n{dry_stdout}"
            );
        }
    }

    apply_clean(home.path(), fake_home.path(), None);

    // Every seeded file is an independent regular file whose bytes equal the operator
    // copy: not a symlink, not a hardlink (nlink == 1).
    for agent in AGENTS {
        let operator_bytes = operator_fixture(&fixtures, agent).as_bytes().to_vec();
        for role in ROLES {
            let seeded = seat_credential(home.path(), agent, role);
            let meta = fs::symlink_metadata(&seeded)
                .unwrap_or_else(|e| panic!("seeded credential missing for {agent}/{role}: {e}"));
            assert!(
                meta.is_file(),
                "the seeded credential for {agent}/{role} must be a regular file"
            );
            assert!(
                !meta.file_type().is_symlink(),
                "the seeded credential for {agent}/{role} must never be a symlink"
            );
            assert_eq!(
                meta.nlink(),
                1,
                "the seeded credential for {agent}/{role} must not be a hardlink"
            );
            assert_eq!(
                meta.permissions().mode() & 0o777,
                0o600,
                "the seeded credential for {agent}/{role} must stay owner-only (0600) like \
                 the operator's original, not group/world-readable inside the Role Home"
            );
            assert_eq!(
                fs::read(&seeded).unwrap(),
                operator_bytes,
                "the seeded credential for {agent}/{role} must equal the operator bytes"
            );

            // The shape sidecar rides beside every seeded copy: shape only, never values.
            let sidecar = seat_shape_record(home.path(), agent, role);
            let sidecar_text = fs::read_to_string(&sidecar)
                .unwrap_or_else(|e| panic!("shape sidecar missing for {agent}/{role}: {e}"));
            let record: serde_json::Value = serde_json::from_str(&sidecar_text)
                .unwrap_or_else(|e| panic!("sidecar for {agent}/{role} must parse: {e}"));
            assert_eq!(
                record["len"].as_u64(),
                Some(operator_bytes.len() as u64),
                "the sidecar for {agent}/{role} must record the credential byte length"
            );
            assert!(
                !sidecar_text.contains(SENTINEL),
                "the sidecar for {agent}/{role} stores shape only, never token values"
            );
        }
    }

    // The operator home is strictly read-only: byte-identical contents and no new paths
    // anywhere under the fake HOME after plan + apply.
    for (path, contents) in &fixtures {
        assert_eq!(
            fs::read_to_string(path).unwrap(),
            *contents,
            "operator credential {} was mutated by hotpot",
            path.display()
        );
    }
    assert_eq!(
        relative_tree(fake_home.path()),
        operator_tree_before,
        "no new files may appear under the operator HOME after plan + apply"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 3 (user story 3): a Role Home credential that already exists —
// e.g. customized via `hotpot auth login` — is never overwritten by the operator copy.
// ---------------------------------------------------------------------------

#[test]
fn a_preexisting_seat_credential_survives_setup_reruns_byte_for_byte() {
    let home = TempDir::new("seed-keep");
    let fake_home = TempDir::new("seed-keep-op");
    plant_operator_credentials(fake_home.path());

    // A prior per-seat override (the byte shape `hotpot auth login` leaves behind: a
    // distinct credential plus its own matching shape record) in one Role Home.
    let override_credential = seat_credential(home.path(), "codex", "lead");
    let override_bytes = "{\"tokens\": {\"access_token\": \"seat-override-only\"}}\n";
    fs::create_dir_all(override_credential.parent().unwrap()).unwrap();
    fs::write(&override_credential, override_bytes).unwrap();
    fs::write(
        seat_shape_record(home.path(), "codex", "lead"),
        "{\"len\": 49, \"keys\": [\"tokens\", \"tokens.access_token\"]}\n",
    )
    .unwrap();

    // Two full re-runs with the operator credential present: the override must survive.
    apply_clean(home.path(), fake_home.path(), None);
    apply_clean(home.path(), fake_home.path(), None);

    assert_eq!(
        fs::read_to_string(&override_credential).unwrap(),
        override_bytes,
        "a pre-existing seat credential must survive setup --apply byte-for-byte"
    );
    // The seat keeps its own shape record too — not the operator copy's.
    assert_eq!(
        fs::read_to_string(seat_shape_record(home.path(), "codex", "lead")).unwrap(),
        "{\"len\": 49, \"keys\": [\"tokens\", \"tokens.access_token\"]}\n",
        "a pre-existing seat credential keeps its own shape record across re-runs"
    );
    // Every other seat still received its seeded copy.
    assert_eq!(
        fs::read_to_string(seat_credential(home.path(), "codex", "supervisor")).unwrap(),
        *operator_fixture(&plant_operator_credentials_fixture(), "codex"),
        "seats without their own credential are still seeded"
    );
}

/// Replants the operator fixtures into a scratch dir and returns them (a fresh copy of the
/// fixture list for byte comparisons, without touching the live fake home).
fn plant_operator_credentials_fixture() -> Vec<(PathBuf, String)> {
    let scratch = TempDir::new("seed-fixture");
    plant_operator_credentials(scratch.path())
}

// ---------------------------------------------------------------------------
// Acceptance criterion 4: after seeding, `hotpot verify` reports every seeded seat
// `configured structurally` (shape sidecar present and matching) — never `login-required`
// or `legacy-shared-risk` — and no secret ever surfaces in the report.
// ---------------------------------------------------------------------------

/// The per-seat diagnostic line prefix `hotpot verify` prints.
fn diag_line(agent: &str, role: &str, state: &str) -> String {
    format!("verify: diag auth {agent}/{role} {state}")
}

#[test]
fn verify_reports_every_seeded_seat_configured_structurally() {
    let home = TempDir::new("seed-verify");
    let fake_home = TempDir::new("seed-verify-op");
    plant_operator_credentials(fake_home.path());
    let bin = TempDir::new("seed-verify-bin");
    write_fake_pi(bin.path());
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &[], &["verify"]);
    assert!(
        out.status.success(),
        "verify must exit 0 (diagnostics are non-blocking)\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    for agent in AGENTS {
        for role in ROLES {
            assert!(
                stdout.contains(&diag_line(agent, role, "configured structurally")),
                "seeded seat {agent}/{role} must verify as configured structurally:\n{stdout}"
            );
        }
    }
    assert!(
        !stdout.contains("login-required") && !stdout.contains("legacy-shared-risk"),
        "no seeded seat may report login-required or legacy-shared-risk:\n{stdout}"
    );
    assert!(
        !stdout.contains(SENTINEL) && !stderr_of(&out).contains(SENTINEL),
        "the seeded sentinel secret must never appear in verify output"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 4, claude special case: an operator `.credentials.json` declaring
// native keyring storage is not a file-backed credential — it is skipped, and those seats
// stay `login-required` until a real `hotpot auth login`.
// ---------------------------------------------------------------------------

#[test]
fn a_claude_keyring_operator_credential_is_skipped_and_the_seat_stays_login_required() {
    let home = TempDir::new("seed-keyring");
    let fake_home = TempDir::new("seed-keyring-op");

    // File-backed codex and pi credentials, but a claude keyring marker file.
    let mut fixtures = vec![
        (
            fake_home.path().join(".codex/auth.json"),
            format!("{{\"tokens\": {{\"access_token\": \"{SENTINEL}-codex\"}}}}\n"),
        ),
        (
            fake_home.path().join(".pi/agent/auth.json"),
            format!("{{\"account\": \"{SENTINEL}-pi\"}}\n"),
        ),
    ];
    fixtures.push((
        fake_home.path().join(".claude/.credentials.json"),
        "{\"storage\": \"keyring\", \"account\": \"claude-main\"}\n".to_string(),
    ));
    for (path, contents) in &fixtures {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    let bin = TempDir::new("seed-keyring-bin");
    write_fake_pi(bin.path());

    // The dry-run plans no claude seeding at all.
    let dry = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &[], &["setup"]);
    assert!(dry.status.success(), "dry-run must exit 0");
    let dry_stdout = stdout_of(&dry);
    for role in ROLES {
        assert!(
            !dry_stdout.contains(&format!(
                "plan: file {}",
                seat_credential(home.path(), "claude", role).display()
            )),
            "a keyring-declaring operator credential must not be seeded (claude/{role}):\n{dry_stdout}"
        );
    }

    apply_clean(home.path(), fake_home.path(), Some(bin.path()));
    for role in ROLES {
        assert!(
            !seat_credential(home.path(), "claude", role).exists(),
            "no claude/{role} credential file may be created from a keyring marker"
        );
    }

    let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &[], &["verify"]);
    assert!(out.status.success(), "verify stays non-blocking");
    let stdout = stdout_of(&out);
    for role in ROLES {
        assert!(
            stdout.contains(&diag_line("claude", role, "login-required")),
            "claude/{role} must stay login-required when the operator credential declares \
             keyring storage:\n{stdout}"
        );
    }
    // The file-backed families are still seeded and verify clean.
    for role in ROLES {
        assert!(
            stdout.contains(&diag_line("codex", role, "configured structurally"))
                && stdout.contains(&diag_line("pi", role, "configured structurally")),
            "codex and pi seats are still seeded and configured:\n{stdout}"
        );
    }
}

// ---------------------------------------------------------------------------
// Acceptance criterion 5: `hotpot seat launch` seeds a missing credential on demand —
// after the Role Home exists, before the `hc` terminal is created (same never-overwrite,
// never-symlink rules) — riding the fake-`hc` launch harness from `tests/seat_launch.rs`.
// ---------------------------------------------------------------------------

/// The fake `hc`: `hc run ...` records one argv line into `<state>/create.log`;
/// `hc ls --json` prints the scripted `<state>/list.json`; anything else exits 0
/// (the state dir rides on `HOTPOT_FAKE_HC_STATE`, inherited from the `hotpot` process).
fn write_fake_hc(bin_dir: &Path) {
    fs::create_dir_all(bin_dir).unwrap();
    let script = "\
#!/bin/sh\n\
state=\"$HOTPOT_FAKE_HC_STATE\"\n\
case \"$1\" in\n\
  ls) exec cat \"$state/list.json\" ;;\n\
  run) printf 'run-recorded\\n' >> \"$state/create.log\" ;;\n\
esac\n\
exit 0\n";
    let shim = bin_dir.join("hc");
    fs::write(&shim, script).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A fake vendor CLI binary: executable, never executed (the fake `hc` owns the terminal),
/// but resolvable on `PATH` by `hotpot seat launch`.
fn write_fake_vendor(bin_dir: &Path, vendor: &str) {
    let shim = bin_dir.join(vendor);
    fs::write(&shim, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn seat_launch_seeds_a_missing_credential_before_creating_the_terminal() {
    let home = TempDir::new("seed-launch");
    let fake_home = TempDir::new("seed-launch-op");
    let bin = TempDir::new("seed-launch-bin");
    let hc_state = TempDir::new("seed-launch-hc");
    let (repo, _head) = init_git_repo();

    // Provisioned-but-unseeded: setup --apply ran while the operator had no credential.
    fs::write(hc_state.path().join("list.json"), "[]\n").unwrap();
    write_fake_hc(bin.path());
    for vendor in ["codex", "claude", "pi"] {
        write_fake_vendor(bin.path(), vendor);
    }
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));
    assert!(
        !seat_credential(home.path(), "codex", "supervisor").exists(),
        "fixture: the seat starts unseeded (setup ran before the operator credential existed)"
    );

    // The operator logs in afterwards; the next launch inherits the credential on demand.
    let operator_credential = fake_home.path().join(".codex/auth.json");
    let operator_bytes = format!("{{\"tokens\": {{\"access_token\": \"{SENTINEL}-late\"}}}}\n");
    fs::create_dir_all(operator_credential.parent().unwrap()).unwrap();
    fs::write(&operator_credential, &operator_bytes).unwrap();

    let out = run_hotpot(
        home.path(),
        fake_home.path(),
        Some(bin.path()),
        &[("HOTPOT_FAKE_HC_STATE", &hc_state.path().to_string_lossy())],
        &[
            "seat",
            "launch",
            "supervisor",
            "--agent",
            "codex",
            "--project",
            repo.path().to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "seat launch must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );

    // The fake `hc` harness still saw the terminal creation.
    let create_log = fs::read_to_string(hc_state.path().join("create.log")).unwrap_or_default();
    assert_eq!(
        create_log.trim_end(),
        "run-recorded",
        "the launch must still create the hc terminal after seeding:\n{create_log}"
    );

    // The Role Home now owns an independent seeded copy plus its shape sidecar.
    let seeded = seat_credential(home.path(), "codex", "supervisor");
    let meta = fs::symlink_metadata(&seeded)
        .unwrap_or_else(|e| panic!("launch must seed the missing credential: {e}"));
    assert!(meta.is_file() && !meta.file_type().is_symlink() && meta.nlink() == 1);
    assert_eq!(
        meta.permissions().mode() & 0o777,
        0o600,
        "the on-demand seeded credential must also land owner-only (0600)"
    );
    assert_eq!(
        fs::read_to_string(&seeded).unwrap(),
        operator_bytes,
        "the seeded copy must equal the operator bytes"
    );
    let sidecar = fs::read_to_string(seat_shape_record(home.path(), "codex", "supervisor"))
        .expect("launch must record the shape sidecar beside the seeded copy");
    assert!(!sidecar.contains(SENTINEL), "the sidecar stores shape only");

    // The operator's file is a read-only input even on the launch path.
    assert_eq!(
        fs::read_to_string(&operator_credential).unwrap(),
        operator_bytes
    );

    // A second launch never overwrites the seeded credential (never-overwrite rule).
    fs::write(&seeded, "{\"tokens\": {\"access_token\": \"seat-own-override\"}}\n").unwrap();
    let again = run_hotpot(
        home.path(),
        fake_home.path(),
        Some(bin.path()),
        &[("HOTPOT_FAKE_HC_STATE", &hc_state.path().to_string_lossy())],
        &[
            "seat",
            "launch",
            "supervisor",
            "--agent",
            "codex",
            "--project",
            repo.path().to_str().unwrap(),
        ],
    );
    assert!(again.status.success(), "re-launch must exit 0");
    assert_eq!(
        fs::read_to_string(&seeded).unwrap(),
        "{\"tokens\": {\"access_token\": \"seat-own-override\"}}\n",
        "a re-launch must never overwrite the seat's existing credential"
    );
}
