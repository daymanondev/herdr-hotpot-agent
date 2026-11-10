# ADR 0005: Commit-Bound Assignments, Writer Leases, Warm Worktrees, and Manual Lead Integration

- **Status**: accepted

Writable work in `hotpot` is governed by commit-bound `Assignments`, exclusive `Writer Leases` (single in-checkout writer by default, or up to three isolated worktree writers with warm build caches), and manual integration by `Lead`.

## Context

Auto-merging parallel agent branches on green tests causes semantic conflicts on the main branch, while spawning cold Git worktrees for every single-scope task forces expensive `target/` or `node_modules/` rebuilds. Conversely, sharing a single checkout without a lease allows `Lead` doc edits to dirty `git status --porcelain` while an `Engineer` `Peer` is preparing a commit.

## Decision

1. **Commit-Bound Preconditions & Handoff**:
   - Every writable `Assignment` records an verified `baseCommit` and requires a clean `git status --porcelain=v1 --untracked-files=all` before dispatch.
   - A `Peer` reports completion exclusively via the `handoff` MCP tool (never by prose). `hotpot` derives the `Candidate` commit (`baseCommit..HEAD`), verifies working-tree cleanliness, computes changed paths, and flags any edits outside `Write Scope` as `scope.exceeded`.
2. **Two Writer Lease Modes**:
   - **Default (In-Checkout Single Writer)**: At most one writable `Engineer` `Peer` runs in the `Project` checkout at a time. While the lease is held, all other seats (including `Lead`) are barred from modifying files in that checkout until `handoff` completes.
   - **Isolated Writers (`isolation: "worktree"`, cap 3)**: `Lead` may dispatch up to three concurrent writable `Peers` in linked Git worktrees when their `Write Scope` and `serialOnly` globs do not overlap (`writer_exclusive`, `lease_cap`, `scope_overlap`, `serial_path`). Worktrees are provisioned with a **Warm Build Cache** (shared `CARGO_TARGET_DIR` for Rust and `.worktreeinclude` / `node_modules` symlinks) so `Gate` runs stay fast.
3. **Manual Integration by `Lead`**:
   - `hotpot` never auto-merges, resets, rebases, or stashes. After `Peer` verification (and optional independent `gate_run` or `Reviewer` `Peer`), `Lead` manually integrates the accepted `Candidate` commit into `<project-root>`.
