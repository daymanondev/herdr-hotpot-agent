//! Detached Gate execution against a `Candidate` commit (spec user story 25).
//!
//! `gate_run` (a Lead tool) appends `gate_started` and spawns
//! `hotpot gate-runner` as a detached background process (its own process
//! group), so the Gate keeps running and records its completion even if the
//! MCP session exits. The runner re-executes the assignment's Gate command
//! against the candidate commit — inside the assignment's own workspace while
//! its `HEAD` still is the candidate (keeping an isolated worktree's warm
//! build cache), otherwise in a throwaway detached worktree checked out at
//! that commit — and appends exactly one `gate_completed` ledger event:
//!
//! - `green`     the Gate exited 0;
//! - `red`       the Gate exited non-zero; a masked log tail is written under
//!   Hotpot Home and referenced from the event;
//! - `uncertain` the Gate crashed, was signalled, or exceeded its timeout.

use super::{Assignment, Ledger, RuntimeState, masking};
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Verdict: the Gate exited 0.
pub const GREEN: &str = "green";
/// Verdict: the Gate exited non-zero; a masked tail is on disk.
pub const RED: &str = "red";
/// Verdict: the Gate crashed, was signalled, or timed out.
pub const UNCERTAIN: &str = "uncertain";
/// Default gate budget before an uncertain timeout (ten minutes).
pub const DEFAULT_TIMEOUT_MS: u64 = 600_000;
/// Combined stdout+stderr bytes kept in a red gate tail.
const TAIL_LIMIT_BYTES: usize = 8 * 1024;

