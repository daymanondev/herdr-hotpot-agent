//! Operator credential seeding into Role Homes.
//!
//! A `Human` already authenticated in their personal `codex`, `claude`, and `pi`
//! installations should not have to re-run `hotpot auth login` for every `Seat`: whenever
//! a Role Home is provisioned (`hotpot setup` planning/apply) or a `Seat` is launched
//! (`hotpot seat launch`), an existing operator credential is seeded into the Role Home as
//! an independent file copy so the `Seat` authenticates immediately.
//!
//! The safety rules (spec: "Credential Seeding & Environment Scoping", first bullet):
//!
//! - the copy is always an ordinary file written atomically — never a symlink or hardlink
//!   into the operator's homes (`legacy-shared-risk` stays detectable);
//! - a Role Home that already owns a credential (e.g. created by `hotpot auth login`) is
//!   never overwritten, so intentional per-seat account overrides stay durable;
//! - the operator's home is a strictly read-only input — nothing under `~/.codex`,
//!   `~/.claude`, or `~/.pi/agent` is ever written;
//! - only file-backed credentials are seeded: a claude `.credentials.json` declaring
//!   native keyring storage (`"storage": "keyring"`) is skipped, since its secret lives in
//!   the OS keyring, not in the file;
//! - every seeded copy carries the structural shape sidecar
//!   ([`auth::PRESERVED_SHAPE_FILE`]) beside it, so `hotpot verify` reports the seat
//!   `configured structurally` and a later divergence stays detectable. The sidecar stores
//!   shape only — no token value is ever read into any output.

use crate::auth::{PRESERVED_SHAPE_FILE, preserved_shape_contents};
use crate::provisioner::{PlanError, PlanItem, atomic_write_file};
use crate::role_homes::{role_home_dir, seat_credentials_file};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

/// The operator's file-backed credential source for `agent` under the operator's home.
fn operator_credential_path(operator_home: &Path, agent: &str) -> PathBuf {
    match agent {
        "codex" => operator_home.join(".codex").join("auth.json"),
        "claude" => operator_home.join(".claude").join(".credentials.json"),
        "pi" => operator_home.join(".pi").join("agent").join("auth.json"),
        other => unreachable!("operator credential source asked for unknown agent {other}"),
    }
}

/// The operator's credential contents for `agent` when a seedable file-backed credential
/// exists: present and readable as text, and — for claude, whose credentials can declare
/// native OS-keyring storage (the same `"storage": "keyring"` marker `hotpot verify`
/// reads) — not a keyring marker file. Absent, unreadable, and keyring-declaring sources
/// all yield `None`: there is nothing file-backed to copy.
fn operator_credential_contents(operator_home: &Path, agent: &str) -> Option<String> {
    let contents = fs::read_to_string(operator_credential_path(operator_home, agent)).ok()?;
    if agent == "claude"
        && serde_json::from_str::<Value>(&contents)
            .ok()
            .as_ref()
            .is_some_and(|value| value["storage"].as_str() == Some("keyring"))
    {
        return None;
    }
    Some(contents)
}

/// The seeding plan items for one seat: an atomically written independent copy of the
/// operator's credential plus its shape sidecar — but only when the operator owns a
/// file-backed credential and the Role Home does not already have that credential file on
/// disk (anything already occupying the path, including a symlink, means the seat owns its
/// credential and is left untouched). Operator homes are only ever read.
pub fn seed_plan_items(home: &Path, operator_home: &Path, agent: &str, role: &str) -> Vec<PlanItem> {
    let role_home = role_home_dir(home, agent, role);
    let credential = seat_credentials_file(&role_home, agent);
    if fs::symlink_metadata(&credential).is_ok() {
        return Vec::new();
    }
    let Some(contents) = operator_credential_contents(operator_home, agent) else {
        return Vec::new();
    };
    let sidecar = preserved_shape_contents(&contents);
    vec![
        PlanItem::File {
            path: credential,
            contents,
        },
        PlanItem::File {
            path: role_home.join(PRESERVED_SHAPE_FILE),
            contents: sidecar,
        },
    ]
}

/// On-demand seeding for `hotpot seat launch`: applies exactly the items
/// [`seed_plan_items`] would plan (the same never-overwrite, never-symlink rules) straight
/// to disk, so a provisioned-but-unseeded `Seat` inherits the operator's credential before
/// its vendor terminal starts. Returns whether a credential was seeded.
pub fn seed_seat_credential(
    home: &Path,
    operator_home: &Path,
    agent: &str,
    role: &str,
) -> Result<bool, PlanError> {
    let items = seed_plan_items(home, operator_home, agent, role);
    let seeded = !items.is_empty();
    for item in items {
        let PlanItem::File { path, contents } = item else {
            continue;
        };
        atomic_write_file(&path, &contents)?;
    }
    Ok(seeded)
}
