//! Seam tests for the Peer `ask` / `handoff` surface: identity, assignment, and
//! workspace derived server-side from the session correlation, Challenge Signal
//! routing, and writable handoffs validated against live Git `baseCommit..Candidate`
//! evidence before they are believed (spec user stories 23-24; ticket 09).
//!
//! Two MCP stdio sessions (Lead and Peer correlations) run against a temp real git
//! repo/worktree and temp `HOTPOT_HOME`; real `git` commits are driven in the peer
//! workspace; runtime evidence is captured through the `hotpot hook` seam. Zero
//! network; no private-internal assertions.

mod common;

use common::{
    McpProcess, TempDir, bash_payload, commit_file, create_assignment, init_git_repo,
    ledger_lines, peer_hook, valid_create, write_payload,
};
use serde_json::{Value, json};
use std::path::Path;

/// The message of a JSON-RPC error response, asserting one is present.
fn error_message(resp: &Value) -> String {
    resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON-RPC error: {resp}"))
        .to_string()
}

/// Events of one type recorded in the project ledger.
fn ledger_events_of_type(home: &Path, kind: &str) -> Vec<Value> {
    ledger_lines(home)
        .into_iter()
        .filter(|e| e["type"] == json!(kind))
        .collect()
}

/// Creates and dispatches a writable worktree-mode assignment bound to
/// `correlation`; returns (assignment id, worktree path).
fn dispatch_worktree_peer(
    lead: &mut McpProcess,
    head: &str,
    write_scope: &[&str],
    correlation: &str,
) -> (String, String) {
    let mut args = valid_create(head);
    args["outcome"] = json!("Ship a correlated vertical slice");
    args["writeScope"] = json!(write_scope);
    args["serialOnly"] = json!([]);
    let id = create_assignment(lead, args);
    let dispatched = lead.tools_call(
        json!("dispatch"),
        "assignment_dispatch",
        json!({"assignment": id, "correlation": correlation, "isolation": "worktree"}),
    );
    assert!(
        dispatched.get("error").is_none(),
        "worktree dispatch must succeed: {dispatched}"
    );
    let worktree = dispatched["result"]["lease"]["worktree"]
        .as_str()
        .unwrap_or_else(|| panic!("dispatch reports the worktree: {dispatched}"))
        .to_string();
    (id, worktree)
}

// ---------------------------------------------------------------------------
// Slice 1: server-derived identity & the `ask` round-trip
// ---------------------------------------------------------------------------

#[test]
fn tools_list_with_a_peer_correlation_returns_exactly_ask_and_handoff() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    let mut peer = McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-x"));

    let list = peer.request(json!(1), "tools/list", json!({}));
    let names: Vec<&str> = list["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["ask", "handoff"]);
}

#[test]
fn peer_calls_supplying_identity_in_args_are_jsonrpc_errors() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, _worktree) = dispatch_worktree_peer(&mut lead, &head, &["src/**"], "corr-id");

    let mut peer = McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-id"));
    let lines_before = common::ledger_line_count(home.path());

    // Every identity-bearing key in ask/handoff args is refused: the server
    // derives identity, assignment, and workspace from the session correlation.
    for tool in ["ask", "handoff"] {
        for key in ["assignment", "assignment_id", "seat", "workspace", "correlation"] {
            let mut args = json!({"question": "why?", "summary": "done"});
            args[key] = json!(id);
            let resp = peer.tools_call(json!(1), tool, args);
            let message = error_message(&resp);
            assert!(
                message.contains(key) && message.contains("server"),
                "{tool} with `{key}` must name the refused key and the server-derived binding: {message}"
            );
        }
    }
    assert_eq!(
        common::ledger_line_count(home.path()),
        lines_before,
        "identity-arg refusals append nothing to the ledger"
    );
}

#[test]
fn peer_without_a_correlation_cannot_ask_or_handoff() {
    let home = TempDir::new("home");
    let (repo, _head) = init_git_repo();
    let mut peer = McpProcess::spawn("peer", repo.path(), home.path());

    let resp = peer.tools_call(json!(1), "ask", json!({"question": "who am I?"}));
    let message = error_message(&resp);
    assert!(
        message.contains("correlation"),
        "error names the missing correlation pin: {message}"
    );

    let resp = peer.tools_call(json!(2), "handoff", json!({"summary": "done"}));
    assert!(
        error_message(&resp).contains("correlation"),
        "handoff without a correlation is refused too"
    );
}

