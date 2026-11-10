//! Seam tests for the minimal `hotpot profile` write path (ticket 13): the
//! right-hand panel's inline `agent/model · thinking` pills and named presets
//! persist role profile defaults in Hotpot Home through
//! `hotpot profile set <agent> <role> <model> --thinking <effort>`, reusing the
//! provisioner's atomic write engine (ticket 01).
//!
//! Seam (ticket 13 "Testing seam"): the `hotpot` CLI from `cargo test` against
//! a temp `HOTPOT_HOME`; assert exit codes, stdout/stderr, and the
//! `<HOTPOT_HOME>/profiles.json` file `hotpot status` reads back
//! (`src/status.rs: read_profiles`). Zero network; no private-internal
//! assertions.

mod common;

use common::TempDir;
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;

/// Runs `hotpot <args..>` against `home` and returns the raw output.
fn run_hotpot(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .args(args)
        .env("HOTPOT_HOME", home)
        .output()
        .expect("spawn hotpot CLI")
}

/// Parses `<home>/profiles.json`, asserting it exists and is a JSON object.
fn profiles_json(home: &Path) -> Value {
    let raw = fs::read_to_string(home.join("profiles.json"))
        .unwrap_or_else(|e| panic!("profiles.json exists after profile set: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("profiles.json is JSON: {e}\n{raw}"))
}

#[test]
fn profile_set_writes_the_role_defaults_file_atomically() {
    let home = TempDir::new("home");
    let out = run_hotpot(
        home.path(),
        &["profile", "set", "codex", "lead", "gpt-5.1-codex", "--thinking", "high"],
    );
    assert!(
        out.status.success(),
        "profile set exits 0: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        profiles_json(home.path()),
        json!({"lead": {"agent": "codex", "model": "gpt-5.1-codex", "thinking": "high"}}),
        "the role profile defaults file carries the pill's payload"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("profile: set codex/lead"),
        "stdout names the applied profile: {stdout}"
    );
    // No leftover temporary sibling from the atomic write.
    let entries: Vec<String> = fs::read_dir(home.path())
        .unwrap()
        .filter_map(|e| e.unwrap().file_name().into_string().ok())
        .collect();
    assert!(
        entries.iter().all(|name| !name.contains("hotpot-tmp")),
        "no temp sibling remains: {entries:?}"
    );
}

#[test]
fn profile_set_merges_roles_and_preserves_unknown_fields() {
    let home = TempDir::new("home");
    // A pre-existing profile with another role and an unknown extra field.
    fs::write(
        home.path().join("profiles.json"),
        r#"{"supervisor": {"agent": "claude", "model": "claude-opus-4-6", "thinking": "high", "note": "keep-me"}}"#,
    )
    .unwrap();
    let out = run_hotpot(
        home.path(),
        &["profile", "set", "pi", "lead", "glm-4.7-air", "--thinking", "medium"],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));

    let profiles = profiles_json(home.path());
    assert_eq!(
        profiles["lead"],
        json!({"agent": "pi", "model": "glm-4.7-air", "thinking": "medium"})
    );
    // The untouched role (and its unknown field) survives the rewrite.
    assert_eq!(profiles["supervisor"]["agent"], json!("claude"));
    assert_eq!(profiles["supervisor"]["note"], json!("keep-me"));
}

#[test]
fn profile_set_refuses_ultra_and_ultracode_efforts_without_touching_the_file() {
    let home = TempDir::new("home");
    for effort in ["ultra", "ultracode"] {
        let out = run_hotpot(
            home.path(),
            &["profile", "set", "claude", "peer", "claude-haiku-4-5", "--thinking", effort],
        );
        assert!(
            !out.status.success(),
            "`{effort}` must be refused (exit {})",
            out.status
        );
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("refused"),
            "stderr names the refusal: {stderr}"
        );
        assert!(
            !home.path().join("profiles.json").exists(),
            "a refused effort writes nothing"
        );
    }
}

#[test]
fn profile_set_rejects_unknown_agents_roles_and_bad_usage() {
    let home = TempDir::new("home");
    for args in [
        vec!["profile", "set", "zcode", "lead", "whatever", "--thinking", "low"],
        vec!["profile", "set", "claude", "owner", "claude-haiku-4-5", "--thinking", "low"],
        vec!["profile", "set", "claude", "lead", "claude-haiku-4-5", "--thinking", "extreme"],
        vec!["profile", "set"],
        vec!["profile"],
    ] {
        let out = run_hotpot(home.path(), &args);
        assert!(!out.status.success(), "{args:?} must fail");
        assert!(
            !home.path().join("profiles.json").exists(),
            "{args:?} writes nothing"
        );
    }
}

#[test]
fn profile_defaults_written_by_the_cli_reach_hotpot_status() {
    let home = TempDir::new("home");
    let workspace = TempDir::new("ws");
    let out = run_hotpot(
        home.path(),
        &["profile", "set", "codex", "lead", "gpt-5.1-codex", "--thinking", "high"],
    );
    assert!(out.status.success());

    // The read side (`hotpot status` via read_profiles) serves the written
    // defaults: a lead seat with no dispatch record picks them up. No hc
    // anywhere, so the ledger is the only source.
    let out = Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .args(["status", "--workspace", workspace.path().to_str().unwrap()])
        .env("HOTPOT_HOME", home.path())
        .env("HOTPOT_HC", home.path().join("missing-hc"))
        .output()
        .expect("spawn hotpot status");
    assert!(out.status.success());
    let status: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(status["seats"], json!([]), "no seats launched yet");
    assert_eq!(profiles_json(home.path())["lead"]["model"], json!("gpt-5.1-codex"));
}
