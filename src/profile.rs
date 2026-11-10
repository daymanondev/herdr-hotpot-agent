//! Minimal `hotpot profile` write path (ticket 13): the persistence behind the
//! right-hand panel's inline `agent/model · thinking` pills and named preset
//! profiles.
//!
//! `hotpot profile set <agent> <role> <model> --thinking <effort>` atomically
//! updates the role profile defaults in Hotpot Home (`<HOTPOT_HOME>/
//! profiles.json` — the file `hotpot status` reads back through
//! [`crate::status`]), reusing the provisioner's atomic write engine (ticket
//! 01: temporary sibling + rename, so a polling panel never observes a
//! half-written file). Writes merge per role: other roles and unknown fields
//! survive unchanged. Validation fails closed — an unknown agent family or
//! role, or a refused thinking effort (`ultra`/`ultracode`), writes nothing.

use crate::provisioner::{PlanError, atomic_write_file};
use crate::runtime::{AGENT_FAMILIES, REFUSED_THINKING_EFFORTS, THINKING_EFFORTS};
use serde_json::{Value, json};
use std::path::Path;

/// The file this write path owns, relative to Hotpot Home.
pub const PROFILES_FILE: &str = "profiles.json";

/// The seat roles a profile default may address (the `profiles.json` keys the
/// status projection reads).
pub const PROFILE_ROLES: &[&str] = &["supervisor", "lead", "peer"];

/// One built-in role default: the fallback `agent` / `model` / `thinking`
/// every unset `profiles.json` field resolves to, so the always-present Seat
/// Slots of the Herdr panel render a concrete profile even on a fresh Project.
#[derive(Debug, Clone, Copy)]
pub struct BuiltinProfile {
    pub role: &'static str,
    pub agent: &'static str,
    pub model: &'static str,
    pub thinking: &'static str,
}

/// The single source of the built-in role defaults (ticket nui/01): one
/// constant both the Rust resolver and the JS fixtures name, mirroring the
/// panel's *Deep Architecture* room preset (`shared/protocol.js:
/// PRESET_PROFILES`) as the out-of-the-box posture.
pub const BUILTIN_ROLE_PROFILES: &[BuiltinProfile] = &[
    BuiltinProfile {
        role: "supervisor",
        agent: "claude",
        model: "claude-opus-4-6",
        thinking: "high",
    },
    BuiltinProfile {
        role: "lead",
        agent: "codex",
        model: "gpt-5.1-codex",
        thinking: "high",
    },
    BuiltinProfile {
        role: "peer",
        agent: "codex",
        model: "gpt-5.1-codex",
        thinking: "medium",
    },
];

/// Validates a `profile set` payload and merges it into
/// `<home>/profiles.json`, writing the result atomically. The merge preserves
/// every other role entry and every unknown field of the addressed role; an
/// existing non-object `profiles.json` (or role entry) is a shape conflict and
/// fails closed without writing.
pub fn set(
    home: &Path,
    agent: &str,
    role: &str,
    model: &str,
    thinking: &str,
) -> Result<(), PlanError> {
    if !AGENT_FAMILIES.contains(&agent) {
        return Err(PlanError::new(
            "invalid-agent",
            format!("unknown agent family `{agent}`: permitted families are codex|claude|pi"),
        ));
    }
    if !PROFILE_ROLES.contains(&role) {
        return Err(PlanError::new(
            "invalid-role",
            format!("unknown role `{role}`: permitted roles are supervisor|lead|peer"),
        ));
    }
    if REFUSED_THINKING_EFFORTS.contains(&thinking) {
        return Err(PlanError::new(
            "refused-thinking",
            format!(
                "thinking effort `{thinking}` refused: ultra/ultracode are not permitted \
                 efforts; use low|medium|high"
            ),
        ));
    }
    if !THINKING_EFFORTS.contains(&thinking) {
        return Err(PlanError::new(
            "invalid-thinking",
            format!("invalid thinking effort `{thinking}`: permitted efforts are low|medium|high"),
        ));
    }
    if model.trim().is_empty() {
        return Err(PlanError::new(
            "invalid-model",
            "a non-empty model name is required",
        ));
    }

    let path = home.join(PROFILES_FILE);
    let mut profiles: Value = match std::fs::read_to_string(&path) {
        Ok(raw) if raw.trim().is_empty() => json!({}),
        Ok(raw) => serde_json::from_str(&raw).map_err(|e| {
            PlanError::new(
                "profiles-unreadable",
                format!("{} exists but is not valid JSON: {e}", path.display()),
            )
        })?,
        Err(_) => json!({}),
    };
    let Some(map) = profiles.as_object_mut() else {
        return Err(PlanError::new(
            "path-shape-conflict",
            format!(
                "{} is not a JSON object of role entries; refusing to overwrite",
                path.display()
            ),
        ));
    };
    let entry = map
        .entry(role.to_string())
        .or_insert_with(|| json!({}));
    let Some(entry) = entry.as_object_mut() else {
        return Err(PlanError::new(
            "path-shape-conflict",
            format!(
                "profiles.json role entry `{role}` is not an object; refusing to overwrite"
            ),
        ));
    };
    // The addressed role's payload lands additively: unknown fields survive.
    entry.insert("agent".to_string(), json!(agent));
    entry.insert("model".to_string(), json!(model));
    entry.insert("thinking".to_string(), json!(thinking));

    let rendered = serde_json::to_string_pretty(&profiles)
        .map_err(|e| PlanError::new("io", format!("failed to render profiles.json: {e}")))?;
    atomic_write_file(&path, &format!("{rendered}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_rejects_every_disallowed_payload_before_touching_disk() {
        let dir = crate::provisioner::normalize(std::env::temp_dir().as_path());
        let home = dir.join("hotpot-profile-never-written");
        for (agent, role, model, thinking) in [
            ("zcode", "lead", "m", "low"),
            ("claude", "owner", "m", "low"),
            ("claude", "lead", "m", "ultra"),
            ("claude", "lead", "m", "ultracode"),
            ("claude", "lead", "m", "extreme"),
            ("claude", "lead", "  ", "low"),
        ] {
            let err = set(&home, agent, role, model, thinking)
                .expect_err("invalid payload must fail closed");
            assert!(!home.join(PROFILES_FILE).exists(), "nothing written for {err}");
        }
    }

    #[test]
    fn builtin_role_profiles_cover_every_role_with_permitted_values() {
        let roles: Vec<&str> = BUILTIN_ROLE_PROFILES.iter().map(|p| p.role).collect();
        assert_eq!(roles, Vec::from(PROFILE_ROLES), "exactly the profile roles");
        for profile in BUILTIN_ROLE_PROFILES {
            assert!(
                AGENT_FAMILIES.contains(&profile.agent),
                "{} agent `{}` is a verified family",
                profile.role,
                profile.agent
            );
            assert!(
                THINKING_EFFORTS.contains(&profile.thinking),
                "{} thinking `{}` is a permitted effort",
                profile.role,
                profile.thinking
            );
            assert!(
                !profile.model.trim().is_empty(),
                "{} model is a concrete name",
                profile.role
            );
        }
    }
}
