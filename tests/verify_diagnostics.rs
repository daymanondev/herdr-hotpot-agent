//! CLI-level integration tests for ticket 05 of the Hotpot Foundation spec: the Pi
//! `models.json` (ZCode via TriDefender/zcode-api) projection into Pi Role Homes and the
//! five non-blocking, secret-free, offline `hotpot verify` authentication diagnostics.
//!
//! Seam (per `.scratch/hotpot-foundation/spec.md` "Testing Decisions → Highest-Level Testing
//! Seam"): invoke the `hotpot` binary with `HOTPOT_HOME` and `HOME` pointed at temp dirs, a
//! fixture operator Pi home (`~/.pi/agent`), fixture seat credential files, and a fake `pi`
//! binary on `PATH` (the offline RPC probe hits the fake binary, never the network), then
//! assert stdout diagnostic lines, exit codes, projected files, and byte-unchanged operator
//! files. Zero network; operator homes are read-only inputs.

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
            "hotpot-diag-{tag}-{}-{}",
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

/// The three roles every agent family is provisioned for.
const ROLES: [&str; 3] = ["supervisor", "lead", "peer"];

/// The agent families provisioned by `hotpot setup`.
const AGENTS: [&str; 3] = ["codex", "claude", "pi"];

/// Sentinel secret planted inside fixture credential files: it must never surface in any
/// `hotpot verify` output (acceptance criterion 4).
const SENTINEL: &str = "SENTINEL-SECRET-t0k3n-do-not-print";

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

// ---------------------------------------------------------------------------
// Pi models.json (ZCode via TriDefender/zcode-api) projection — acceptance criterion 1
// ---------------------------------------------------------------------------

/// Operator Pi models.json fixture: a `zcode` provider served by the local
/// TriDefender/zcode-api endpoint on `127.0.0.1:8080`, plus a second unrelated provider to
/// prove every operator entry survives the projection.
fn plant_operator_pi_models(fake_home: &Path) -> PathBuf {
    let models = r#"{
  "providers": {
    "zcode": {
      "name": "ZCode via TriDefender/zcode-api",
      "baseUrl": "http://127.0.0.1:8080/v1",
      "apiKey": "local-proxy",
      "models": [
        {"id": "glm-5.3", "label": "GLM 5.3", "contextWindow": 200000},
        {"id": "glm-5.3-air", "label": "GLM 5.3 Air"}
      ]
    },
    "anthropic": {
      "name": "Anthropic",
      "baseUrl": "https://api.anthropic.com/v1",
      "models": [{"id": "claude-opus-4-6"}]
    }
  }
}
"#;
    let path = fake_home.join(".pi/agent/models.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, models).unwrap();
    path
}

#[test]
fn setup_apply_projects_the_operator_pi_models_into_every_pi_role_home() {
    let home = TempDir::new("pimodels");
    let fake_home = TempDir::new("fakehome");
    let operator_models = plant_operator_pi_models(fake_home.path());
    let bytes_before = fs::read(&operator_models).unwrap();
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);

    // The dry-run plans one models config per Pi Role Home.
    let dry = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &["setup"]);
    assert!(
        dry.status.success(),
        "dry-run must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&dry),
        stderr_of(&dry)
    );
    let dry_stdout = stdout_of(&dry);
    for role in ROLES {
        let planned = home.path().join(format!("roles/pi/{role}/models.json"));
        assert!(
            dry_stdout.contains(&format!("plan: file {}", planned.display())),
            "dry-run must plan the Pi models config for pi/{role}:\n{dry_stdout}"
        );
    }

    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    // Every Pi Role Home carries the operator's providers, zcode endpoint included.
    for role in ROLES {
        let projected = read_json(&home.path().join(format!("roles/pi/{role}/models.json")));
        let providers = projected["providers"]
            .as_object()
            .unwrap_or_else(|| panic!("pi/{role} models.json must carry a providers object"));
        assert_eq!(
            providers.len(),
            2,
            "pi/{role} models.json must preserve every operator provider"
        );
        assert_eq!(
            providers["zcode"]["baseUrl"].as_str(),
            Some("http://127.0.0.1:8080/v1"),
            "pi/{role} models.json must carry the zcode provider at the local endpoint"
        );
        let ids: Vec<&str> = providers["zcode"]["models"]
            .as_array()
            .expect("zcode models array survives the projection")
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec!["glm-5.3", "glm-5.3-air"],
            "pi/{role} models.json must preserve the zcode model entries"
        );
    }

    // The operator's file is a read-only input: byte-identical afterwards.
    assert_eq!(
        fs::read(&operator_models).unwrap(),
        bytes_before,
        "setup must never mutate the operator's models.json"
    );
}

