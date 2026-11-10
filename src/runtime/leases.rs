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

// ---------------------------------------------------------------------------
// Isolation mode
// ---------------------------------------------------------------------------

/// The dispatch-time isolation request of a writable assignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// Default: the single in-checkout exclusive writer.
    Checkout,
    /// `isolation: "worktree"`: one of up to [`LEASE_CAP`] isolated writers.
    Worktree,
}

/// Parses the optional `isolation` argument of `assignment_dispatch`.
pub fn parse_isolation(args: &Value) -> Result<Isolation, RpcError> {
    match args.get("isolation") {
        None | Some(Value::Null) => Ok(Isolation::Checkout),
        Some(Value::String(s)) => match s.trim() {
            "checkout" => Ok(Isolation::Checkout),
            "worktree" => Ok(Isolation::Worktree),
            other => Err(RpcError::invalid_params(format!(
                "assignment_dispatch: `isolation` must be \"worktree\" or \"checkout\", got `{other}`"
            ))),
        },
        Some(_) => Err(RpcError::invalid_params(
            "assignment_dispatch: `isolation` must be the string \"worktree\" or \"checkout\"",
        )),
    }
}

// ---------------------------------------------------------------------------
// Scope glob matching (local implementation; the runtime takes no extra crates)
// ---------------------------------------------------------------------------

/// Matches a `*`/`?`/`**` glob against a '/'-separated repository-relative path.
/// `*` and `?` stay within one component; a lone `**` component spans components.
pub fn glob_matches(glob: &str, path: &str) -> bool {
    fn segment(pat: &[char], text: &[char]) -> bool {
        let (mut p, mut t) = (0usize, 0usize);
        let (mut star, mut mark) = (None::<usize>, 0usize);
        while t < text.len() {
            if p < pat.len() && (pat[p] == '?' || pat[p] == text[t]) {
                p += 1;
                t += 1;
            } else if p < pat.len() && pat[p] == '*' {
                star = Some(p);
                mark = t;
                p += 1;
            } else if let Some(s) = star {
                p = s + 1;
                mark += 1;
                t = mark;
            } else {
                return false;
            }
        }
        while p < pat.len() && pat[p] == '*' {
            p += 1;
        }
        p == pat.len()
    }

    fn rec(pat: &[Vec<char>], segs: &[Vec<char>]) -> bool {
        if pat.is_empty() {
            return segs.is_empty();
        }
        if pat[0] == vec!['*', '*'] {
            return (0..=segs.len()).any(|skip| rec(&pat[1..], &segs[skip..]));
        }
        if segs.is_empty() {
            return false;
        }
        segment(&pat[0], &segs[0]) && rec(&pat[1..], &segs[1..])
    }

    let pat: Vec<Vec<char>> = glob
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().collect())
        .collect();
    let segs: Vec<Vec<char>> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().collect())
        .collect();
    rec(&pat, &segs)
}

/// The successive literal component prefixes of a glob ("src/ui/**" ->
/// ["src", "src/ui"]; a fully literal glob yields just itself).
fn literal_prefixes(glob: &str) -> Vec<String> {
    let mut prefixes = Vec::new();
    let mut cur = String::new();
    for comp in glob.split('/').filter(|s| !s.is_empty()) {
        if comp.contains('*') || comp.contains('?') {
            break;
        }
        if !cur.is_empty() {
            cur.push('/');
        }
        cur.push_str(comp);
        prefixes.push(cur.clone());
    }
    prefixes
}

/// Two scope entries share at least one repository path: either one matches a
/// literal path covered by the other's leading structure.
pub fn globs_overlap(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    literal_prefixes(a).iter().any(|p| glob_matches(b, p))
        || literal_prefixes(b).iter().any(|p| glob_matches(a, p))
}

