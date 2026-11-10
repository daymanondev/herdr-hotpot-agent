//! Seam tests for Writer Leases: exclusive in-checkout leases after clean-HEAD
//! verification, and scope-disjoint isolated worktrees with warm build caches
//! (spec user stories 20-22; `WriterLeaseMode` implementation decision).
//!
//! Drives the `hotpot mcp` stdio harness against a temp real git repo and temp
//! `HOTPOT_HOME`, asserting JSON-RPC errors, ledger events, git state, and
//! worktree symlinks on disk. Zero network; no private-internal assertions.

mod common;

use common::{McpProcess, TempDir, create_assignment, git, init_git_repo, ledger_lines, valid_create};
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

#[test]
fn dirty_checkout_refuses_dispatch_and_appends_no_lease_event() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    // An untracked file dirties the tree per `--untracked-files=all`.
    std::fs::write(repo.path().join("scratch-notes.txt"), "dirty\n").unwrap();
    let id = create_assignment(&mut mcp, valid_create(&head));

    let refused = mcp.tools_call(
        json!(2),
        "assignment_dispatch",
        json!({"assignment": id}),
    );
    let message = error_message(&refused);
    assert!(
        message.contains("git precondition failed")
            && message.contains("not clean")
            && message.contains("scratch-notes.txt"),
        "error names the failed git precondition and the dirty entry: {message}"
    );
    assert!(
        ledger_events_of_type(home.path(), "writer_lease_acquired").is_empty(),
        "no lease event is appended on a refused dispatch"
    );
    assert!(
        ledger_events_of_type(home.path(), "assignment_dispatched").is_empty(),
        "the refused dispatch never reaches the ledger"
    );
    let status = mcp.tools_call(json!(3), "assignment_status", json!({"assignment": id}));
    assert_eq!(status["result"]["state"], "Draft");
}

#[test]
fn head_moved_past_base_commit_refuses_dispatch() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());
    let id = create_assignment(&mut mcp, valid_create(&head));

    // A second commit moves HEAD past the assignment's verified baseCommit while
    // the tree itself stays clean.
    std::fs::write(repo.path().join("README.md"), "# fixture project\n\nmoved on\n").unwrap();
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-qm", "move HEAD past baseCommit"]);
    let moved = git(repo.path(), &["rev-parse", "HEAD"]);
    assert_ne!(moved, head);

    let refused = mcp.tools_call(
        json!(2),
        "assignment_dispatch",
        json!({"assignment": id}),
    );
    let message = error_message(&refused);
    assert!(
        message.contains("git precondition failed")
            && message.contains("HEAD")
            && message.contains("baseCommit")
            && message.contains(&moved),
        "error names the failed HEAD/baseCommit precondition and the moved HEAD: {message}"
    );
    assert!(
        ledger_events_of_type(home.path(), "writer_lease_acquired").is_empty(),
        "no lease event is appended on a refused dispatch"
    );
}

#[test]
fn read_only_assignment_acquires_no_lease() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    // A Reviewer with no Write Scope is read-only: it acquires no Writer Lease,
    // so its dispatch is valid even while the tree is dirty.
    let mut reviewer = valid_create(&head);
    reviewer["disposition"] = json!("Reviewer");
    reviewer["writeScope"] = json!([]);
    reviewer["serialOnly"] = json!([]);
    std::fs::write(repo.path().join("untracked.txt"), "still fine for readers\n").unwrap();
    let id = create_assignment(&mut mcp, reviewer);

    let dispatched = mcp.tools_call(
        json!(2),
        "assignment_dispatch",
        json!({"assignment": id}),
    );
    assert!(
        dispatched.get("error").is_none(),
        "read-only dispatch ignores checkout cleanliness: {dispatched}"
    );
    assert_eq!(dispatched["result"]["state"], "Dispatched");
    assert!(
        dispatched["result"].get("lease").is_none(),
        "no lease is reported for a read-only assignment: {dispatched}"
    );
    assert!(
        ledger_events_of_type(home.path(), "writer_lease_acquired").is_empty(),
        "no lease event exists for a read-only dispatch"
    );

    // And it does not block the next writable exclusive dispatch.
    let writable = create_assignment(&mut mcp, valid_create(&head));
    git(repo.path(), &["clean", "-fdq"]);
    let exclusive = mcp.tools_call(
        json!(4),
        "assignment_dispatch",
        json!({"assignment": writable}),
    );
    assert!(
        exclusive.get("error").is_none(),
        "a read-only lease never blocks a writable dispatch: {exclusive}"
    );
    assert_eq!(exclusive["result"]["lease"]["mode"], "InCheckoutExclusive");
}

