//! Bridge to `herdr-core` (`hc`) persistent terminals.
//!
//! The `hc` binary is injectable for tests (and operators with custom installs): the
//! `HOTPOT_HC` environment variable names the binary directly, otherwise `hc` is discovered
//! on `PATH` (ticket 06).

use crate::provisioner::{PlanError, find_on_path};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
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
/// Every failure path carries the captured `hc` stderr in the error — capturing it via
/// `.output()` must not also discard it.
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
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if stderr.contains("unexpected argument '--env'") {
        let fallback = rewrite_for_herdr_core(args);
        let retry = Command::new(&hc)
            .arg("run")
            .args(&fallback)
            .output()
            .map_err(|e| PlanError::new("io", format!("failed to invoke hc run: {e}")))?;
        if retry.status.success() {
            return Ok(());
        }
        let retry_stderr = String::from_utf8_lossy(&retry.stderr).trim().to_string();
        return Err(PlanError::new(
            "hc-run-failed",
            hc_run_failure_detail(&retry.status, &retry_stderr),
        ));
    }
    Err(PlanError::new(
        "hc-run-failed",
        hc_run_failure_detail(&out.status, &stderr),
    ))
}

/// The `hc-run-failed` diagnostic: the exit status plus `hc`'s own stderr when it said
/// anything (an empty stderr never trails a dangling separator).
fn hc_run_failure_detail(status: &std::process::ExitStatus, stderr: &str) -> String {
    if stderr.is_empty() {
        format!("hc run exited with {status}")
    } else {
        format!("hc run exited with {status}: {stderr}")
    }
}

/// Rewrites `hc run` argv carrying `--env K=V ... -- <vendor> ...` into
/// `hc run ... -- env K=V ... <vendor> ...` for `herdr-core` binaries whose `hc run` does not
/// take `--env` directly, and maps `--disallow-tool` to `--disallowedTools` for Claude Code.
/// For a recognized vendor command, the Role Home scoping is translated into the vendor's
/// canonical configuration variable (`CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `PI_CODING_AGENT_DIR`)
/// while the `HOME=<role_home>` override is dropped, so the live terminal inherits the
/// operator's real `HOME` (macOS Keychain access for Claude Code, local toolchains) instead
/// of being jailed behind the Role Home path.
fn rewrite_for_herdr_core(args: &[String]) -> Vec<String> {
    let sep = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (before, after) = (&args[..sep], &args[sep..]);
    let vendor = after.get(1).map(String::as_str).and_then(vendor_home_var);
    let mut hc_flags = Vec::new();
    let mut env_pairs = Vec::new();
    let mut role_home: Option<String> = None;
    let mut i = 0;
    while i < before.len() {
        if before[i] == "--env" && i + 1 < before.len() {
            let pair = &before[i + 1];
            if vendor.is_some()
                && let Some(value) = pair.strip_prefix("HOME=")
            {
                // Known vendor: drop the HOME override (the operator's real HOME must stay
                // intact) and remember the Role Home for the vendor variable below.
                role_home = Some(value.to_string());
                i += 2;
                continue;
            }
            env_pairs.push(pair.clone());
            i += 2;
        } else {
            hc_flags.push(before[i].clone());
            i += 1;
        }
    }
    if let (Some(var), Some(home)) = (vendor, role_home)
        && !env_pairs.iter().any(|p| p.starts_with(var) && p[var.len()..].starts_with('='))
    {
        env_pairs.push(format!("{var}={home}"));
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

/// The vendor's canonical Role Home configuration variable, inferred from the vendor command
/// binary name following `--` on the `hc run` argv. `None` for unrecognized vendors (their
/// env pairs pass through the rewrite unchanged).
fn vendor_home_var(command: &str) -> Option<&'static str> {
    let name = Path::new(command).file_name()?.to_str()?;
    match name {
        "codex" => Some("CODEX_HOME"),
        "claude" => Some("CLAUDE_CONFIG_DIR"),
        "pi" => Some("PI_CODING_AGENT_DIR"),
        _ => None,
    }
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

#[cfg(test)]
mod tests {
    use super::rewrite_for_herdr_core;

    /// Table-driven argv fixtures for the real-`herdr-core` fallback rewrite (ticket 03):
    /// every row pins the full rewritten argv so the vendor translation, the dropped `HOME`
    /// override, and the untouched label/cwd/flag arguments are all part of the contract.
    #[test]
    fn rewrite_scopes_vendor_home_and_preserves_operator_home() {
        let cases: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![(
            "codex: vendor var kept, HOME override dropped",
            vec![
                "--bg",
                "--keep",
                "--cwd",
                "/repo",
                "--label",
                "hotpot.v1=true",
                "--label",
                "hotpot.agent=codex",
                "--env",
                "HOME=/roles/codex/lead",
                "--env",
                "CODEX_HOME=/roles/codex/lead",
                "--",
                "/usr/local/bin/codex",
            ],
            vec![
                "--bg",
                "--keep",
                "--cwd",
                "/repo",
                "--label",
                "hotpot.v1=true",
                "--label",
                "hotpot.agent=codex",
                "--",
                "env",
                "CODEX_HOME=/roles/codex/lead",
                "/usr/local/bin/codex",
            ],
        ), (
            "claude: vendor var kept, HOME dropped, deny flags mapped",
            vec![
                "--cwd",
                "/repo",
                "--label",
                "hotpot.agent=claude",
                "--env",
                "HOME=/roles/claude/peer",
                "--env",
                "CLAUDE_CONFIG_DIR=/roles/claude/peer",
                "--",
                "/opt/claude",
                "--disallow-tool",
                "Task",
            ],
            vec![
                "--cwd",
                "/repo",
                "--label",
                "hotpot.agent=claude",
                "--",
                "env",
                "CLAUDE_CONFIG_DIR=/roles/claude/peer",
                "/opt/claude",
                "--disallowedTools",
                "Task",
            ],
        ), (
            "pi: HOME translated into PI_CODING_AGENT_DIR",
            vec![
                "--cwd",
                "/repo",
                "--env",
                "HOME=/roles/pi/peer",
                "--",
                "/usr/bin/pi",
                "--no-extensions",
            ],
            vec![
                "--cwd",
                "/repo",
                "--",
                "env",
                "PI_CODING_AGENT_DIR=/roles/pi/peer",
                "/usr/bin/pi",
                "--no-extensions",
            ],
        ), (
            "unknown vendor: env pairs pass through unchanged, HOME kept",
            vec![
                "--cwd",
                "/repo",
                "--env",
                "HOME=/roles/other/peer",
                "--env",
                "FOO=bar",
                "--",
                "/opt/bin/someagent",
                "-x",
            ],
            vec![
                "--cwd",
                "/repo",
                "--",
                "env",
                "HOME=/roles/other/peer",
                "FOO=bar",
                "/opt/bin/someagent",
                "-x",
            ],
        ), (
            "no env pairs: rewrites to today's shape without inventing a variable",
            vec!["--bg", "--cwd", "/repo", "--", "/opt/claude", "-p"],
            vec!["--bg", "--cwd", "/repo", "--", "/opt/claude", "-p"],
        )];

        for (name, input, expected) in cases {
            let input: Vec<String> = input.into_iter().map(str::to_string).collect();
            let expected: Vec<String> = expected.into_iter().map(str::to_string).collect();
            assert_eq!(
                rewrite_for_herdr_core(&input),
                expected,
                "case `{name}` must rewrite to the vendor-scoped argv"
            );
        }
    }
}