/// Rejects a worktree dispatch whose declared Write Scope / `serialOnly` paths
/// collide with an active worktree lease: paths inside another lease's
/// `serialOnly` are `serial_path`; anything overlapping another lease's Write
/// Scope is `scope_overlap` (spec user story 21).
pub fn check_scope_disjointness(
    tool: &str,
    new_scope: &[String],
    new_serial: &[String],
    peers: &[(String, Vec<String>, Vec<String>)],
) -> Result<(), RpcError> {
    let entries: Vec<&String> = new_scope.iter().chain(new_serial.iter()).collect();
    for (peer, scope, serial) in peers {
        for entry in &entries {
            if serial.iter().any(|s| globs_overlap(entry, s)) {
                return Err(RpcError::invalid_params(format!(
                    "{tool}: serial_path: declared path `{entry}` falls inside the serialOnly scope of the active worktree lease for `{peer}`; serial-only paths admit no concurrent writers"
                )));
            }
            if scope.iter().any(|w| globs_overlap(entry, w)) {
                return Err(RpcError::invalid_params(format!(
                    "{tool}: scope_overlap: declared path `{entry}` overlaps the Write Scope of the active worktree lease for `{peer}`; worktree writers require pairwise-disjoint scopes"
                )));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Worktree provisioning
// ---------------------------------------------------------------------------

/// The materialized isolated workspace of one worktree lease.
#[derive(Debug, Clone)]
pub struct WorktreeProvision {
    pub worktree: PathBuf,
    pub cargo_target_dir: PathBuf,
    pub worktree_include: Vec<String>,
}

/// Creates the isolated git worktree for `assignment` at `base_commit` under
/// `<project_dir>/worktrees/<asg_id>/` and provisions the warm build cache
/// (shared `CARGO_TARGET_DIR` under Hotpot Home).
pub fn provision_worktree(
    project_root: &Path,
    project_dir: &Path,
    assignment: &str,
    base_commit: &str,
) -> Result<WorktreeProvision, RpcError> {
    let worktree = project_dir.join("worktrees").join(assignment);
    if let Some(parent) = worktree.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| RpcError::internal(format!("failed to create worktrees dir: {e}")))?;
    }
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "add", "--detach"])
        .arg(&worktree)
        .arg(base_commit)
        .output()
        .map_err(|e| RpcError::internal(format!("failed to invoke git: {e}")))?;
    if !output.status.success() {
        return Err(RpcError::internal(format!(
            "assignment_dispatch: failed to create isolated worktree {}: {}",
            worktree.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    // Warm build cache: one shared CARGO_TARGET_DIR for the project's worktree
    // peers, so a Gate run inside a fresh worktree skips the cold-build penalty.
    let cargo_target_dir = project_dir.join("cache").join("cargo-target");
    std::fs::create_dir_all(&cargo_target_dir).map_err(|e| {
        RpcError::internal(format!("failed to create shared CARGO_TARGET_DIR: {e}"))
    })?;

    Ok(WorktreeProvision {
        worktree,
        cargo_target_dir,
        worktree_include: Vec::new(),
    })
}

/// Removes an isolated worktree from disk and git's worktree registry.
pub fn remove_worktree(project_root: &Path, worktree: &Path) -> Result<(), RpcError> {
    let path = worktree.to_string_lossy().to_string();
    if git_stdout(project_root, &["worktree", "remove", "--force", &path]).is_err() {
        let _ = git_stdout(project_root, &["worktree", "prune"]);
        std::fs::remove_dir_all(worktree).map_err(|e| {
            RpcError::internal(format!(
                "failed to remove worktree {}: {e}",
                worktree.display()
            ))
        })?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_covers_directory_trees_and_single_segments() {
        assert!(glob_matches("src/**", "src/engine/mod.rs"));
        assert!(glob_matches("src/**", "src"));
        assert!(!glob_matches("src/**", "docs/readme.md"));
        assert!(glob_matches("src/*", "src/main.rs"));
        assert!(!glob_matches("src/*", "src/ui/main.rs"));
        assert!(glob_matches("**", "any/deep/path.rs"));
        assert!(glob_matches("*.lock", "Cargo.lock"));
        assert!(!glob_matches("*.lock", "src/Cargo.lock"));
        assert!(glob_matches("src/*.rs", "src/main.rs"));
        assert!(!glob_matches("src/*.rs", "src/main.md"));
    }

    #[test]
    fn globs_overlap_detects_shared_and_disjoint_scope_trees() {
        assert!(globs_overlap("src/**", "src/ui/**"));
        assert!(globs_overlap("src/ui/**", "src/**"));
        assert!(!globs_overlap("src/engine/**", "src/ui/**"));
        assert!(globs_overlap("Cargo.lock", "Cargo.lock"));
        assert!(!globs_overlap("Cargo.lock", "src/**"));
        assert!(globs_overlap("**", "docs/**"));
        assert!(!globs_overlap("docs/*", "pkg-a/**"));
    }
}