// ---------------------------------------------------------------------------
// Pi models.json projection — acceptance criterion 2: absent operator file
// ---------------------------------------------------------------------------

#[test]
fn setup_apply_without_an_operator_pi_models_notes_and_projects_no_models_file() {
    let home = TempDir::new("nopimodels");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);

    let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &["setup", "--apply"]);
    assert!(
        out.status.success(),
        "setup --apply without an operator Pi models.json must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    let stdout = stdout_of(&out);
    assert!(
        stdout.contains(".pi/agent/models.json"),
        "the stdout note must name the absent operator file:\n{stdout}"
    );
    assert!(
        stdout.lines().any(|l| l.starts_with("note: ")),
        "the absent-models report must be a stdout note line:\n{stdout}"
    );

    // No models file anywhere under the Pi Role Homes.
    for role in ROLES {
        assert!(
            !home.path().join(format!("roles/pi/{role}/models.json")).exists(),
            "no models config may be written for pi/{role} when the operator file is absent"
        );
    }
}

// ---------------------------------------------------------------------------
// Pi models.json projection: an unparseable operator file fails setup closed
// ---------------------------------------------------------------------------

#[test]
fn unparseable_operator_pi_models_fails_setup_closed_before_writing() {
    let home = TempDir::new("badpimodels");
    let fake_home = TempDir::new("fakehome");
    let path = fake_home.path().join(".pi/agent/models.json");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{ not json ]").unwrap();

    for args in [&["setup"][..], &["setup", "--apply"][..]] {
        let out = run_hotpot(home.path(), fake_home.path(), None, args);
        assert!(
            !out.status.success(),
            "`hotpot {}` must fail closed when the operator Pi models config cannot be projected",
            args.join(" ")
        );
        let stderr = stderr_of(&out);
        assert!(
            stderr.contains("pi-models-projection"),
            "the diagnostic must carry the pi-models-projection failure code:\n{stderr}"
        );
    }

    // Nothing was provisioned and the operator file is a read-only input even on failure.
    assert!(
        !home.path().join("hotpot.json").exists(),
        "nothing may be written under Hotpot Home when the Pi models config cannot be projected"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json ]");
}

// ---------------------------------------------------------------------------
// verify auth diagnostics — acceptance criterion 3: the five per-seat states, non-blocking
// (verify exits 0 when only diagnostics are non-clean)
// ---------------------------------------------------------------------------

/// The per-seat diagnostic line prefix `hotpot verify` prints.
fn diag_line(agent: &str, role: &str, state: &str) -> String {
    format!("verify: diag auth {agent}/{role} {state}")
}

/// Runs `hotpot verify` on the home and asserts it exits 0 (diagnostics are non-blocking).
fn verify_green(home: &Path, fake_home: &Path, bin: &Path) -> String {
    let out = run_hotpot(home, fake_home, Some(bin), &["verify"]);
    assert!(
        out.status.success(),
        "verify must exit 0 while diagnostics are non-blocking\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );
    stdout_of(&out)
}

/// The seat credential file path `hotpot setup` declares for one seat (mirroring the vendor
/// names: codex/pi `auth.json`, claude `.credentials.json`).
fn seat_credential(home: &Path, agent: &str, role: &str) -> PathBuf {
    let name = match agent {
        "claude" => ".credentials.json",
        _ => "auth.json",
    };
    home.join("roles").join(agent).join(role).join(name)
}

/// Plants a fixture credential file (and optional preserved shape record) for one seat.
fn plant_seat_credential(home: &Path, agent: &str, role: &str, contents: &str) -> PathBuf {
    let path = seat_credential(home, agent, role);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, contents).unwrap();
    path
}

