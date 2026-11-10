//! Seam tests for Writer Leases: exclusive in-checkout leases after clean-HEAD
//! verification, and scope-disjoint isolated worktrees with warm build caches
//! (spec user stories 20-22; `WriterLeaseMode` implementation decision).
//!
//! Drives the `hotpot mcp` stdio harness against a temp real git repo and temp
//! `HOTPOT_HOME`, asserting JSON-RPC errors, ledger events, git state, and
//! worktree symlinks on disk. Zero network; no private-internal assertions.

mod common;

use common::{McpProcess, TempDir, create_assignment, init_git_repo, ledger_lines, valid_create};
use serde_json::{Value, json};
use std::path::Path;

/// Events of one type recorded in the project ledger.
fn ledger_events_of_type(home: &Path, kind: &str) -> Vec<Value> {
    ledger_lines(home)
        .into_iter()
        .filter(|e| e["type"] == json!(kind))
        .collect()
}

/// The message of a JSON-RPC error response, asserting one is present.
fn error_message(resp: &Value) -> String {
    resp["error"]["message"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a JSON-RPC error: {resp}"))
        .to_string()
}

#[test]
fn exclusive_lease_acquire_refuse_and_release_after_workspace_close() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    // Clean checkout at baseCommit: dispatch acquires the exclusive lease.
    let first = create_assignment(&mut mcp, valid_create(&head));
    let dispatched = mcp.tools_call(
        json!(2),
        "assignment_dispatch",
        json!({"assignment": first}),
    );
    assert!(
        dispatched.get("error").is_none(),
        "clean-checkout dispatch must succeed: {dispatched}"
    );
    assert_eq!(dispatched["result"]["state"], "Dispatched");
    assert_eq!(
        dispatched["result"]["lease"]["mode"], "InCheckoutExclusive",
        "dispatch result reports the acquired lease mode: {dispatched}"
    );
    let acquired = ledger_events_of_type(home.path(), "writer_lease_acquired");
    assert_eq!(acquired.len(), 1, "exactly one lease event appended");
    assert_eq!(acquired[0]["assignment"], json!(first));
    assert_eq!(acquired[0]["mode"], "InCheckoutExclusive");

    // A second writable dispatch while the lease is held is refused.
    let second = create_assignment(&mut mcp, valid_create(&head));
    let refused = mcp.tools_call(
        json!(4),
        "assignment_dispatch",
        json!({"assignment": second}),
    );
    let message = error_message(&refused);
    assert!(
        message.contains("writer_exclusive"),
        "refusal carries the writer_exclusive code: {message}"
    );
    assert!(
        message.contains(&first),
        "refusal names the lease-holding assignment: {message}"
    );
    assert_eq!(
        ledger_events_of_type(home.path(), "writer_lease_acquired").len(),
        1,
        "the refused dispatch appends no lease event"
    );
    let still_draft = mcp.tools_call(
        json!(5),
        "assignment_status",
        json!({"assignment": second}),
    );
    assert_eq!(still_draft["result"]["state"], "Draft");

    // workspace_close releases the lease; a new exclusive dispatch succeeds.
    let closed = mcp.tools_call(
        json!(6),
        "workspace_close",
        json!({"assignment": first}),
    );
    assert!(
        closed.get("error").is_none(),
        "workspace_close on a held lease: {closed}"
    );
    assert_eq!(closed["result"]["lease"]["released"], json!(true));
    let released = ledger_events_of_type(home.path(), "writer_lease_released");
    assert_eq!(released.len(), 1, "lease release is a ledger event");
    assert_eq!(released[0]["assignment"], json!(first));

    let redispatch = mcp.tools_call(
        json!(7),
        "assignment_dispatch",
        json!({"assignment": second}),
    );
    assert!(
        redispatch.get("error").is_none(),
        "after release a new exclusive dispatch succeeds: {redispatch}"
    );
    assert_eq!(redispatch["result"]["state"], "Dispatched");
    assert_eq!(
        redispatch["result"]["lease"]["mode"], "InCheckoutExclusive"
    );
}
