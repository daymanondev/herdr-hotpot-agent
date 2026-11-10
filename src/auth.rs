//! `hotpot auth login <codex|claude|pi> <supervisor|lead|peer>`: interactive vendor login
//! scoped to exactly one Seat's Role Home, plus the seat authentication diagnostics
//! `hotpot verify` reports.
//!
//! The vendor login binary is discovered on `PATH` and invoked as `<vendor> login` with its
//! working directory and home environment pointing strictly at the Role Home — so the
//! credentials it writes (codex `auth.json`, claude `.credentials.json`, pi under `$HOME`)
//! land inside that one seat, never in the operator's homes. The process launch is
//! environment-injectable the same way as the `hc` profile runner in `provisioner.rs`:
//! tests (and operators) swap the vendor by putting a shim earlier on `PATH`.
//!
//! The diagnostics ([`diagnostics`]) are non-blocking, offline, and secret-free: they are
//! derived from credential-file presence and structural shape only — never from token
//! values, which are never surfaced in any output — and they never affect the `hotpot
//! verify` exit status (Capability Hygiene owns that).

use crate::provisioner::{PlanError, find_on_path};
use crate::role_homes::{role_home_dir, seat_credentials_file};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::MetadataExt;
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
    let code = status.code().unwrap_or(1);
    if code == 0 {
        // Preserve the fresh credential's shape (never its values) so a later
        // `hotpot verify` can detect divergence (AUTH_STATE_DIVERGED).
        preserve_credential_shape(&role_home, agent)?;
    }
    Ok(code)
}

/// Preserves the seat credential's shape record ([`PRESERVED_SHAPE_FILE`]) beside it: byte
/// length plus recursive key names, written atomically. A successful vendor login that did
/// not write the declared credential file preserves nothing.
pub(crate) fn preserve_credential_shape(role_home: &Path, agent: &str) -> Result<(), PlanError> {
    let credential = seat_credentials_file(role_home, agent);
    let Some(shape) = credential_shape(&credential) else {
        return Ok(());
    };
    crate::provisioner::atomic_write_file(
        &role_home.join(PRESERVED_SHAPE_FILE),
        &preserved_shape_record(&shape),
    )
}

/// Renders one shape as the [`PRESERVED_SHAPE_FILE`] record contents (pretty-printed with
/// a trailing newline): byte length plus recursive key names — never any token value.
fn preserved_shape_record(shape: &CredentialShape) -> String {
    let record = json!({ "len": shape.len, "keys": shape.keys });
    serde_json::to_string_pretty(&record).unwrap() + "\n"
}

/// The sidecar record contents for one credential text (the shape of the bytes a seeding
/// step is about to copy, computed before they land in the Role Home) — the same record
/// [`preserve_credential_shape`] writes after a successful `hotpot auth login`.
pub(crate) fn preserved_shape_contents(credential_text: &str) -> String {
    preserved_shape_record(&shape_of_text(credential_text))
}

// ---------------------------------------------------------------------------
// Seat authentication diagnostics (`hotpot verify`)
// ---------------------------------------------------------------------------

/// A structurally complete, seat-owned file-backed credential (the clean state).
pub const AUTH_STATE_CONFIGURED: &str = "configured structurally";
/// No usable credential at the seat's declared path (or one too malformed to use).
pub const AUTH_STATE_LOGIN_REQUIRED: &str = "login-required";
/// The credential path is not an ordinary seat-owned file (symlinked/hardlinked/shared).
pub const AUTH_STATE_LEGACY_SHARED: &str = "legacy-shared-risk";
/// The credential no longer matches the shape record preserved beside it at login time.
pub const AUTH_STATE_DIVERGED: &str = "diverged-file-preserve";
/// The credential declares native OS-keyring storage, unverifiable offline.
pub const AUTH_STATE_KEYRING: &str = "native-keyring unverifiable";

/// Name of the shape record preserved beside a seat credential after a successful
/// `hotpot auth login` (byte length plus recursive key names — never any token value).
pub const PRESERVED_SHAPE_FILE: &str = "credentials.preserved.json";

/// One per-seat diagnostic line: the seat identity, one of the five states, and a secret-free
/// detail. Never blocking — the `hotpot verify` path prints these without counting them
/// toward the exit status (Capability Hygiene owns that).
pub struct AuthDiagnostic {
    pub agent: String,
    pub role: String,
    pub state: &'static str,
    pub detail: String,
}

impl AuthDiagnostic {
    /// The report line printed on stdout: `verify: diag auth <agent>/<role> <state> <detail>`.
    pub fn render(&self) -> String {
        format!(
            "verify: diag auth {}/{} {} {}",
            self.agent, self.role, self.state, self.detail
        )
    }
}

/// Runs the authentication diagnostics over every provisioned seat (agent × role) of the
/// Hotpot Home. Offline and secret-free by construction: no process is spawned and no token
/// value is ever rendered.
pub fn diagnostics(home: &Path) -> Vec<AuthDiagnostic> {
    let mut out = Vec::new();
    for agent in crate::provisioner::AGENTS {
        for role in crate::provisioner::ROLES {
            out.push(seat_diagnostic(home, agent, role));
        }
    }
    out
}