#[test]
fn verify_reports_login_required_for_every_seat_without_credentials_and_exits_zero() {
    let home = TempDir::new("nologin");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    let stdout = verify_green(home.path(), fake_home.path(), bin.path());
    for agent in AGENTS {
        for role in ROLES {
            assert!(
                stdout.contains(&diag_line(agent, role, "login-required")),
                "verify must report login-required for {agent}/{role}:\n{stdout}"
            );
        }
    }
}

#[test]
fn verify_reports_legacy_shared_risk_for_a_symlinked_seat_credential() {
    let home = TempDir::new("symlinked");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    // The legacy shape: the seat's credential path symlinks straight at the operator's
    // mutable credential file.
    let operator_auth = fake_home.path().join(".codex/auth.json");
    fs::create_dir_all(operator_auth.parent().unwrap()).unwrap();
    fs::write(&operator_auth, format!("{{\"token\": \"{SENTINEL}\"}}\n")).unwrap();
    let seat = seat_credential(home.path(), "codex", "lead");
    std::os::unix::fs::symlink(&operator_auth, &seat).unwrap();

    let stdout = verify_green(home.path(), fake_home.path(), bin.path());
    assert!(
        stdout.contains(&diag_line("codex", "lead", "legacy-shared-risk")),
        "a symlinked credential must report legacy-shared-risk:\n{stdout}"
    );
    // One seat's shape does not disturb the others.
    assert!(
        stdout.contains(&diag_line("codex", "supervisor", "login-required")),
        "untouched seats keep their own diagnostic:\n{stdout}"
    );
}

#[test]
fn verify_reports_diverged_file_preserve_when_the_credential_leaves_its_preserved_shape() {
    let home = TempDir::new("diverged");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    // A logged-in seat whose live credential no longer matches the shape record preserved
    // beside it at login time (here: the file grew after the record was taken).
    let credential = plant_seat_credential(
        home.path(),
        "claude",
        "peer",
        &format!("{{\"tokens\": {{\"access_token\": \"{SENTINEL}\"}}}}\n"),
    );
    let record = credential
        .parent()
        .unwrap()
        .join("credentials.preserved.json");
    fs::write(
        &record,
        "{\"len\": 4096, \"keys\": [\"tokens\", \"tokens.access_token\"]}\n",
    )
    .unwrap();

    let stdout = verify_green(home.path(), fake_home.path(), bin.path());
    assert!(
        stdout.contains(&diag_line("claude", "peer", "diverged-file-preserve")),
        "a credential diverging from its preserved shape record must be reported:\n{stdout}"
    );
}

#[test]
fn verify_reports_native_keyring_unverifiable_for_a_keyring_style_credential() {
    let home = TempDir::new("keyring");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    // A keyring-style provider: the file is only a marker declaring that the real secret
    // lives in the OS-native keyring, which an offline verify cannot confirm.
    plant_seat_credential(
        home.path(),
        "pi",
        "supervisor",
        "{\"storage\": \"keyring\", \"account\": \"pi-main\"}\n",
    );

    let stdout = verify_green(home.path(), fake_home.path(), bin.path());
    assert!(
        stdout.contains(&diag_line("pi", "supervisor", "native-keyring unverifiable")),
        "a keyring-style credential must report native-keyring unverifiable:\n{stdout}"
    );
}