#[test]
fn ask_with_an_unbound_correlation_or_bad_signal_errors() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (_id, _worktree) = dispatch_worktree_peer(&mut lead, &head, &["src/**"], "corr-bound");

    // A correlation no assignment was ever dispatched with is refused.
    let mut stranger =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-ghost"));
    let resp = stranger.tools_call(json!(1), "ask", json!({"question": "hello?"}));
    let message = error_message(&resp);
    assert!(
        message.contains("corr-ghost"),
        "error names the unbound correlation: {message}"
    );

    // An unknown Challenge Signal is refused without a ledger append.
    let mut peer =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-bound"));
    let lines_before = common::ledger_line_count(home.path());
    let resp = peer.tools_call(
        json!(2),
        "ask",
        json!({"question": "can I haz?", "signal": "PLEASE"}),
    );
    let message = error_message(&resp);
    assert!(
        message.contains("PLEASE") && message.contains("signal"),
        "error names the invalid Challenge Signal: {message}"
    );
    assert_eq!(common::ledger_line_count(home.path()), lines_before);
}

#[test]
fn ask_round_trip_routes_a_challenge_signal_to_lead_and_the_answer_back() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, _worktree) = dispatch_worktree_peer(&mut lead, &head, &["src/**"], "corr-ask");

    let mut peer =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-ask"));

    // The peer raises a structured BLOCKED Challenge Signal.
    let asked = peer.tools_call(
        json!(1),
        "ask",
        json!({
            "question": "the checkout fence blocks my write; is the lease mine?",
            "signal": "BLOCKED"
        }),
    );
    assert!(
        asked.get("error").is_none(),
        "peer ask with a valid signal must succeed: {asked}"
    );
    assert_eq!(asked["result"]["state"], "Asking");
    assert_eq!(asked["result"]["signal"], "BLOCKED");

    let asked_events = ledger_events_of_type(home.path(), "assignment_asked");
    assert_eq!(asked_events.len(), 1, "ask appends exactly one event");
    assert_eq!(asked_events[0]["assignment"], json!(id));
    assert_eq!(asked_events[0]["signal"], "BLOCKED");
    assert_eq!(
        asked_events[0]["question"],
        json!("the checkout fence blocks my write; is the lease mine?")
    );

    // The Lead session answers; the assignment returns to Dispatched carrying
    // the answer.
    let answered = lead.tools_call(
        json!(2),
        "assignment_answer",
        json!({"assignment": id, "answer": "yes, the lease is yours until handoff"}),
    );
    assert!(
        answered.get("error").is_none(),
        "lead answers the Asking assignment: {answered}"
    );
    assert_eq!(answered["result"]["state"], "Dispatched");
    assert_eq!(
        answered["result"]["lastAnswer"],
        "yes, the lease is yours until handoff"
    );

    // The peer's next status view reflects the answer.
    let next = peer.tools_call(json!(3), "ask", json!({"question": "understood, proceeding"}));
    assert_eq!(next["result"]["state"], "Asking");
    assert_eq!(
        next["result"]["lastAnswer"],
        "yes, the lease is yours until handoff"
    );
}

// ---------------------------------------------------------------------------
// Slice 2: writable `handoff` validated against live Git evidence
// ---------------------------------------------------------------------------

/// Records the full happy-path evidence chain of a peer that wrote, committed,
/// and then ran the assignment Gate (`./scripts/check.sh` from `valid_create`).
fn record_write_commit_gate_evidence(home: &Path, worktree: &Path, correlation: &str) -> String {
    let write = peer_hook(home, correlation, &write_payload(&worktree.join("src/engine.rs")));
    assert_eq!(
        write["hookSpecificOutput"]["permissionDecision"], "allow",
        "peer write in its own worktree is allowed: {write}"
    );
    let candidate = commit_file(worktree, "src/engine.rs", "pub fn ship() {}\n");
    let gate = peer_hook(home, correlation, &bash_payload("./scripts/check.sh"));
    assert_eq!(
        gate["hookSpecificOutput"]["permissionDecision"], "allow",
        "peer gate run is allowed: {gate}"
    );
    candidate
}

