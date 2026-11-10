//! Writer Leases (spec: "Assignment & Writer Lease State Machine").
//!
//! A writable `Assignment` must hold a `WriterLease` before its `Peer` may write:
//! either the single `InCheckoutExclusive` lease (granted only after clean-HEAD
//! verification of the project checkout) or one of up to three pairwise
//! scope-disjoint `IsolatedWorktree` leases provisioned with a warm build cache.
//! Read-only assignments (no Write Scope) acquire no lease.

use crate::runtime::RpcError;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Locks checkout writes for all other seats; max 1 active writer.
pub const IN_CHECKOUT_EXCLUSIVE: &str = "InCheckoutExclusive";
/// Isolated git worktree writers; max [`LEASE_CAP`], disjoint scopes.
pub const ISOLATED_WORKTREE: &str = "IsolatedWorktree";
/// Maximum concurrent isolated worktree writers.
pub const LEASE_CAP: usize = 3;

/// One active `WriterLease`, projected from the ledger's lease events.
#[derive(Debug, Clone)]
pub struct WriterLease {
    pub assignment: String,
    pub mode: String,
    pub worktree: Option<String>,
    pub cargo_target_dir: Option<String>,
    pub worktree_include: Vec<String>,
}

/// `git rev-parse HEAD` of the project checkout.
pub fn head_commit(project_root: &Path) -> Result<String, RpcError> {
    git_stdout(project_root, &["rev-parse", "HEAD"])
        .map(|s| s.trim().to_string())
        .map_err(|e| RpcError::internal(format!("failed to inspect project HEAD: {e}")))
}

/// Entries reported by `git status --porcelain=v1 --untracked-files=all`.
pub fn porcelain_entries(project_root: &Path) -> Result<Vec<String>, RpcError> {
    git_stdout(
        project_root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .map(|out| {
        out.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    })
    .map_err(|e| RpcError::internal(format!("failed to inspect the working tree: {e}")))
}

/// Verifies the `InCheckoutExclusive` preconditions of spec user story 20: the
/// checkout `HEAD` must equal the assignment's verified `baseCommit` and
/// `git status --porcelain=v1 --untracked-files=all` must be empty.
pub fn verify_checkout_preconditions(
    project_root: &Path,
    base_commit: &str,
) -> Result<(), RpcError> {
    let head = head_commit(project_root)?;
    if head != base_commit {
        return Err(RpcError::invalid_params(format!(
            "assignment_dispatch: git precondition failed: HEAD `{head}` does not equal the assignment baseCommit `{base_commit}`; the checkout must sit at the verified baseCommit before an exclusive writer lease is granted"
        )));
    }
    let dirty = porcelain_entries(project_root)?;
    if !dirty.is_empty() {
        let sample: Vec<String> = dirty.iter().take(3).cloned().collect();
        return Err(RpcError::invalid_params(format!(
            "assignment_dispatch: git precondition failed: working tree is not clean (`git status --porcelain=v1 --untracked-files=all` reports {} entr{}): {}; the checkout must be clean before an exclusive writer lease is granted",
            dirty.len(),
            if dirty.len() == 1 { "y" } else { "ies" },
            sample.join(", ")
        )));
    }
    Ok(())
}

/// Runs `git -C <repo> <args..>` and returns stdout on success, stderr otherwise.
fn git_stdout(project_root: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(args)
        .output()
        .map_err(|e| format!("failed to invoke git: {e}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// The `writer_lease_acquired` event payload for one lease acquisition.
pub fn lease_acquired_event(
    assignment: &str,
    mode: &str,
    worktree: Option<&Path>,
    cargo_target_dir: Option<&Path>,
    worktree_include: &[String],
) -> Value {
    let mut event = json!({
        "type": "writer_lease_acquired",
        "assignment": assignment,
        "mode": mode,
    });
    if let Some(w) = worktree {
        event["worktree"] = json!(w.to_string_lossy());
    }
    if let Some(c) = cargo_target_dir {
        event["cargoTargetDir"] = json!(c.to_string_lossy());
    }
    if !worktree_include.is_empty() {
        event["worktreeInclude"] = json!(worktree_include);
    }
    event
}
