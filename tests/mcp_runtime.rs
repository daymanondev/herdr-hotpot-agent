//! Seam tests for the Runtime Coordinator Layer (`hotpot mcp`): the role tool
//! surface, `Assignment` lifecycle state machine, verified `baseCommit`, and the
//! append-only ledger guarantees. The shared stdio harness lives in `tests/common`.

mod common;

use common::{
    McpProcess, TempDir, init_git_repo, ledger_line_count, ledger_lines, ledger_path,
    seed_ledger_event, valid_create,
};
use serde_json::{Value, json};

const SUPERVISOR_TOOLS: &[&str] = &[
    "room_status",
    "runtime_findings",
    "message_lead",
    "attention_feedback",
    "lead_replace_prepare",
    "lead_replace_confirm",
    "lead_replace_cancel",
];

const LEAD_TOOLS: &[&str] = &[
    "assignment_create",
    "assignment_dispatch",
    "assignment_status",
    "assignment_answer",
    "assignment_accept",
    "assignment_close",
    "gate_run",
    "workspace_close",
    "lease_reclaim",
];

const PEER_TOOLS: &[&str] = &["ask", "handoff"];

#[test]
fn tools_list_returns_exactly_the_spec_tool_set_per_role() {
    for (role, expected) in [
        ("supervisor", SUPERVISOR_TOOLS),
        ("lead", LEAD_TOOLS),
        ("peer", PEER_TOOLS),
    ] {
        let home = TempDir::new("home");
        let (repo, _head) = init_git_repo();
        let mut mcp = McpProcess::spawn(role, repo.path(), home.path());

        let init = mcp.request(json!("init-1"), "initialize", json!({}));
        assert_eq!(init["jsonrpc"], "2.0", "{role} initialize echo");
        assert_eq!(init["id"], "init-1", "{role} initialize id echo");
        assert!(
            init["result"]["serverInfo"].is_object(),
            "{role} serverInfo"
        );

        let list = mcp.request(json!(1), "tools/list", json!({}));
        assert_eq!(list["jsonrpc"], "2.0");
        assert_eq!(list["id"], 1);
        let names: Vec<String> = list["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("{role} tools/list returns an array"))
            .iter()
            .map(|t| {
                t["name"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{role} tool has a name"))
                    .to_string()
            })
            .collect();
        assert_eq!(
            names, expected,
            "{role} tools/list must be exactly the spec's tool set"
        );
    }
}

#[test]
fn refused_thinking_efforts_error_without_ledger_append() {
    for refused in ["ultra", "ultracode"] {
        let home = TempDir::new("home");
        let (repo, head) = init_git_repo();
        let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

        let mut args = valid_create(&head);
        args["thinking"] = json!(refused);
        let lines_before = ledger_line_count(home.path());

        let resp = mcp.tools_call(json!(1), "assignment_create", args);
        let error = resp["error"]
            .as_object()
            .unwrap_or_else(|| panic!("`{refused}` must be refused: {resp}"));
        assert_eq!(resp["id"], 1, "id is echoed on refusal");
        let message = error["message"].as_str().unwrap();
        assert!(
            message.contains(refused),
            "error names the refused effort `{refused}`: {message}"
        );
        assert!(
            message.contains("refused"),
            "error names the refusal: {message}"
        );
        assert_eq!(
            ledger_line_count(home.path()),
            lines_before,
            "a refused create appends nothing to the ledger"
        );
    }
}

#[test]
fn base_commit_is_verified_against_the_project_git_repo() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    let bogus = "deadbeef".repeat(5); // 40 hex chars, not a repo object
    let mut args = valid_create(&bogus);
    let resp = mcp.tools_call(json!(1), "assignment_create", args);
    let message = resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("bogus baseCommit must error: {resp}"));
    assert!(
        message.contains("baseCommit") && message.contains(&bogus),
        "error names the rejected baseCommit: {message}"
    );

    args = valid_create(&head); // the repo's real `git rev-parse HEAD`
    let resp = mcp.tools_call(json!(2), "assignment_create", args);
    assert!(
        resp.get("error").is_none(),
        "real HEAD sha must be accepted: {resp}"
    );
    assert_eq!(resp["result"]["state"], "Draft");
    assert_eq!(resp["result"]["baseCommit"], head);
}