#[test]
fn handoff_happy_path_validates_git_evidence_and_hands_back() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, worktree) = dispatch_worktree_peer(&mut lead, &head, &["src/**"], "corr-happy");
    let worktree_path = Path::new(&worktree).to_path_buf();

    let candidate = record_write_commit_gate_evidence(home.path(), &worktree_path, "corr-happy");

    // The hook seam recorded minimal ledger evidence: one write, then one gate
    // run, in strictly increasing seq order.
    let writes = ledger_events_of_type(home.path(), "workspace_write");
    let gates = ledger_events_of_type(home.path(), "workspace_gate");
    assert_eq!(writes.len(), 1, "the Write tool call is recorded: {writes:?}");
    assert_eq!(writes[0]["assignment"], json!(id));
    assert_eq!(writes[0]["correlation"], json!("corr-happy"));
    assert_eq!(writes[0]["workspace"], json!(worktree));
    assert_eq!(gates.len(), 1, "the Gate run is recorded: {gates:?}");
    assert_eq!(gates[0]["gate"], json!("./scripts/check.sh"));
    assert!(
        gates[0]["seq"].as_u64().unwrap() > writes[0]["seq"].as_u64().unwrap(),
        "the gate evidence is newer than the write evidence"
    );

    let mut peer =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-happy"));
    let handed = peer.tools_call(
        json!(1),
        "handoff",
        json!({"summary": "engine slice shipped, gate green"}),
    );
    assert!(
        handed.get("error").is_none(),
        "clean-tree handoff with fresh gate evidence must succeed: {handed}"
    );
    assert_eq!(handed["result"]["state"], "HandedBack");
    assert_eq!(
        handed["result"]["candidate"], json!(candidate),
        "candidate equals the worktree HEAD"
    );
    assert_eq!(
        handed["result"]["range"],
        json!(format!("{head}..{candidate}")),
        "the result carries the baseCommit..candidate range"
    );
    assert_eq!(handed["result"]["scope"]["exceeded"], json!(false));

    // The Lead-side view confirms HandedBack with the unread-handback timestamp
    // (feeds the attention-letter slice).
    let status = lead.tools_call(json!(2), "assignment_status", json!({"assignment": id}));
    assert_eq!(status["result"]["state"], "HandedBack");
    assert_eq!(status["result"]["candidate"], json!(candidate));
    assert!(
        status["result"]["handedBackAt"].as_u64().is_some(),
        "the unread-handback timestamp is recorded: {status}"
    );
    let events = ledger_events_of_type(home.path(), "assignment_handed_back");
    assert_eq!(events.len(), 1, "exactly one handback event");
    assert_eq!(events[0]["assignment"], json!(id));
    assert_eq!(events[0]["candidate"], json!(candidate));
    assert_eq!(
        events[0]["summary"],
        json!("engine slice shipped, gate green")
    );
}

#[test]
fn handoff_with_a_dirty_tree_errors_and_the_assignment_stays_dispatched() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, worktree) = dispatch_worktree_peer(&mut lead, &head, &["src/**"], "corr-dirty");
    record_write_commit_gate_evidence(home.path(), Path::new(&worktree), "corr-dirty");

    // An uncommitted file dirties the peer workspace.
    std::fs::write(Path::new(&worktree).join("scratch-notes.txt"), "uncommitted\n").unwrap();

    let mut peer =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-dirty"));
    let refused = peer.tools_call(json!(1), "handoff", json!({"summary": "not so fast"}));
    let message = error_message(&refused);
    assert!(
        message.contains("not clean") && message.contains("scratch-notes.txt"),
        "error names the unclean state and the dirty entry: {message}"
    );

    let status = lead.tools_call(json!(2), "assignment_status", json!({"assignment": id}));
    assert_eq!(status["result"]["state"], "Dispatched");
    assert!(
        ledger_events_of_type(home.path(), "assignment_handed_back").is_empty(),
        "a refused handoff appends no handback event"
    );
}

