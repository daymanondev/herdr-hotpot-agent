//! Seam tests for background `gate_run`, Reviewer dispatch, manual
//! `assignment_accept`, and secret masking on gate tails and `hotpot export`
//! (spec user story 25, "Secret Masking on Gate Tails & Exports"; ticket 10).
//!
//! Fake gate shell scripts committed at the repo base are re-executed by the
//! detached Gate runner against a handed-back `Candidate`; tests assert ledger
//! events, masked tail files under `HOTPOT_HOME`, and the export bundle.
//! Zero network; no private-internal assertions.

mod common;

use common::{
    McpProcess, TempDir, bash_payload, commit_file, create_assignment, git, init_git_repo,
    ledger_lines, peer_hook, valid_create, write_payload,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The message of a JSON-RPC error response, asserting one is present.
fn error_message(resp: &Value) -> String {
    resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON-RPC error: {resp}"))
        .to_string()
}

/// Events of one type recorded in the project ledger.
fn events_of_type(home: &Path, kind: &str) -> Vec<Value> {
    ledger_lines(home)
        .into_iter()
        .filter(|e| e["type"] == json!(kind))
        .collect()
}

/// Writes an executable shell script into `repo`.
fn write_exec(repo: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    let file = repo.join(name);
    std::fs::write(&file, body).unwrap();
    let mut perms = std::fs::metadata(&file).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&file, perms).unwrap();
}

/// A fixture repository with the fake gate suite committed at the base:
/// a green gate, a red gate that prints fixture secrets, and a hanging gate.
fn init_gate_repo() -> (TempDir, String) {
    let (repo, _head) = init_git_repo();
    let path = repo.path();
    write_exec(
        path,
        "gate-green.sh",
        "#!/bin/sh\necho 'gate: all checks passed'\nexit 0\n",
    );
    write_exec(
        path,
        "gate-red.sh",
        concat!(
            "#!/bin/sh\n",
            "echo 'gate: build failed'\n",
            "echo 'token sk-proj-FIXTURE_API_KEY_0123456789abcdef leaked in logs'\n",
            "cat <<'EOF'\n",
            "-----BEGIN PRIVATE KEY-----\n",
            "MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKYwggSiAgEAAoIBAQC0fixture\n",
            "-----END PRIVATE KEY-----\n",
            "EOF\n",
            "exit 1\n",
        ),
    );
    write_exec(
        path,
        "gate-hang.sh",
        "#!/bin/sh\necho 'gate: hanging'\nsleep 60\nexit 1\n",
    );
    git(path, &["add", "-A"]);
    git(path, &["commit", "-qm", "fixture: fake gate scripts"]);
    let head = git(path, &["rev-parse", "HEAD"]);
    (repo, head)
}