#[test]
fn invalid_disposition_is_refused() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    let mut args = valid_create(&head);
    args["disposition"] = json!("Wizard");
    let resp = mcp.tools_call(json!(1), "assignment_create", args);
    let message = resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("Disposition `Wizard` must be refused: {resp}"));
    assert!(
        message.contains("Wizard") && message.contains("Disposition"),
        "error names the invalid Disposition: {message}"
    );
    assert_eq!(ledger_line_count(home.path()), 0);
}

#[test]
fn dispatch_moves_draft_to_dispatched_and_rejects_non_draft() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    let created = mcp.tools_call(json!(1), "assignment_create", valid_create(&head));
    let id = created["result"]["id"].as_str().unwrap().to_string();

    let dispatched = mcp.tools_call(
        json!(2),
        "assignment_dispatch",
        json!({
            "assignment": id,
            "correlation": "corr-seed-1",
        }),
    );
    assert!(
        dispatched.get("error").is_none(),
        "dispatch of a Draft assignment: {dispatched}"
    );
    assert_eq!(dispatched["result"]["state"], "Dispatched");
    assert_eq!(dispatched["result"]["correlation"], "corr-seed-1");
    assert!(
        dispatched["result"]["workspace"]
            .as_str()
            .is_some_and(|w| !w.is_empty()),
        "dispatch registers a workspace binding"
    );

    let status = mcp.tools_call(json!(3), "assignment_status", json!({"assignment": id}));
    assert_eq!(status["result"]["state"], "Dispatched");

    let again = mcp.tools_call(json!(4), "assignment_dispatch", json!({"assignment": id}));
    let message = again["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("dispatching a Dispatched assignment must error: {again}"));
    assert!(
        message.contains("Draft"),
        "error explains dispatch requires Draft: {message}"
    );
}

#[test]
fn answering_an_asking_assignment_returns_it_to_dispatched() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    let created = mcp.tools_call(json!(1), "assignment_create", valid_create(&head));
    let id = created["result"]["id"].as_str().unwrap().to_string();
    mcp.tools_call(json!(2), "assignment_dispatch", json!({"assignment": id}));

    // Answering before any ask is an illegal transition.
    let early = mcp.tools_call(
        json!(3),
        "assignment_answer",
        json!({
            "assignment": id, "answer": "premature"
        }),
    );
    assert!(
        early.get("error").is_some(),
        "answering a Dispatched (not Asking) assignment must error: {early}"
    );

    // Simulate the peer `ask` path (ticket 09) by appending its ledger event.
    seed_ledger_event(
        home.path(),
        json!({
            "type": "assignment_asked",
            "assignment": id,
            "question": "Which ledger file should the runtime watch?"
        }),
    );
    let asking = mcp.tools_call(json!(4), "assignment_status", json!({"assignment": id}));
    assert_eq!(asking["result"]["state"], "Asking");

    let answered = mcp.tools_call(
        json!(5),
        "assignment_answer",
        json!({
            "assignment": id,
            "answer": "Replay <HOTPOT_HOME>/runtime/v1/<project-slug>/ledger.jsonl"
        }),
    );
    assert!(
        answered.get("error").is_none(),
        "answering an Asking assignment: {answered}"
    );
    assert_eq!(answered["result"]["state"], "Dispatched");
}