/// The structural fingerprint of one credential file: its byte length and its recursive
/// JSON key names (dotted paths, sorted). Deliberately excludes every value — divergence
/// between a credential and its preserved record is detected from shape alone, so token
/// contents never flow into a comparison or an output line.
#[derive(Debug, PartialEq, Eq)]
struct CredentialShape {
    len: u64,
    keys: Vec<String>,
}

/// Collects the recursive dotted key paths of `value` into `out` (objects only; arrays are
/// traversed without index labels).
fn collect_key_paths(value: &Value, prefix: String, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                out.push(path.clone());
                collect_key_paths(inner, path, out);
            }
        }
        Value::Array(items) => {
            for inner in items {
                collect_key_paths(inner, prefix.clone(), out);
            }
        }
        _ => {}
    }
}

/// The shape of the credential text: byte length plus recursive key names; text that does
/// not parse as JSON still has a shape (its keys are simply empty).
fn shape_of_text(text: &str) -> CredentialShape {
    let mut keys = Vec::new();
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        collect_key_paths(&value, String::new(), &mut keys);
    }
    keys.sort();
    CredentialShape {
        len: text.len() as u64,
        keys,
    }
}

/// The shape of the credential at `path`, or `None` when it cannot be read.
fn credential_shape(path: &Path) -> Option<CredentialShape> {
    fs::read_to_string(path).ok().as_deref().map(shape_of_text)
}

/// The shape of the record preserved beside a seat credential (see
/// [`preserve_credential_shape`]), or `None` when no record exists.
fn preserved_shape(role_home: &Path) -> Option<CredentialShape> {
    let record: Value =
        serde_json::from_str(&fs::read_to_string(role_home.join(PRESERVED_SHAPE_FILE)).ok()?)
            .ok()?;
    Some(CredentialShape {
        len: record["len"].as_u64()?,
        keys: record["keys"]
            .as_array()?
            .iter()
            .filter_map(|k| k.as_str().map(str::to_string))
            .collect(),
    })
}

/// The diagnostic of one seat, resolved from its declared credential file's presence and
/// shape only.
fn seat_diagnostic(home: &Path, agent: &str, role: &str) -> AuthDiagnostic {
    let role_home = role_home_dir(home, agent, role);
    let credential = seat_credentials_file(&role_home, agent);
    let rel = credential
        .strip_prefix(home)
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| credential.display().to_string());
    let diagnostic = |state: &'static str, detail: String| AuthDiagnostic {
        agent: agent.to_string(),
        role: role.to_string(),
        state,
        detail,
    };
    match std::fs::symlink_metadata(&credential) {
        Err(_) => diagnostic(
            AUTH_STATE_LOGIN_REQUIRED,
            format!("{rel} is missing; run `hotpot auth login {agent} {role}`"),
        ),
        Ok(meta) if meta.file_type().is_symlink() => diagnostic(
            AUTH_STATE_LEGACY_SHARED,
            format!(
                "{rel} is a symlink (a shared mutable credential, not seat-owned); \
                 replace it via `hotpot auth login {agent} {role}`"
            ),
        ),
        Ok(meta) if !meta.is_file() => diagnostic(
            AUTH_STATE_LEGACY_SHARED,
            format!(
                "{rel} is not a regular file; a seat credential must be an ordinary \
                 seat-owned file"
            ),
        ),
        Ok(meta) if meta.nlink() > 1 => diagnostic(
            AUTH_STATE_LEGACY_SHARED,
            format!(
                "{rel} is a hardlink (nlink={}, one file shared by several paths); \
                 replace it via `hotpot auth login {agent} {role}`",
                meta.nlink()
            ),
        ),
        Ok(_) => {
            // One read feeds both the divergence comparison and the structural checks.
            let text = fs::read_to_string(&credential).ok();
            // A preserved shape record exists only after a successful `hotpot auth login`;
            // a live credential that no longer matches it has been tampered with or replaced.
            if let (Some(preserved), Some(live)) = (
                preserved_shape(&role_home),
                text.as_deref().map(shape_of_text),
            ) && live != preserved
            {
                return diagnostic(
                    AUTH_STATE_DIVERGED,
                    format!(
                        "{rel} no longer matches the shape preserved in {PRESERVED_SHAPE_FILE} \
                         at login time"
                    ),
                );
            }
            let parsed = text
                .as_deref()
                .and_then(|text| serde_json::from_str::<Value>(text).ok());
            if parsed
                .as_ref()
                .is_some_and(|value| value["storage"].as_str() == Some("keyring"))
            {
                return diagnostic(
                    AUTH_STATE_KEYRING,
                    format!(
                        "{rel} declares native keyring storage; an offline verify cannot \
                         confirm the OS keyring"
                    ),
                );
            }
            if parsed.is_some_and(|value| {
                value.as_object().is_some_and(|map| !map.is_empty())
            }) {
                return diagnostic(
                    AUTH_STATE_CONFIGURED,
                    format!("{rel} is a structurally complete file-backed credential"),
                );
            }
            diagnostic(
                AUTH_STATE_LOGIN_REQUIRED,
                format!(
                    "{rel} is not a structurally complete file-backed credential; \
                     run `hotpot auth login {agent} {role}`"
                ),
            )
        }
    }
}