/// Creates and dispatches a worktree-isolated Engineer (optionally on an
/// explicit agent family), records write + gate evidence through the hook
/// seam, commits, and hands back; returns (assignment id, worktree, candidate).
fn engineer_hands_back(
    lead: &mut McpProcess,
    repo: &Path,
    home: &Path,
    head: &str,
    gate: &str,
    correlation: &str,
    agent: Option<&str>,
) -> (String, String, String) {
    let mut args = valid_create(head);
    args["gate"] = json!(gate);
    args["writeScope"] = json!(["src/**"]);
    args["serialOnly"] = json!([]);
    let id = create_assignment(lead, args);
    let mut dispatch =
        json!({"assignment": id, "correlation": correlation, "isolation": "worktree"});
    if let Some(agent) = agent {
        dispatch["agent"] = json!(agent);
    }
    let dispatched = lead.tools_call(json!("eng-dispatch"), "assignment_dispatch", dispatch);
    assert!(
        dispatched.get("error").is_none(),
        "worktree dispatch must succeed: {dispatched}"
    );
    let worktree = dispatched["result"]["lease"]["worktree"]
        .as_str()
        .unwrap_or_else(|| panic!("dispatch reports the worktree: {dispatched}"))
        .to_string();
    let worktree_path = PathBuf::from(&worktree);

    let write = peer_hook(
        home,
        correlation,
        &write_payload(&worktree_path.join("src/engine.rs")),
    );
    assert_eq!(
        write["hookSpecificOutput"]["permissionDecision"], "allow",
        "peer write in its own worktree is allowed: {write}"
    );
    let candidate = commit_file(&worktree_path, "src/engine.rs", "pub fn ship() {}\n");
    let gate_ran = peer_hook(home, correlation, &bash_payload(gate));
    assert_eq!(
        gate_ran["hookSpecificOutput"]["permissionDecision"], "allow",
        "peer gate run is allowed: {gate_ran}"
    );

    let mut peer = McpProcess::spawn_correlated("peer", repo, home, Some(correlation));
    let handed = peer.tools_call(
        json!(1),
        "handoff",
        json!({"summary": "slice shipped, gate green locally"}),
    );
    assert!(
        handed.get("error").is_none(),
        "clean-tree handoff must succeed: {handed}"
    );
    assert_eq!(handed["result"]["state"], "HandedBack");
    (id, worktree, candidate)
}

/// Polls the ledger until the detached runner records a `gate_completed`
/// beyond the `prev` completions already observed; returns the newest one.
fn wait_for_gate_completion(home: &Path, prev: usize, secs: u64) -> Option<Value> {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let mut completions = events_of_type(home, "gate_completed");
        if completions.len() > prev {
            return completions.pop();
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// Starts `gate_run` (optionally with an injected timeout), asserts it started,
/// and waits for the completion event beyond any earlier completions.
fn gate_run_and_wait(
    lead: &mut McpProcess,
    home: &Path,
    id: &str,
    timeout_ms: Option<u64>,
) -> Value {
    let prev = events_of_type(home, "gate_completed").len();
    let mut args = json!({"assignment": id});
    if let Some(ms) = timeout_ms {
        args["timeoutMs"] = json!(ms);
    }
    let started = lead.tools_call(json!("gate"), "gate_run", args);
    assert!(
        started.get("error").is_none(),
        "gate_run must succeed: {started}"
    );
    assert_eq!(
        started["result"]["state"], "GateRunning",
        "gate_run moves HandedBack to GateRunning: {started}"
    );
    wait_for_gate_completion(home, prev, 15)
        .expect("the detached gate runner records gate_completed")
}

#[test]
fn green_gate_completion_is_recorded_without_auto_accepting() {
    let home = TempDir::new("home");
    let (repo, head) = init_gate_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, _worktree, _candidate) = engineer_hands_back(
        &mut lead,
        repo.path(),
        home.path(),
        &head,
        "./gate-green.sh",
        "corr-green",
        None,
    );

    let completed = gate_run_and_wait(&mut lead, home.path(), &id, None);
    assert_eq!(completed["assignment"], json!(id));
    assert_eq!(completed["verdict"], json!("green"));
    assert_eq!(completed["exitCode"], json!(0));

    // Nothing is ever auto-accepted on a green gate: the assignment is still
    // HandedBack, now carrying the verdict, and no accept event exists.
    let status = lead.tools_call(
        json!(11),
        "assignment_status",
        json!({"assignment": id}),
    );
    assert_eq!(
        status["result"]["state"], "HandedBack",
        "a green gate never auto-accepts: {status}"
    );
    assert_eq!(status["result"]["gateVerdict"], json!("green"));
    assert!(
        events_of_type(home.path(), "assignment_accepted").is_empty(),
        "no acceptance is recorded without an explicit assignment_accept"
    );

    // The same completion is observable on the Supervisor findings surface.
    let mut supervisor = McpProcess::spawn("supervisor", repo.path(), home.path());
    let findings = supervisor.tools_call(json!(1), "runtime_findings", json!({}));
    assert!(
        findings.get("error").is_none(),
        "runtime_findings must answer: {findings}"
    );
    let text = serde_json::to_string(&findings["result"]).unwrap();
    assert!(
        text.contains("\"verdict\":\"green\"") && text.contains(&id),
        "findings carry the gate verdict for the assignment: {text}"
    );
}

#[test]
fn red_gate_writes_a_masked_and_deterministic_tail_under_hotpot_home() {
    let home = TempDir::new("home");
    let (repo, head) = init_gate_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, _worktree, _candidate) = engineer_hands_back(
        &mut lead,
        repo.path(),
        home.path(),
        &head,
        "./gate-red.sh",
        "corr-red",
        None,
    );

    let completed = gate_run_and_wait(&mut lead, home.path(), &id, None);
    assert_eq!(completed["verdict"], json!("red"));
    assert_eq!(completed["exitCode"], json!(1));

    // The masked tail lives under HOTPOT_HOME and is referenced by the event.
    let tail = completed["tail"]
        .as_str()
        .unwrap_or_else(|| panic!("red completion references its tail: {completed}"))
        .to_string();
    assert!(
        tail.starts_with(home.path().as_os_str().to_str().unwrap()),
        "tail is written under HOTPOT_HOME: {tail}"
    );
    let first = std::fs::read_to_string(&tail).unwrap();

    // Fixture secrets (an sk- API key and a PEM private-key block) are masked.
    assert!(
        first.contains("{{SECRET_api_key_"),
        "the sk- key becomes an api_key placeholder: {first}"
    );
    assert!(
        first.contains("{{SECRET_private_key_"),
        "the PEM block becomes a private_key placeholder: {first}"
    );
    assert!(
        !first.contains("sk-proj-FIXTURE_API_KEY") && !first.contains("0123456789abcdef"),
        "the raw API key never reaches the tail: {first}"
    );
    assert!(
        !first.contains("BEGIN PRIVATE KEY") && !first.contains("MIIEvQIBADANBgkqhkiG9w0BAQEF"),
        "the raw private-key block never reaches the tail: {first}"
    );
    assert!(
        first.contains("gate: build failed"),
        "non-secret diagnostics survive masking: {first}"
    );

    // Deterministic: the same gate output yields byte-identical placeholders.
    let second_completed = gate_run_and_wait(&mut lead, home.path(), &id, None);
    let second_tail = second_completed["tail"].as_str().unwrap().to_string();
    assert_ne!(tail, second_tail, "each run writes its own tail file");
    let second = std::fs::read_to_string(&second_tail).unwrap();
    assert_eq!(
        first, second,
        "the same input yields byte-identical masked tails"
    );
}