#[test]
fn gate_accept_and_close_follow_the_terminal_state_machine() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    let created = mcp.tools_call(json!(1), "assignment_create", valid_create(&head));
    let id = created["result"]["id"].as_str().unwrap().to_string();
    mcp.tools_call(json!(2), "assignment_dispatch", json!({"assignment": id}));

    // Accepting before any handback is an illegal transition.
    let early = mcp.tools_call(json!(3), "assignment_accept", json!({"assignment": id}));
    assert!(
        early.get("error").is_some(),
        "accepting a Dispatched assignment must error: {early}"
    );

    // Simulate the peer `handoff` path (ticket 09).
    seed_ledger_event(
        home.path(),
        json!({
            "type": "assignment_handed_back",
            "assignment": id,
            "summary": "peer handback with green gate evidence"
        }),
    );
    let handed = mcp.tools_call(json!(4), "assignment_status", json!({"assignment": id}));
    assert_eq!(handed["result"]["state"], "HandedBack");

    let gate = mcp.tools_call(json!(5), "gate_run", json!({"assignment": id}));
    assert!(
        gate.get("error").is_none(),
        "gate_run starts from HandedBack: {gate}"
    );
    assert_eq!(gate["result"]["state"], "GateRunning");

    let accepted = mcp.tools_call(
        json!(6),
        "assignment_accept",
        json!({
            "assignment": id, "note": "candidate integrated manually"
        }),
    );
    assert!(
        accepted.get("error").is_none(),
        "accept from GateRunning: {accepted}"
    );
    assert_eq!(accepted["result"]["state"], "Accepted");

    let closed = mcp.tools_call(json!(7), "assignment_close", json!({"assignment": id}));
    assert!(
        closed.get("error").is_none(),
        "close from terminal Accepted: {closed}"
    );
    assert_eq!(closed["result"]["state"], "Closed");

    // Closed is final: every lifecycle tool must refuse.
    let redispatch = mcp.tools_call(json!(8), "assignment_dispatch", json!({"assignment": id}));
    assert!(redispatch.get("error").is_some());
}

#[test]
fn closing_in_flight_work_stops_it_and_double_close_errors() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    let created = mcp.tools_call(json!(1), "assignment_create", valid_create(&head));
    let id = created["result"]["id"].as_str().unwrap().to_string();
    mcp.tools_call(json!(2), "assignment_dispatch", json!({"assignment": id}));

    let stopped = mcp.tools_call(json!(3), "assignment_close", json!({"assignment": id}));
    assert!(
        stopped.get("error").is_none(),
        "closing dispatched in-flight work: {stopped}"
    );
    assert_eq!(stopped["result"]["state"], "Stopped");

    let again = mcp.tools_call(json!(4), "assignment_close", json!({"assignment": id}));
    assert!(
        again.get("error").is_some(),
        "closing an already-Stopped assignment must error: {again}"
    );
}

#[test]
fn ledger_is_append_only_jsonl_with_strictly_increasing_seq() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();

    let first_id;
    {
        let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());
        let created = mcp.tools_call(json!(1), "assignment_create", valid_create(&head));
        first_id = created["result"]["id"].as_str().unwrap().to_string();
        mcp.tools_call(
            json!(2),
            "assignment_dispatch",
            json!({"assignment": first_id}),
        );
    }

    let ledger = ledger_path(home.path());
    let repo_base = repo
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let slug = ledger
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert!(
        slug.starts_with(&format!("{repo_base}-")),
        "project slug embeds the repo basename: {slug}"
    );
    assert_eq!(
        ledger.file_name().unwrap(),
        "ledger.jsonl",
        "ledger file name"
    );

    let before = std::fs::read(&ledger).unwrap();
    let seqs: Vec<u64> = ledger_lines(home.path())
        .iter()
        .map(|e| e["seq"].as_u64().expect("event carries a seq"))
        .collect();
    assert_eq!(
        seqs,
        (1..=seqs.len() as u64).collect::<Vec<_>>(),
        "sequence numbers strictly increase from 1"
    );

    // A fresh process appending more events must only ever grow the file.
    {
        let mut fresh = McpProcess::spawn("lead", repo.path(), home.path());
        let second = fresh.tools_call(json!(3), "assignment_create", valid_create(&head));
        assert!(second.get("error").is_none(), "{second}");
        fresh.tools_call(
            json!(4),
            "assignment_dispatch",
            json!({
                "assignment": second["result"]["id"].as_str().unwrap()
            }),
        );
    }

    let after = std::fs::read(&ledger).unwrap();
    assert!(
        after.starts_with(&before),
        "the ledger is append-only: earlier bytes are a stable prefix"
    );
    let seqs: Vec<u64> = ledger_lines(home.path())
        .iter()
        .map(|e| e["seq"].as_u64().unwrap())
        .collect();
    assert_eq!(
        seqs,
        (1..=4).collect::<Vec<_>>(),
        "seq continues across processes"
    );
}