#[test]
fn verify_reports_configured_structurally_for_a_complete_file_backed_credential() {
    let home = TempDir::new("configured");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    // A structurally complete file-backed credential: a regular seat-owned file parsing as
    // a non-empty JSON object (the sentinel value must never surface in the report).
    plant_seat_credential(
        home.path(),
        "codex",
        "supervisor",
        &format!("{{\"tokens\": {{\"access_token\": \"{SENTINEL}\"}}}}\n"),
    );

    let stdout = verify_green(home.path(), fake_home.path(), bin.path());
    assert!(
        stdout.contains(&diag_line("codex", "supervisor", "configured structurally")),
        "a structurally complete credential must report configured structurally:\n{stdout}"
    );
    assert!(
        stdout.contains(&diag_line("codex", "lead", "login-required")),
        "seats without credentials keep reporting login-required:\n{stdout}"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 4: secrets planted inside credential files never surface in verify
// output — including as key names, which the shape machinery does read.
// ---------------------------------------------------------------------------

#[test]
fn verify_output_never_contains_a_secret_planted_in_credential_files() {
    let home = TempDir::new("secrets");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    // One seat per interesting state, each carrying the sentinel as a JSON value and, for
    // the seats whose shape the diagnostics inspect, as a key name as well.
    plant_seat_credential(
        home.path(),
        "codex",
        "supervisor",
        &format!("{{\"{SENTINEL}\": \"{SENTINEL}\"}}\n"),
    );
    plant_seat_credential(
        home.path(),
        "claude",
        "lead",
        &format!("{{\"tokens\": {{\"access_token\": \"{SENTINEL}\"}}}}\n"),
    );
    let diverged = plant_seat_credential(
        home.path(),
        "pi",
        "peer",
        &format!("{{\"{SENTINEL}\": \"{SENTINEL}\"}}\n"),
    );
    fs::write(
        diverged.parent().unwrap().join("credentials.preserved.json"),
        "{\"len\": 4096, \"keys\": []}\n",
    )
    .unwrap();

    let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &["verify"]);
    let stdout = stdout_of(&out);
    let stderr = stderr_of(&out);
    assert!(
        out.status.success(),
        "diagnostics stay non-blocking\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains(&diag_line("codex", "supervisor", "configured structurally"))
            && stdout.contains(&diag_line("claude", "lead", "configured structurally"))
            && stdout.contains(&diag_line("pi", "peer", "diverged-file-preserve")),
        "the diagnostics must still classify the planted seats:\n{stdout}"
    );
    assert!(
        !stdout.contains(SENTINEL),
        "the planted sentinel secret must never appear in verify stdout:\n{stdout}"
    );
    assert!(
        !stderr.contains(SENTINEL),
        "the planted sentinel secret must never appear in verify stderr:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// Acceptance criterion 5: diagnostics are additive to ticket 03 — a failing hygiene closure
// still makes verify exit non-zero even when every auth diagnostic is clean.
// ---------------------------------------------------------------------------

#[test]
fn a_failing_hygiene_closure_keeps_verify_non_zero_despite_clean_diagnostics() {
    let home = TempDir::new("additive");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    // Every seat carries a structurally complete credential: all nine diagnostics clean.
    for agent in AGENTS {
        for role in ROLES {
            plant_seat_credential(
                home.path(),
                agent,
                role,
                &format!("{{\"tokens\": {{\"access_token\": \"{SENTINEL}\"}}}}\n"),
            );
        }
    }
    // One sabotaged hygiene closure (a deny entry removed from a Claude seat).
    let settings_path = home.path().join("roles/claude/supervisor/settings.json");
    let mut settings = read_json(&settings_path);
    let denied: Vec<serde_json::Value> = settings["disallowedTools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool.as_str() != Some("Cron"))
        .cloned()
        .collect();
    settings["disallowedTools"] = serde_json::Value::Array(denied);
    fs::write(&settings_path, serde_json::to_string_pretty(&settings).unwrap() + "\n").unwrap();

    let out = run_hotpot(home.path(), fake_home.path(), Some(bin.path()), &["verify"]);
    assert!(
        !out.status.success(),
        "a failing hygiene closure must keep verify non-zero even with clean diagnostics"
    );
    let stderr = stderr_of(&out);
    assert!(
        stderr.contains("claude-deny-list"),
        "verify stderr must still name the failing hygiene check:\n{stderr}"
    );
    let stdout = stdout_of(&out);
    for agent in AGENTS {
        for role in ROLES {
            assert!(
                stdout.contains(&diag_line(agent, role, "configured structurally")),
                "every clean diagnostic must still be reported:\n{stdout}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The preserved shape record lifecycle: `hotpot auth login` preserves the fresh
// credential's shape beside it, giving `diverged-file-preserve` a real baseline.
// ---------------------------------------------------------------------------

/// Writes a fake `codex` vendor shim whose "login" drops a fixture credential into
/// `$CODEX_HOME/auth.json` and exits 0.
fn write_fake_codex_login(bin_dir: &Path) {
    fs::create_dir_all(bin_dir).unwrap();
    let script = "#!/bin/sh\ncat > \"$CODEX_HOME/auth.json\" <<'EOF'\n{\"tokens\": {\"access_token\": \"vendor-token\"}}\nEOF\nexit 0\n";
    let shim = bin_dir.join("codex");
    fs::write(&shim, script).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn auth_login_preserves_the_credential_shape_and_detects_later_divergence() {
    let home = TempDir::new("preserve");
    let fake_home = TempDir::new("fakehome");
    let bin = TempDir::new("bin");
    write_fake_pi(bin.path(), &["builtin:mcp"], 0);
    write_fake_codex_login(bin.path());
    apply_clean(home.path(), fake_home.path(), Some(bin.path()));

    let out = run_hotpot(
        home.path(),
        fake_home.path(),
        Some(bin.path()),
        &["auth", "login", "codex", "lead"],
    );
    assert!(
        out.status.success(),
        "auth login codex lead must exit 0\nstdout:\n{}\nstderr:\n{}",
        stdout_of(&out),
        stderr_of(&out)
    );

    let credential = seat_credential(home.path(), "codex", "lead");
    assert!(credential.exists(), "the vendor shim must have written the credential");
    let record_path = credential.parent().unwrap().join("credentials.preserved.json");
    let record_text = fs::read_to_string(&record_path)
        .unwrap_or_else(|e| panic!("login must preserve a shape record beside the credential: {e}"));
    let record = read_json(&record_path);
    assert_eq!(
        record["len"].as_u64(),
        Some(fs::read(&credential).unwrap().len() as u64),
        "the preserved record must carry the credential's byte length:\n{record_text}"
    );
    let keys: Vec<&str> = record["keys"]
        .as_array()
        .expect("the preserved record carries the recursive key names")
        .iter()
        .map(|k| k.as_str().unwrap())
        .collect();
    assert!(
        keys.contains(&"tokens") && keys.contains(&"tokens.access_token"),
        "the preserved record must carry the credential's key shape:\n{record_text}"
    );
    assert!(
        !record_text.contains("vendor-token"),
        "the preserved record stores shape only, never the token value:\n{record_text}"
    );

    // Steady state: the logged-in seat verifies as configured structurally.
    let stdout = verify_green(home.path(), fake_home.path(), bin.path());
    assert!(
        stdout.contains(&diag_line("codex", "lead", "configured structurally")),
        "a seat whose credential matches its preserved shape is configured structurally:\n{stdout}"
    );

    // Tamper: the live credential leaves its preserved shape.
    fs::write(
        &credential,
        "{\"tokens\": {\"access_token\": \"rotated\"}, \"extra\": true}\n",
    )
    .unwrap();
    let stdout = verify_green(home.path(), fake_home.path(), bin.path());
    assert!(
        stdout.contains(&diag_line("codex", "lead", "diverged-file-preserve")),
        "a credential that no longer matches its preserved shape is reported diverged:\n{stdout}"
    );
}