#[test]
fn a_hanging_gate_with_an_injected_short_timeout_is_uncertain() {
    let home = TempDir::new("home");
    let (repo, head) = init_gate_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, _worktree, _candidate) = engineer_hands_back(
        &mut lead,
        repo.path(),
        home.path(),
        &head,
        "./gate-hang.sh",
        "corr-hang",
        None,
    );

    // The gate hangs; Lead injects a short timeout through gate_run.
    let completed = gate_run_and_wait(&mut lead, home.path(), &id, Some(400));
    assert_eq!(completed["verdict"], json!("uncertain"));
    assert_eq!(completed["reason"], json!("timeout"));
    assert!(
        completed.get("exitCode").is_none(),
        "a timed-out gate has no exit code: {completed}"
    );

    // The uncertain verdict is visible on the assignment and nothing was accepted.
    let status = lead.tools_call(
        json!(11),
        "assignment_status",
        json!({"assignment": id}),
    );
    assert_eq!(status["result"]["state"], "HandedBack");
    assert_eq!(status["result"]["gateVerdict"], json!("uncertain"));
    assert_eq!(status["result"]["gateReason"], json!("timeout"));
    assert!(events_of_type(home.path(), "assignment_accepted").is_empty());

    // A non-positive timeout is an argument error, not a run.
    let lines_before = common::ledger_line_count(home.path());
    let refused = lead.tools_call(
        json!(12),
        "gate_run",
        json!({"assignment": id, "timeoutMs": 0}),
    );
    let message = error_message(&refused);
    assert!(
        message.contains("timeoutMs"),
        "error names the invalid timeout argument: {message}"
    );
    assert_eq!(
        common::ledger_line_count(home.path()),
        lines_before,
        "a refused gate_run appends nothing"
    );
}