#[test]
fn jsonrpc_errors_cover_unknown_methods_tools_and_cross_role_calls() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();

    let mut lead = McpProcess::spawn("lead", repo.path(), home.path());

    let unknown_method = lead.request(json!(41), "water/find", json!({}));
    assert_eq!(unknown_method["id"], 41, "id echo on method errors");
    assert_eq!(unknown_method["error"]["code"], -32601);
    assert!(
        unknown_method["error"]["message"]
            .as_str()
            .unwrap()
            .contains("water/find"),
        "error names the unknown method"
    );

    // The old SLP tool names must be gone: they are unknown tools now.
    let old_name = lead.tools_call(json!("42"), "lead_set_contract", json!({}));
    assert_eq!(old_name["error"]["code"], -32602);
    assert!(
        old_name["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool"),
        "old SLP tool name is refused: {old_name}"
    );
    assert_eq!(old_name["id"], "42", "string id echo");

    // Malformed request lines still get exactly one JSON-RPC error response.
    let parse_error = lead.raw_roundtrip("{ not json");
    assert_eq!(parse_error["error"]["code"], -32700);
    assert_eq!(parse_error["id"], Value::Null);

    // The role fence: a peer never sees Lead tools, even by calling them directly.
    let mut peer = McpProcess::spawn("peer", repo.path(), home.path());
    let fenced = peer.tools_call(json!(1), "assignment_create", valid_create(&head));
    assert_eq!(
        fenced["error"]["code"], -32602,
        "peer calling a lead tool is an unknown-tool error: {fenced}"
    );
    assert!(
        fenced["error"]["message"]
            .as_str()
            .unwrap()
            .contains("peer"),
        "fence error names the caller's role"
    );

    // Peer `ask`/`handoff` are listed but their semantics land with ticket 09.
    let ask = peer.tools_call(json!(2), "ask", json!({"question": "scope check?"}));
    assert!(
        ask.get("error").is_some(),
        "peer ask is an explicit not-yet error: {ask}"
    );
}

/// Asserts every story-19 field recorded on the assignment projection.
fn assert_recorded_fields(assignment: &Value, head: &str) {
    assert_eq!(assignment["state"], "Draft");
    assert_eq!(assignment["outcome"], "Ship the append-only runtime ledger");
    assert_eq!(assignment["disposition"], "Engineer");
    assert_eq!(assignment["baseCommit"], head);
    assert_eq!(assignment["writeScope"], json!(["src/**"]));
    assert_eq!(assignment["serialOnly"], json!(["Cargo.lock"]));
    assert_eq!(assignment["gate"], "./scripts/check.sh");
    assert_eq!(assignment["thinking"], "medium");
    assert_eq!(
        assignment["thinkingReason"],
        "bounded mechanical change to the dispatcher"
    );
}

#[test]
fn assignment_create_persists_and_replays_across_processes() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();

    let created_id;
    {
        let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());
        let resp = mcp.tools_call(json!(1), "assignment_create", valid_create(&head));
        assert!(
            resp.get("error").is_none(),
            "assignment_create must succeed: {resp}"
        );
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        let result = &resp["result"];
        let id = result["id"].as_str().unwrap().to_string();
        assert!(
            id.starts_with("asg_"),
            "assignment id looks like asg_*: {id}"
        );
        assert_recorded_fields(result, &head);
        created_id = id;
    }

    // A fresh `hotpot mcp` process over the same state dir must rebuild the Draft
    // assignment (with all recorded fields) purely from the ledger.
    let mut fresh = McpProcess::spawn("lead", repo.path(), home.path());
    let resp = fresh.tools_call(
        json!("s1"),
        "assignment_status",
        json!({
            "assignment": created_id
        }),
    );
    assert!(
        resp.get("error").is_none(),
        "assignment_status after replay: {resp}"
    );
    assert_eq!(resp["id"], "s1");
    assert_eq!(resp["result"]["id"], created_id.as_str());
    assert_recorded_fields(&resp["result"], &head);
}