#[test]
fn handoff_without_commits_errors_because_head_equals_base_commit() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, worktree) = dispatch_worktree_peer(&mut lead, &head, &["src/**"], "corr-empty");

    // Evidence exists (write + gate) but the worktree never moved past baseCommit.
    let worktree_path = Path::new(&worktree).to_path_buf();
    let write = peer_hook(
        home.path(),
        "corr-empty",
        &write_payload(&worktree_path.join("src/engine.rs")),
    );
    assert_eq!(write["hookSpecificOutput"]["permissionDecision"], "allow");
    let gate = peer_hook(home.path(), "corr-empty", &bash_payload("./scripts/check.sh"));
    assert_eq!(gate["hookSpecificOutput"]["permissionDecision"], "allow");

    let mut peer =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-empty"));
    let refused = peer.tools_call(json!(1), "handoff", json!({"summary": "nothing to show"}));
    let message = error_message(&refused);
    assert!(
        message.contains("baseCommit") && message.contains("commit"),
        "error names the missing Candidate commits: {message}"
    );
    let status = lead.tools_call(json!(2), "assignment_status", json!({"assignment": id}));
    assert_eq!(status["result"]["state"], "Dispatched");
    assert!(ledger_events_of_type(home.path(), "assignment_handed_back").is_empty());
}

#[test]
fn handoff_requires_the_gate_to_have_run_after_the_last_write() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());

    // (a) The Gate never ran.
    let (id_a, worktree_a) = dispatch_worktree_peer(&mut lead, &head, &["src/**"], "corr-nogate");
    let wa = Path::new(&worktree_a).to_path_buf();
    peer_hook(home.path(), "corr-nogate", &write_payload(&wa.join("src/a.rs")));
    commit_file(&wa, "src/a.rs", "a\n");
    let mut peer_a =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-nogate"));
    let refused = peer_a.tools_call(json!(1), "handoff", json!({"summary": "trust me"}));
    let message = error_message(&refused);
    assert!(
        message.contains("gate precondition failed") && message.contains("./scripts/check.sh"),
        "gate-missing error names the gate precondition and the required command: {message}"
    );
    let status = lead.tools_call(json!(2), "assignment_status", json!({"assignment": id_a}));
    assert_eq!(status["result"]["state"], "Dispatched");

    // (b) The Gate ran, then the peer wrote again: the evidence is stale.
    let (_id_b, worktree_b) = dispatch_worktree_peer(&mut lead, &head, &["docs/**"], "corr-stale");
    let wb = Path::new(&worktree_b).to_path_buf();
    peer_hook(home.path(), "corr-stale", &write_payload(&wb.join("docs/b.md")));
    commit_file(&wb, "docs/b.md", "b\n");
    peer_hook(home.path(), "corr-stale", &bash_payload("./scripts/check.sh"));
    // ... and one more write after the gate run.
    let late = peer_hook(home.path(), "corr-stale", &write_payload(&wb.join("docs/b_late.md")));
    assert_eq!(late["hookSpecificOutput"]["permissionDecision"], "allow");
    commit_file(&wb, "docs/b_late.md", "late\n");

    let mut peer_b =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-stale"));
    let refused = peer_b.tools_call(
        json!(3),
        "handoff",
        json!({"summary": "gate was green earlier"}),
    );
    let message = error_message(&refused);
    assert!(
        message.contains("gate precondition failed"),
        "stale-gate error names the gate precondition: {message}"
    );
    assert!(
        message.contains("after"),
        "stale-gate error names the ordering requirement: {message}"
    );
}

// ---------------------------------------------------------------------------
// Slice 3: Write Scope flagging & read-only handoffs
// ---------------------------------------------------------------------------