#[test]
fn accept_is_explicit_and_closed_follows_workspace_close() {
    let home = TempDir::new("home");
    let (repo, head) = init_gate_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());

    // Accepting a Dispatched assignment (no handoff) is a JSON-RPC error.
    let mut early = valid_create(&head);
    early["gate"] = json!("./gate-green.sh");
    early["writeScope"] = json!(["docs/**"]);
    early["serialOnly"] = json!([]);
    let early_id = create_assignment(&mut lead, early);
    lead.tools_call(
        json!(1),
        "assignment_dispatch",
        json!({"assignment": early_id, "isolation": "worktree"}),
    );
    let refused = lead.tools_call(
        json!(2),
        "assignment_accept",
        json!({"assignment": early_id}),
    );
    let message = error_message(&refused);
    assert!(
        message.contains("HandedBack"),
        "accept before any handoff names the required state: {message}"
    );

    // Full path: valid handoff, green gate, explicit accept, workspace close.
    let (id, worktree, _candidate) = engineer_hands_back(
        &mut lead,
        repo.path(),
        home.path(),
        &head,
        "./gate-green.sh",
        "corr-accept",
        None,
    );
    gate_run_and_wait(&mut lead, home.path(), &id, None);

    let accepted = lead.tools_call(
        json!(10),
        "assignment_accept",
        json!({"assignment": id, "note": "candidate integrated manually"}),
    );
    assert!(
        accepted.get("error").is_none(),
        "accept after a green gate: {accepted}"
    );
    assert_eq!(accepted["result"]["state"], "Accepted");

    // Closed follows once the peer workspace is archived via workspace_close.
    let closed = lead.tools_call(
        json!(11),
        "workspace_close",
        json!({"assignment": id}),
    );
    assert!(
        closed.get("error").is_none(),
        "workspace_close after accept: {closed}"
    );
    assert_eq!(closed["result"]["state"], "Closed");
    assert_eq!(closed["result"]["lease"]["released"], json!(true));
    let closed_events = events_of_type(home.path(), "assignment_closed");
    assert_eq!(
        closed_events.len(),
        1,
        "workspace_close finalizes the Accepted assignment as Closed"
    );
    assert_eq!(closed_events[0]["assignment"], json!(id));
    assert!(
        !Path::new(&worktree).exists(),
        "the accepted assignment's worktree is cleaned up on close"
    );
}