/// Spawns `hotpot gate-runner` detached so the Gate outlives the MCP session.
pub fn spawn_detached(
    state_dir: &Path,
    project_root: &Path,
    assignment: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()
        .map_err(|e| format!("failed to resolve the running hotpot binary: {e}"))?;
    Command::new(exe)
        .arg("gate-runner")
        .arg("--state-dir")
        .arg(state_dir)
        .arg("--project")
        .arg(project_root)
        .arg("--assignment")
        .arg(assignment)
        .arg("--timeout-ms")
        .arg(timeout_ms.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to spawn the detached gate runner: {e}"))
}

/// Runs the Gate of `assignment_id` to completion and appends the
/// `gate_completed` event. This is the `hotpot gate-runner` entry point.
pub fn run(
    state_dir: &Path,
    project_root: &Path,
    assignment_id: &str,
    timeout_ms: u64,
) -> Result<(), String> {
    let ledger = Ledger::open(state_dir, project_root).map_err(|e| e.to_string())?;
    let state = RuntimeState::load(&ledger);
    let Some(asg) = state.assignment(assignment_id) else {
        return Err(format!("gate-runner: unknown assignment `{assignment_id}`"));
    };
    // The ordinal of this gate run (1-based) names its tail file.
    let run_no = ledger
        .read_events()
        .iter()
        .filter(|e| {
            e.get("type").and_then(Value::as_str) == Some("gate_started")
                && e.get("assignment").and_then(Value::as_str) == Some(assignment_id)
        })
        .count();
    let gates_dir = ledger.project_dir().join("gates").join(assignment_id);
    fs::create_dir_all(&gates_dir).map_err(|e| format!("gate-runner: {e}"))?;

    // The Gate runs against the candidate commit: the handed-back `Candidate`
    // when there is one, otherwise the reviewed baseCommit (read-only briefs
    // such as a Reviewer hand back no candidate of their own).
    let target = asg
        .candidate
        .clone()
        .unwrap_or_else(|| asg.base_commit.clone());
    let mut detached: Option<PathBuf> = None;
    let workspace = match workspace_at(asg, &target) {
        Some(path) => path,
        None => {
            let checkout = gates_dir.join(format!("checkout-{run_no}"));
            if !checkout_worktree(project_root, &checkout, &target) {
                // No workspace and no checkoutable candidate: the run cannot
                // even start, which is an uncertain completion, not silence.
                ledger
                    .append(json!({
                        "type": "gate_completed",
                        "assignment": assignment_id,
                        "verdict": UNCERTAIN,
                        "reason": "workspace-missing",
                    }))
                    .map_err(|e| e.to_string())?;
                return Ok(());
            }
            detached = Some(checkout.clone());
            checkout
        }
    };

    let started = Instant::now();
    let (verdict, exit_code, reason, output) =
        execute(&asg.gate, &workspace, timeout_ms, &gates_dir, run_no);
    let mut event = json!({
        "type": "gate_completed",
        "assignment": assignment_id,
        "verdict": verdict,
        "durationMs": started.elapsed().as_millis() as u64,
    });
    if let Some(code) = exit_code {
        event["exitCode"] = json!(code);
    }
    if let Some(reason) = reason {
        event["reason"] = json!(reason);
    }
    if verdict == RED {
        let tail_path = gates_dir.join(format!("tail-{run_no}.txt"));
        let tail = tail_bytes(&output, TAIL_LIMIT_BYTES);
        let masked = masking::mask_text(&String::from_utf8_lossy(tail));
        fs::write(&tail_path, masked).map_err(|e| format!("gate-runner: {e}"))?;
        event["tail"] = json!(tail_path.to_string_lossy());
    }

    // The throwaway checkout is torn down before the completion event lands, so
    // `gate_completed` always describes a fully finished run — scratch included
    // — and a watcher reacting to it never observes a half-cleaned gates dir.
    if let Some(checkout) = detached {
        let path = checkout.to_string_lossy().to_string();
        let _ = Command::new("git")
            .arg("-C")
            .arg(project_root)
            .args(["worktree", "remove", "--force", &path])
            .output();
        let _ = fs::remove_dir_all(&checkout);
    }
    ledger.append(event).map_err(|e| e.to_string())?;
    Ok(())
}

/// The assignment's recorded workspace when it still exists on disk and its
/// `HEAD` is exactly `target` — the fast path that reuses the peer's own
/// (warm) workspace for the independent gate run.
fn workspace_at(asg: &Assignment, target: &str) -> Option<PathBuf> {
    let workspace = asg.workspace.as_deref()?.trim();
    if workspace.is_empty() {
        return None;
    }
    let path = PathBuf::from(workspace);
    if !path.is_dir() {
        return None;
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(&path)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (head == target).then_some(path)
}

/// Creates a throwaway detached worktree at `commit`; false on failure.
fn checkout_worktree(project_root: &Path, checkout: &Path, commit: &str) -> bool {
    let _ = fs::remove_dir_all(checkout);
    Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["worktree", "add", "--detach"])
        .arg(checkout)
        .arg(commit)
        .output()
        .is_ok_and(|out| out.status.success())
}

/// Executes `sh -c <gate>` in `workspace` with stdout/stderr captured to
/// per-run files (no pipe deadlocks), bounded by `timeout_ms`; returns the
/// verdict, exit code, uncertain reason, and the combined output.
fn execute(
    gate: &str,
    workspace: &Path,
    timeout_ms: u64,
    gates_dir: &Path,
    run_no: usize,
) -> (&'static str, Option<i32>, Option<String>, Vec<u8>) {
    let stdout_path = gates_dir.join(format!("run-{run_no}.out"));
    let stderr_path = gates_dir.join(format!("run-{run_no}.err"));
    let child = (|| -> Option<Child> {
        let out_file = fs::File::create(&stdout_path).ok()?;
        let err_file = fs::File::create(&stderr_path).ok()?;
        Command::new("sh")
            .arg("-c")
            .arg(gate)
            .current_dir(workspace)
            .stdout(Stdio::from(out_file))
            .stderr(Stdio::from(err_file))
            .spawn()
            .ok()
    })();
    let Some(child) = child else {
        return (UNCERTAIN, None, Some("spawn-failed".to_string()), Vec::new());
    };

    let (verdict, exit_code, reason) = wait_for(child, timeout_ms);
    let mut output = Vec::new();
    for path in [&stdout_path, &stderr_path] {
        if let Ok(mut file) = fs::File::open(path) {
            let _ = file.read_to_end(&mut output);
        }
        let _ = fs::remove_file(path);
    }
    (verdict, exit_code, reason, output)
}

/// Polls the gate child until it exits or the timeout elapses.
fn wait_for(mut child: Child, timeout_ms: u64) -> (&'static str, Option<i32>, Option<String>) {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(1));
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return match status.code() {
                    Some(0) => (GREEN, Some(0), None),
                    Some(code) => (RED, Some(code), None),
                    None => (UNCERTAIN, None, Some("signal-terminated".to_string())),
                };
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return (UNCERTAIN, None, Some("timeout".to_string()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(_) => return (UNCERTAIN, None, Some("wait-failed".to_string())),
        }
    }
}

/// The last `limit` bytes of the combined gate output (the diagnostic tail).
fn tail_bytes(output: &[u8], limit: usize) -> &[u8] {
    &output[output.len().saturating_sub(limit)..]
}