/// A writable worktree-mode `assignment_create` payload with the given scope.
fn worktree_create(head: &str, write_scope: &[&str], serial_only: &[&str]) -> Value {
    json!({
        "outcome": "Ship a scope-disjoint vertical slice",
        "disposition": "Engineer",
        "baseCommit": head,
        "writeScope": write_scope,
        "serialOnly": serial_only,
        "gate": "./scripts/check.sh",
    })
}

/// Dispatches `assignment` in worktree isolation mode.
fn dispatch_worktree(mcp: &mut McpProcess, id: &str) -> Value {
    mcp.tools_call(
        json!("wt-dispatch"),
        "assignment_dispatch",
        json!({"assignment": id, "isolation": "worktree"}),
    )
}

#[test]
fn three_disjoint_worktree_writers_get_real_worktrees_and_a_fourth_hits_the_cap() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    let scopes: [(&[&str], &[&str]); 3] = [
        (&["src/engine/**"], &["Cargo.lock"]),
        (&["src/ui/**"], &[]),
        (&["docs/**"], &[]),
    ];
    let mut worktrees = Vec::new();
    for (scope, serial) in scopes {
        let id = create_assignment(&mut mcp, worktree_create(&head, scope, serial));
        let dispatched = dispatch_worktree(&mut mcp, &id);
        assert!(
            dispatched.get("error").is_none(),
            "disjoint worktree dispatch must succeed: {dispatched}"
        );
        assert_eq!(dispatched["result"]["state"], "Dispatched");
        assert_eq!(
            dispatched["result"]["lease"]["mode"], "IsolatedWorktree",
            "dispatch result reports the worktree lease mode: {dispatched}"
        );
        let worktree = dispatched["result"]["lease"]["worktree"]
            .as_str()
            .unwrap_or_else(|| panic!("worktree lease reports its worktree path: {dispatched}"))
            .to_string();
        // The worktree lives under <HOTPOT_HOME>/runtime/v1/<project-slug>/worktrees/<asg_id>/.
        let slug_dir = project_runtime_dir(home.path());
        let expected_root = slug_dir.join("worktrees").join(&id);
        assert_eq!(
            std::path::Path::new(&worktree),
            expected_root,
            "worktree path follows the spec layout"
        );
        assert!(
            std::path::Path::new(&worktree).is_dir(),
            "the worktree is materialized on disk: {worktree}"
        );
        assert_eq!(
            git(std::path::Path::new(&worktree), &["rev-parse", "HEAD"]),
            head,
            "worktree HEAD equals the assignment baseCommit"
        );
        worktrees.push(worktree);
    }
    assert_eq!(
        ledger_events_of_type(home.path(), "writer_lease_acquired").len(),
        3
    );

    // A fourth worktree writer is refused at the lease cap.
    let fourth = create_assignment(&mut mcp, worktree_create(&head, &["tools/**"], &[]));
    let refused = dispatch_worktree(&mut mcp, &fourth);
    let message = error_message(&refused);
    assert!(
        message.contains("lease_cap"),
        "fourth concurrent worktree writer hits the cap: {message}"
    );
    let slug_dir = project_runtime_dir(home.path());
    assert!(
        !slug_dir.join("worktrees").join(&fourth).exists(),
        "the refused dispatch provisions no worktree"
    );
}