#[test]
fn reviewer_dispatch_is_read_only_bound_to_the_candidate_on_another_family() {
    let home = TempDir::new("home");
    let (repo, head) = init_gate_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());

    // The engineer peer hands back its candidate from a codex-family seat.
    let (eng_id, _worktree, candidate) = engineer_hands_back(
        &mut lead,
        repo.path(),
        home.path(),
        &head,
        "./gate-green.sh",
        "corr-eng",
        Some("codex"),
    );
    let eng_status = lead.tools_call(
        json!(10),
        "assignment_status",
        json!({"assignment": eng_id}),
    );
    assert_eq!(eng_status["result"]["agent"], json!("codex"));

    // Lead dispatches a read-only Reviewer against the immutable candidate,
    // requesting a different agent family than the engineer's.
    let mut reviewer = valid_create(&candidate);
    reviewer["outcome"] = json!("Independently verify the candidate");
    reviewer["disposition"] = json!("Reviewer");
    reviewer["writeScope"] = json!([]);
    reviewer["serialOnly"] = json!([]);
    reviewer["gate"] = json!("./gate-green.sh");
    let rev_id = create_assignment(&mut lead, reviewer);
    let dispatched = lead.tools_call(
        json!(11),
        "assignment_dispatch",
        json!({
            "assignment": rev_id,
            "correlation": "corr-reviewer",
            "agent": "claude",
            "model": "claude-opus-4-6",
        }),
    );
    assert!(
        dispatched.get("error").is_none(),
        "reviewer dispatch must succeed: {dispatched}"
    );
    assert_eq!(dispatched["result"]["state"], "Dispatched");
    assert_eq!(dispatched["result"]["baseCommit"], json!(candidate));
    assert!(
        dispatched["result"].get("lease").is_none(),
        "a reviewer holds no writer lease: {dispatched}"
    );
    assert_eq!(
        dispatched["result"]["agent"], json!("claude"),
        "the requested reviewer family is recorded: {dispatched}"
    );
    assert_eq!(dispatched["result"]["model"], json!("claude-opus-4-6"));
    let eng_status = lead.tools_call(
        json!(12),
        "assignment_status",
        json!({"assignment": eng_id}),
    );
    assert_ne!(
        dispatched["result"]["agent"], eng_status["result"]["agent"],
        "the reviewer's family differs from the engineer's when requested"
    );

    // The reviewer's later handoff (09) requires only a clean tree.
    let mut reviewer_seat =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-reviewer"));
    let handed = reviewer_seat.tools_call(
        json!(1),
        "handoff",
        json!({"summary": "review complete: candidate verified read-only"}),
    );
    assert!(
        handed.get("error").is_none(),
        "clean-tree read-only reviewer handoff must succeed: {handed}"
    );
    assert_eq!(handed["result"]["state"], "HandedBack");
    assert_eq!(handed["result"]["readOnly"], json!(true));
    assert!(
        events_of_type(home.path(), "writer_lease_acquired")
            .iter()
            .all(|e| e["assignment"] != json!(rev_id)),
        "the reviewer never acquired a writer lease"
    );

    // An unverified agent family is refused without a ledger append.
    let mut scout = valid_create(&candidate);
    scout["disposition"] = json!("Scout");
    scout["writeScope"] = json!([]);
    scout["serialOnly"] = json!([]);
    scout["gate"] = json!("./gate-green.sh");
    let scout_id = create_assignment(&mut lead, scout);
    let lines_before = common::ledger_line_count(home.path());
    let refused = lead.tools_call(
        json!(13),
        "assignment_dispatch",
        json!({"assignment": scout_id, "agent": "gemini"}),
    );
    let message = error_message(&refused);
    assert!(
        message.contains("gemini") && message.contains("codex|claude|pi"),
        "error names the refused family and the verified ones: {message}"
    );
    assert_eq!(
        common::ledger_line_count(home.path()),
        lines_before,
        "the refused dispatch appends nothing"
    );
}

