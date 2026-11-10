//! `hotpot auth login <codex|claude|pi> <supervisor|lead|peer>`: interactive vendor login
//! scoped to exactly one Seat's Role Home.
//!
//! The vendor login binary is discovered on `PATH` and invoked as `<vendor> login` with its
//! working directory and home environment pointing strictly at the Role Home — so the
//! credentials it writes (codex `auth.json`, claude `.credentials.json`, pi under `$HOME`)
//! land inside that one seat, never in the operator's homes. The process launch is
//! environment-injectable the same way as the `hc` profile runner in `provisioner.rs`:
//! tests (and operators) swap the vendor by putting a shim earlier on `PATH`.

use crate::provisioner::{PlanError, find_on_path};
use crate::role_homes::role_home_dir;
use std::path::Path;
use std::process::Command;

/// The login binary each agent family authenticates through.
fn vendor_login_bin(agent: &str) -> Option<&'static str> {
    match agent {
        "codex" => Some("codex"),
        "claude" => Some("claude"),
        "pi" => Some("pi"),
        _ => None,
    }
}

/// Environment variables scoping a vendor process to `role_home`: `$HOME` always, plus the
/// vendor's canonical home override so credentials cannot leak to the operator's home.
fn vendor_login_env(agent: &str) -> Vec<&'static str> {
    match agent {
        "codex" => vec!["HOME", "CODEX_HOME"],
        "claude" => vec!["HOME", "CLAUDE_CONFIG_DIR"],
        "pi" => vec!["HOME"],
        _ => Vec::new(),
    }
}

/// Validates the seat name pair, returning a stable diagnostic naming the invalid value.
fn validate_seat(agent: &str, role: &str) -> Result<(), PlanError> {
    if vendor_login_bin(agent).is_none() {
        return Err(PlanError::new(
            "invalid-agent",
            format!(
                "invalid agent `{agent}` (expected codex|claude|pi) for `hotpot auth login <agent> <role>`"
            ),
        ));
    }
    if !["supervisor", "lead", "peer"].contains(&role) {
        return Err(PlanError::new(
            "invalid-role",
            format!(
                "invalid role `{role}` (expected supervisor|lead|peer) for `hotpot auth login <agent> <role>`"
            ),
        ));
    }
    Ok(())
}

/// Launches the vendor's interactive login scoped to the `<agent>/<role>` seat's Role Home.
/// Returns the vendor process's exit code (0 on success); callers surface a non-zero code
/// unchanged so the operator sees the vendor's own login failure status.
pub fn login(home: &Path, agent: &str, role: &str) -> Result<i32, PlanError> {
    validate_seat(agent, role)?;

    let role_home = role_home_dir(home, agent, role);
    if !role_home.is_dir() {
        return Err(PlanError::new(
            "role-home-missing",
            format!(
                "Role Home {} does not exist; run `hotpot setup --apply` first",
                role_home.display()
            ),
        ));
    }

    let bin_name = vendor_login_bin(agent).expect("validated above");
    let bin = find_on_path(bin_name).ok_or_else(|| {
        PlanError::new(
            "vendor-not-found",
            format!("no `{bin_name}` binary on PATH; install the {bin_name} CLI to log in"),
        )
    })?;

    let mut cmd = Command::new(&bin);
    cmd.arg("login").current_dir(&role_home);
    for var in vendor_login_env(agent) {
        cmd.env(var, &role_home);
    }

    let status = cmd.status().map_err(|e| {
        PlanError::new(
            "io",
            format!("failed to launch {} login in {}: {e}", bin.display(), role_home.display()),
        )
    })?;
    Ok(status.code().unwrap_or(1))
}