#[test]
fn handoff_flags_changed_paths_outside_the_declared_write_scope() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());
    let (id, worktree) = dispatch_worktree_peer(&mut lead, &head, &["src/**"], "corr-scope");
    let worktree_path = Path::new(&worktree).to_path_buf();

    // The peer commits one in-scope and one out-of-scope path, then runs the Gate.
    let write = peer_hook(
        home.path(),
        "corr-scope",
        &write_payload(&worktree_path.join("src/engine.rs")),
    );
    assert_eq!(write["hookSpecificOutput"]["permissionDecision"], "allow");
    commit_file(&worktree_path, "src/engine.rs", "in scope\n");
    let candidate = commit_file(&worktree_path, "outside_scope.txt", "out of scope\n");
    let gate = peer_hook(home.path(), "corr-scope", &bash_payload("./scripts/check.sh"));
    assert_eq!(gate["hookSpecificOutput"]["permissionDecision"], "allow");

    let mut peer =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-scope"));
    let handed = peer.tools_call(
        json!(1),
        "handoff",
        json!({"summary": "shipped, but I also touched a root file"}),
    );
    assert!(
        handed.get("error").is_none(),
        "out-of-scope paths are flagged, not refused: {handed}"
    );
    assert_eq!(handed["result"]["state"], "HandedBack");
    assert_eq!(handed["result"]["candidate"], json!(candidate));
    assert_eq!(handed["result"]["scope"]["exceeded"], json!(true));
    assert_eq!(
        handed["result"]["scope"]["paths"],
        json!(["outside_scope.txt"]),
        "the offending path is listed"
    );

    // The Lead-side ledger record carries the violation for the review flow.
    let status = lead.tools_call(json!(2), "assignment_status", json!({"assignment": id}));
    assert_eq!(status["result"]["state"], "HandedBack");
    let events = ledger_events_of_type(home.path(), "assignment_handed_back");
    assert_eq!(events.last().unwrap()["scopeViolations"], json!(["outside_scope.txt"]));
}

#[test]
fn read_only_handoff_requires_a_clean_tree_and_claims_no_candidate_or_lease() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());

    // A Reviewer with no Write Scope is read-only and acquires no Writer Lease.
    let mut reviewer = valid_create(&head);
    reviewer["disposition"] = json!("Reviewer");
    reviewer["writeScope"] = json!([]);
    reviewer["serialOnly"] = json!([]);
    let id = create_assignment(&mut lead, reviewer);
    let dispatched = lead.tools_call(
        json!(2),
        "assignment_dispatch",
        json!({"assignment": id, "correlation": "corr-reader"}),
    );
    assert!(dispatched.get("error").is_none(), "{dispatched}");
    assert!(dispatched["result"].get("lease").is_none());

    // The checkout tree is clean, so the read-only handback is accepted.
    let mut peer =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-reader"));
    let handed = peer.tools_call(
        json!(1),
        "handoff",
        json!({"summary": "review complete: candidate verified read-only"}),
    );
    assert!(
        handed.get("error").is_none(),
        "clean-tree read-only handoff must succeed: {handed}"
    );
    assert_eq!(handed["result"]["state"], "HandedBack");
    assert_eq!(handed["result"]["candidate"], Value::Null);
    assert_eq!(handed["result"]["readOnly"], json!(true));

    // No candidate or lease was ever claimed for the read-only assignment.
    let events = ledger_events_of_type(home.path(), "assignment_handed_back");
    assert!(events.last().unwrap().get("candidate").is_none());
    assert!(
        ledger_events_of_type(home.path(), "writer_lease_acquired")
            .iter()
            .all(|e| e["assignment"] != json!(id)),
        "a read-only handoff never acquires a Writer Lease"
    );

    // A dirty checkout refuses the read-only handoff with the unclean state.
    std::fs::write(repo.path().join("untracked-review-note.txt"), "dirty\n").unwrap();
    let scout_id = create_assignment(&mut lead, {
        let mut scout = valid_create(&head);
        scout["disposition"] = json!("Scout");
        scout["writeScope"] = json!([]);
        scout["serialOnly"] = json!([]);
        scout
    });
    let dispatched = lead.tools_call(
        json!(3),
        "assignment_dispatch",
        json!({"assignment": scout_id, "correlation": "corr-scout"}),
    );
    assert!(dispatched.get("error").is_none(), "{dispatched}");
    let mut scout =
        McpProcess::spawn_correlated("peer", repo.path(), home.path(), Some("corr-scout"));
    let refused = scout.tools_call(json!(4), "handoff", json!({"summary": "scout report"}));
    let message = error_message(&refused);
    assert!(
        message.contains("not clean") && message.contains("untracked-review-note.txt"),
        "read-only handoff still demands a clean tree: {message}"
    );
}