#[test]
fn export_writes_a_masked_bundle_of_runtime_state_to_an_explicit_path() {
    let home = TempDir::new("home");
    let out_dir = TempDir::new("out");
    let (repo, head) = init_gate_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, _worktree, _candidate) = engineer_hands_back(
        &mut lead,
        repo.path(),
        home.path(),
        &head,
        "./gate-red.sh",
        "corr-export",
        None,
    );
    // A red gate leaves a masked tail; a fixture secret also leaks into the
    // runtime transcript (a peer question in the ledger).
    gate_run_and_wait(&mut lead, home.path(), &id, None);
    common::seed_ledger_event(
        home.path(),
        json!({
            "type": "assignment_asked",
            "assignment": id,
            "question": "should I commit the key sk-proj-FIXTURE_API_KEY_0123456789abcdef here?",
        }),
    );

    let out = out_dir.path().join("hotpot-export.json");
    let export = std::process::Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .arg("export")
        .arg(repo.path())
        .arg("--out")
        .arg(&out)
        .env("HOTPOT_HOME", home.path())
        .output()
        .expect("spawn hotpot export");
    assert!(
        export.status.success(),
        "hotpot export exits 0: {}",
        String::from_utf8_lossy(&export.stderr)
    );
    assert!(out.is_file(), "the bundle is written to the explicit path");

    let bundle = std::fs::read_to_string(&out).unwrap();
    let parsed: Value = serde_json::from_str(&bundle)
        .unwrap_or_else(|e| panic!("the masked bundle stays valid JSON: {e}"));
    assert_eq!(
        parsed["events"].as_array().map(Vec::len).unwrap_or(0),
        common::ledger_lines(home.path()).len(),
        "the bundle carries every ledger event"
    );
    assert!(
        parsed["assignments"]
            .as_array()
            .is_some_and(|a| a.iter().any(|asg| asg["id"] == json!(id))),
        "the bundle carries the assignment projections"
    );

    // Masking applies across the whole bundle: transcript and gate output.
    assert!(
        bundle.contains("{{SECRET_api_key_"),
        "the leaked API key is masked in the bundle"
    );
    assert!(
        !bundle.contains("sk-proj-FIXTURE_API_KEY") && !bundle.contains("0123456789abcdef"),
        "no raw API key survives in the bundle"
    );
    assert!(
        !bundle.contains("BEGIN PRIVATE KEY") && !bundle.contains("MIIEvQIBADANBgkqhkiG9w0BAQEF"),
        "no raw private-key block survives in the bundle"
    );

    // Exporting an unknown project is an explicit error, not an empty bundle.
    let missing = std::process::Command::new(env!("CARGO_BIN_EXE_hotpot"))
        .arg("export")
        .arg("no-such-project-slug")
        .arg("--out")
        .arg(out_dir.path().join("missing.json"))
        .env("HOTPOT_HOME", home.path())
        .output()
        .expect("spawn hotpot export");
    assert!(
        !missing.status.success(),
        "exporting an unknown project exits non-zero"
    );
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("no-such-project-slug"),
        "the error names the unknown project"
    );
    assert!(!out_dir.path().join("missing.json").exists());
}

#[test]
fn gate_run_falls_back_to_a_detached_checkout_when_the_workspace_is_gone() {
    let home = TempDir::new("home");
    let (repo, head) = init_gate_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, worktree, candidate) = engineer_hands_back(
        &mut lead,
        repo.path(),
        home.path(),
        &head,
        "./gate-green.sh",
        "corr-fallback",
        None,
    );

    // The peer's worktree disappears before Lead re-runs the gate; the gate
    // still runs against the immutable candidate via a detached checkout.
    std::fs::remove_dir_all(&worktree).unwrap();
    assert!(!Path::new(&worktree).exists());

    let completed = gate_run_and_wait(&mut lead, home.path(), &id, None);
    assert_eq!(completed["assignment"], json!(id));
    assert_eq!(completed["verdict"], json!("green"));
    let status = lead.tools_call(
        json!(11),
        "assignment_status",
        json!({"assignment": id}),
    );
    assert_eq!(status["result"]["gateVerdict"], json!("green"));
    assert_eq!(status["result"]["candidate"], json!(candidate));

    // The throwaway checkout is cleaned up: only the gates dir remains, with
    // no per-run scratch files for a green gate.
    let slug_dir = common::ledger_path(home.path())
        .parent()
        .unwrap()
        .to_path_buf();
    let gates_dir = slug_dir.join("gates").join(&id);
    assert!(gates_dir.is_dir(), "the gates dir lives under HOTPOT_HOME");
    let leftovers: Vec<String> = std::fs::read_dir(&gates_dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert!(
        leftovers.is_empty(),
        "a green gate leaves no tails or checkouts behind: {leftovers:?}"
    );
}