/// The per-project runtime dir holding the ledger (`<home>/runtime/v1/<slug>`).
fn project_runtime_dir(home: &Path) -> std::path::PathBuf {
    common::ledger_path(home)
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn worktree_scope_overlap_and_serial_path_are_refused() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    // Active lease A writes under src/engine/**.
    let engine = create_assignment(&mut mcp, worktree_create(&head, &["src/engine/**"], &[]));
    let dispatched = dispatch_worktree(&mut mcp, &engine);
    assert!(dispatched.get("error").is_none(), "{dispatched}");

    // A Write Scope overlapping lease A's Write Scope is refused with scope_overlap.
    let overlapping = create_assignment(&mut mcp, worktree_create(&head, &["src/**"], &[]));
    let refused = dispatch_worktree(&mut mcp, &overlapping);
    let message = error_message(&refused);
    assert!(
        message.contains("scope_overlap") && message.contains("src/**"),
        "overlapping scope refusal names the colliding path: {message}"
    );

    // Active lease B serializes Cargo.lock (write scope pkg-a/**, serialOnly Cargo.lock).
    let serializer = create_assignment(
        &mut mcp,
        worktree_create(&head, &["pkg-a/**"], &["Cargo.lock"]),
    );
    let dispatched = dispatch_worktree(&mut mcp, &serializer);
    assert!(dispatched.get("error").is_none(), "{dispatched}");

    // A peer whose declared path is inside lease B's serialOnly is refused
    // with serial_path.
    let into_serial = create_assignment(&mut mcp, worktree_create(&head, &["Cargo.lock"], &[]));
    let refused = dispatch_worktree(&mut mcp, &into_serial);
    let message = error_message(&refused);
    assert!(
        message.contains("serial_path") && message.contains("Cargo.lock"),
        "serial-only path refusal names the path: {message}"
    );

    // A genuinely disjoint peer is still accepted while both leases are held.
    let disjoint = create_assignment(&mut mcp, worktree_create(&head, &["pkg-b/**"], &[]));
    let ok = dispatch_worktree(&mut mcp, &disjoint);
    assert!(ok.get("error").is_none(), "disjoint peer dispatches: {ok}");

    assert_eq!(
        ledger_events_of_type(home.path(), "writer_lease_acquired").len(),
        3,
        "only the three accepted worktree leases exist"
    );
    for refused_id in [&overlapping, &into_serial] {
        assert!(
            !project_runtime_dir(home.path())
                .join("worktrees")
                .join(refused_id)
                .exists(),
            "refused dispatch `{refused_id}` provisions no worktree"
        );
    }
}

#[test]
fn worktree_peers_share_cargo_target_dir_and_worktreeinclude_symlinks() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();

    // Fixture: `.worktreeinclude` names `node_modules` (gitignored, present in
    // the main checkout) so worktrees get a warm symlink instead of a cold dir.
    std::fs::write(repo.path().join(".gitignore"), "node_modules\n").unwrap();
    std::fs::write(repo.path().join(".worktreeinclude"), "node_modules\n").unwrap();
    std::fs::create_dir_all(repo.path().join("node_modules").join("left-pad")).unwrap();
    std::fs::write(
        repo.path().join("node_modules").join("left-pad").join("package.json"),
        "{\"name\":\"left-pad\"}\n",
    )
    .unwrap();
    git(repo.path(), &["add", "-A"]);
    git(repo.path(), &["commit", "-qm", "fixture warm-cache entries"]);

    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());
    let expected_target = std::fs::canonicalize(repo.path().join("node_modules")).unwrap();

    let first = create_assignment(&mut mcp, worktree_create(&head, &["src/engine/**"], &[]));
    let dispatched = dispatch_worktree(&mut mcp, &first);
    assert!(dispatched.get("error").is_none(), "{dispatched}");

    // The shared CARGO_TARGET_DIR is recorded in the dispatch result and ledger.
    let cargo_dir = dispatched["result"]["lease"]["cargoTargetDir"]
        .as_str()
        .unwrap_or_else(|| panic!("worktree lease records cargoTargetDir: {dispatched}"))
        .to_string();
    assert_eq!(
        std::path::PathBuf::from(&cargo_dir),
        project_runtime_dir(home.path()).join("cache").join("cargo-target"),
        "cargoTargetDir is the shared warm-build cache under Hotpot Home"
    );
    assert!(std::path::Path::new(&cargo_dir).is_dir(), "cache dir exists");
    let acquired = ledger_events_of_type(home.path(), "writer_lease_acquired");
    assert_eq!(acquired[0]["cargoTargetDir"], json!(cargo_dir));
    assert_eq!(acquired[0]["worktreeInclude"], json!(["node_modules"]));

    // The worktree's `node_modules` is a symlink into the main checkout.
    let worktree = dispatched["result"]["lease"]["worktree"].as_str().unwrap();
    let link = std::path::Path::new(worktree).join("node_modules");
    let target = std::fs::read_link(&link)
        .unwrap_or_else(|e| panic!("node_modules must be a symlink in the worktree: {e}"));
    assert_eq!(
        std::fs::canonicalize(&target).unwrap(),
        expected_target,
        "symlink points at the main checkout's node_modules"
    );
    assert!(
        link.join("left-pad").join("package.json").is_file(),
        "the symlinked node_modules is usable from inside the worktree"
    );

    // A second worktree peer shares the same warm cache path.
    let second = create_assignment(&mut mcp, worktree_create(&head, &["src/ui/**"], &[]));
    let dispatched = dispatch_worktree(&mut mcp, &second);
    assert!(dispatched.get("error").is_none(), "{dispatched}");
    assert_eq!(
        dispatched["result"]["lease"]["cargoTargetDir"], json!(cargo_dir),
        "worktree peers share one CARGO_TARGET_DIR"
    );
}

