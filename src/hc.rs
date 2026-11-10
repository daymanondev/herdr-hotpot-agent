//! Bridge to `herdr-core` (`hc`) persistent terminals.
//!
//! The `hc` binary is injectable for tests (and operators with custom installs): the
//! `HOTPOT_HC` environment variable names the binary directly, otherwise `hc` is discovered
//! on `PATH` (ticket 06).

use crate::provisioner::{PlanError, find_on_path};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HcTerminalSummary {
    pub address: String,
    pub name: Option<String>,
    pub status: Option<String>,
    pub cwd: Option<String>,
    /// Labels attached to the terminal (e.g. the canonical `hotpot.*` set). Absent when the
    /// listing carries no labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,
}

impl HcTerminalSummary {
    /// The terminal's `hotpot.*` label value, if any.
    pub fn label(&self, key: &str) -> Option<&str> {
        self.labels.as_ref()?.get(key).map(String::as_str)
    }
}

/// Resolves the `hc` binary: `$HOTPOT_HC` when set (non-empty), else the first executable
/// `hc` on `PATH`.
pub fn hc_binary() -> Option<PathBuf> {
    if let Some(raw) = std::env::var_os("HOTPOT_HC")
        && !raw.is_empty()
    {
        return Some(PathBuf::from(raw));
    }
    find_on_path("hc")
}

/// Lists live terminals from `hc ls --json` when `hc` is available on PATH.
pub fn list_terminals() -> Vec<HcTerminalSummary> {
    let Some(hc) = hc_binary() else {
        return Vec::new();
    };
    let output = Command::new(&hc).args(["ls", "--json"]).output();
    let Ok(out) = output else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    serde_json::from_slice::<Vec<HcTerminalSummary>>(&out.stdout).unwrap_or_default()
}

/// Lists terminals fail-closed: any failure to run `hc ls --json` (binary missing, non-zero
/// exit, unparseable output) is an error instead of an empty list — seat-launch decisions
/// (lead reuse / duplicate halt) must never mistake a broken listing for "no terminals".
pub fn list_terminals_strict() -> Result<Vec<HcTerminalSummary>, PlanError> {
    let hc = hc_binary().ok_or_else(|| {
        PlanError::new(
            "hc-not-found",
            "hc is not available (neither HOTPOT_HC nor PATH names it); cannot list terminals",
        )
    })?;
    let out = Command::new(&hc)
        .args(["ls", "--json"])
        .output()
        .map_err(|e| PlanError::new("io", format!("failed to invoke hc ls: {e}")))?;
    if !out.status.success() {
        return Err(PlanError::new(
            "hc-ls-failed",
            format!("hc ls --json exited with {status}", status = out.status),
        ));
    }
    serde_json::from_slice::<Vec<HcTerminalSummary>>(&out.stdout).map_err(|e| {
        PlanError::new(
            "hc-ls-unparseable",
            format!("hc ls --json returned unparseable output: {e}"),
        )
    })
}

/// Creates a persistent terminal via `hc run` with caller-assembled arguments (labels, env,
/// cwd, and the vendor command after `--`); the `hc run` flag vocabulary rides in `args` so
/// this stays a thin process bridge. When the installed `herdr-core` (`hc`) rejects `--env`
/// on `hc run`, falls back to passing the environment pairs via `env K=V ...` after `--` and
/// translating `--disallow-tool` to `--disallowedTools` for real `claude` CLI compatibility.
pub fn run_terminal(args: &[String]) -> Result<(), PlanError> {
    let hc = hc_binary().ok_or_else(|| {
        PlanError::new(
            "hc-not-found",
            "hc is not available (neither HOTPOT_HC nor PATH names it); cannot create a terminal",
        )
    })?;
    let out = Command::new(&hc)
        .arg("run")
        .args(args)
        .output()
        .map_err(|e| PlanError::new("io", format!("failed to invoke hc run: {e}")))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("unexpected argument '--env'") {
        let fallback = rewrite_for_herdr_core(args);
        let retry = Command::new(&hc)
            .arg("run")
            .args(&fallback)
            .status()
            .map_err(|e| PlanError::new("io", format!("failed to invoke hc run: {e}")))?;
        if retry.success() {
            return Ok(());
        }
        return Err(PlanError::new(
            "hc-run-failed",
            format!("hc run exited with {retry}"),
        ));
    }
    Err(PlanError::new(
        "hc-run-failed",
        format!("hc run exited with {}", out.status),
    ))
}

/// Rewrites `hc run` argv carrying `--env K=V ... -- <vendor> ...` into
/// `hc run ... -- env K=V ... <vendor> ...` for `herdr-core` binaries whose `hc run` does not
/// take `--env` directly, and maps `--disallow-tool` to `--disallowedTools` for Claude Code.
fn rewrite_for_herdr_core(args: &[String]) -> Vec<String> {
    let sep = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (before, after) = (&args[..sep], &args[sep..]);
    let mut hc_flags = Vec::new();
    let mut env_pairs = Vec::new();
    let mut i = 0;
    while i < before.len() {
        if before[i] == "--env" && i + 1 < before.len() {
            env_pairs.push(before[i + 1].clone());
            i += 2;
        } else {
            hc_flags.push(before[i].clone());
            i += 1;
        }
    }
    hc_flags.push("--".to_string());
    if !env_pairs.is_empty() {
        hc_flags.push("env".to_string());
        hc_flags.extend(env_pairs);
    }
    for arg in after.iter().skip(1) {
        if arg == "--disallow-tool" {
            hc_flags.push("--disallowedTools".to_string());
        } else {
            hc_flags.push(arg.clone());
        }
    }
    hc_flags
}

/// Sends prompt text to a live `hc` terminal (`hc send <target> <message>`).
pub fn send_terminal_prompt(target: &str, message: &str) -> Result<(), String> {
    let Some(hc) = hc_binary() else {
        return Err("hc is not available (neither HOTPOT_HC nor PATH names it)".to_string());
    };
    let out = Command::new(&hc)
        .args(["send", target, message])
        .output()
        .map_err(|e| format!("failed to invoke hc send: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
