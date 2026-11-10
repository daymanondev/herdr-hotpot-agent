//! Bridge to `herdr-core` (`hc`) persistent terminals.

use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HcTerminalSummary {
    pub address: String,
    pub name: Option<String>,
    pub status: Option<String>,
    pub cwd: Option<String>,
}

/// Lists live terminals from `hc ls --json` when `hc` is available on PATH.
pub fn list_terminals() -> Vec<HcTerminalSummary> {
    let output = Command::new("hc").args(["ls", "--json"]).output();
    let Ok(out) = output else {
        return Vec::new();
    };
    if !out.status.success() {
        return Vec::new();
    }
    serde_json::from_slice::<Vec<HcTerminalSummary>>(&out.stdout).unwrap_or_default()
}

/// Sends prompt text to a live `hc` terminal (`hc send <target> <message>`).
pub fn send_terminal_prompt(target: &str, message: &str) -> Result<(), String> {
    let out = Command::new("hc")
        .args(["send", target, message])
        .output()
        .map_err(|e| format!("failed to invoke hc send: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