#[test]
fn workspace_close_cleans_in_flight_worktrees_and_retains_handed_back_candidates() {
    let home = TempDir::new("home");
    let (repo, head) = init_git_repo();
    let mut mcp = McpProcess::spawn("lead", repo.path(), home.path());

    // In-flight worktree: closing the workspace removes it from disk.
    let in_flight = create_assignment(&mut mcp, worktree_create(&head, &["src/engine/**"], &[]));
    let dispatched = dispatch_worktree(&mut mcp, &in_flight);
    let worktree = dispatched["result"]["lease"]["worktree"].as_str().unwrap().to_string();
    let closed = mcp.tools_call(
        json!(3),
        "workspace_close",
        json!({"assignment": in_flight}),
    );
    assert!(closed.get("error").is_none(), "{closed}");
    assert_eq!(closed["result"]["lease"]["worktreeAction"], "removed");
    assert!(
        !std::path::Path::new(&worktree).exists(),
        "in-flight worktree is cleaned from disk"
    );
    let released = ledger_events_of_type(home.path(), "writer_lease_released");
    assert_eq!(released.last().unwrap()["worktreeAction"], "removed");

    // Handed-back candidate: the worktree is retained as integration evidence.
    let candidate = create_assignment(&mut mcp, worktree_create(&head, &["src/ui/**"], &[]));
    let dispatched = dispatch_worktree(&mut mcp, &candidate);
    let candidate_worktree = dispatched["result"]["lease"]["worktree"]
        .as_str()
        .unwrap()
        .to_string();
    common::seed_ledger_event(
        home.path(),
        json!({
            "type": "assignment_handed_back",
            "assignment": candidate,
            "summary": "peer handback awaiting manual integration"
        }),
    );
    let closed = mcp.tools_call(
        json!(5),
        "workspace_close",
        json!({"assignment": candidate}),
    );
    assert!(closed.get("error").is_none(), "{closed}");
    assert_eq!(closed["result"]["lease"]["worktreeAction"], "retained");
    assert!(
        std::path::Path::new(&candidate_worktree).exists(),
        "a handed-back candidate worktree is retained on disk"
    );

    // Both leases are freed: a fresh exclusive dispatch succeeds.
    let writable = create_assignment(&mut mcp, valid_create(&head));
    let exclusive = mcp.tools_call(
        json!(6),
        "assignment_dispatch",
        json!({"assignment": writable}),
    );
    assert!(
        exclusive.get("error").is_none(),
        "released leases free the checkout for a new writer: {exclusive}"
    );

    // Closing again is refused: the lease is already released.
    let again = mcp.tools_call(
        json!(7),
        "workspace_close",
        json!({"assignment": in_flight}),
    );
    let message = error_message(&again);
    assert!(
        message.contains("no active writer lease"),
        "double close names the missing lease: {message}"
    );
}
