//! 3-Layer Choke, Seat Write Fence, Skill/MCP Isolation, and Clean-Room Provider Home Provisioning.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Supervisor,
    Lead,
    Agent,
}

impl Role {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "supervisor" => Some(Self::Supervisor),
            "lead" => Some(Self::Lead),
            "agent" | "peer" => Some(Self::Agent),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Supervisor => "supervisor",
            Self::Lead => "lead",
            Self::Agent => "agent",
        }
    }
}

/// Core design skills permitted to the `Lead` seat (Peer-Owned Domain Skills Law).
pub const LEAD_ALLOWED_SKILLS: &[&str] = &["codebase-design", "domain-modeling", "tdd"];

/// Owner-only bootstrap skills that neither `Lead` nor `Peer` may invoke or read.
pub const OWNER_ONLY_SKILLS: &[&str] = &["factory", "hotpot"];

/// Memory files a `Supervisor` or `Lead` seat is permitted to edit at `<project-root>`.
pub const SEAT_MEMORY_WHITELIST: &[&str] = &[
    "AGENTS.md",
    "CLAUDE.md",
    "CONTEXT.md",
    "GLOSSARY.md",
    ".gitignore",
    ".worktreeinclude",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChokeVerdict {
    pub allowed: bool,
    pub code: &'static str,
    pub reason: String,
}

impl ChokeVerdict {
    pub fn allow() -> Self {
        Self {
            allowed: true,
            code: "allowed",
            reason: String::new(),
        }
    }

    pub fn deny(code: &'static str, reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            code,
            reason: reason.into(),
        }
    }
}

/// Extracts `<skill-name>` if `path` points inside a `.../skills/<skill-name>/SKILL.md` tree.
pub fn extract_skill_name_from_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let parts: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
    for window in parts.windows(3) {
        if window[0] == "skills" && window[2].eq_ignore_ascii_case("SKILL.md") {
            return Some(window[1].to_string());
        }
    }
    None
}

/// Checks whether `role` is allowed to load or read `skill_name`.
pub fn check_skill_access(
    role: Role,
    skill_name: &str,
    contract_skills: Option<&[String]>,
) -> ChokeVerdict {
    let skill = skill_name.trim();
    if role != Role::Supervisor && OWNER_ONLY_SKILLS.contains(&skill) {
        return ChokeVerdict::deny(
            "skill-fence",
            format!(
                "Role `{}` is forbidden from loading Owner-only skill `{skill}`",
                role.as_str()
            ),
        );
    }

    match role {
        Role::Supervisor => ChokeVerdict::allow(),
        Role::Lead => {
            if LEAD_ALLOWED_SKILLS.contains(&skill) {
                ChokeVerdict::allow()
            } else {
                ChokeVerdict::deny(
                    "lead-skill-fence",
                    format!(
                        "Role `lead` may only load core design skills ({}); implementation skill `{skill}` is Peer-owned",
                        LEAD_ALLOWED_SKILLS.join(", ")
                    ),
                )
            }
        }
        Role::Agent => {
            if let Some(allowed) = contract_skills
                && !allowed.is_empty()
                && !allowed.iter().any(|s| s == skill)
            {
                return ChokeVerdict::deny(
                    "peer-contract-skill-fence",
                    format!("Skill `{skill}` is not in this Task Contract's granted skills"),
                );
            }
            ChokeVerdict::allow()
        }
    }
}

/// Evaluates whether a file write path is permitted for `role`.
pub fn evaluate_write_path(
    role: Role,
    target_path: &str,
    workspace_root: &str,
    state_dir: &str,
) -> ChokeVerdict {
    let norm_target = target_path.replace('\\', "/");
    let norm_state = state_dir.trim_end_matches('/').replace('\\', "/");
    if !norm_state.is_empty() && norm_target.starts_with(&norm_state) {
        return ChokeVerdict::deny(
            "state-dir-protected",
            "Direct writes to Hotpot state directory are forbidden",
        );
    }

    let norm_ws = workspace_root.trim_end_matches('/').replace('\\', "/");
    let rel = if !norm_ws.is_empty() && norm_target.starts_with(&format!("{norm_ws}/")) {
        &norm_target[norm_ws.len() + 1..]
    } else {
        norm_target.as_str()
    };

    match role {
        Role::Supervisor | Role::Lead => {
            if SEAT_MEMORY_WHITELIST.contains(&rel)
                || rel.starts_with("docs/")
                || rel.starts_with(".scratch/")
            {
                ChokeVerdict::allow()
            } else {
                ChokeVerdict::deny(
                    "seat-write-fence",
                    format!(
                        "Seat `{}` cannot modify product file `{rel}`; dispatch a Task to a Peer instead",
                        role.as_str()
                    ),
                )
            }
        }
        Role::Agent => {
            if !norm_ws.is_empty()
                && norm_target.starts_with('/')
                && !norm_target.starts_with(&format!("{norm_ws}/"))
                && !norm_target.starts_with("/tmp/")
            {
                return ChokeVerdict::deny(
                    "worktree-escape",
                    format!(
                        "Peer write `{norm_target}` escapes Task worktree `{norm_ws}`"
                    ),
                );
            }
            if SEAT_MEMORY_WHITELIST[..4].contains(&rel) {
                return ChokeVerdict::deny(
                    "root-memory-protected",
                    format!(
                        "Peer cannot modify root memory file `{rel}` without explicit Contract opt-in"
                    ),
                );
            }
            ChokeVerdict::allow()
        }
    }
}

/// Evaluates a shell command against Layer-2 Choke rules.
pub fn evaluate_bash_command(
    role: Role,
    command: &str,
    contract_skills: Option<&[String]>,
) -> ChokeVerdict {
    let trimmed = command.trim();

    // 1. Block direct `hc` or `herdr-app` orchestration calls by seats and Peers
    for token in trimmed.split([';', '|', '&', '\n']) {
        let first = token.split_whitespace().next().unwrap_or("");
        let bin = first.rsplit('/').next().unwrap_or(first);
        if bin == "hc" || bin == "herdr-app" || bin == "herdr" {
            return ChokeVerdict::deny(
                "seat-hc-tool-fence",
                format!(
                    "Direct `{bin}` CLI orchestration is forbidden for role `{}`; use role-scoped Hotpot MCP tools",
                    role.as_str()
                ),
            );
        }
    }

    // 2. Check if bash command reads a `.../skills/<name>/SKILL.md` file
    for word in trimmed.split_whitespace() {
        let clean = word.trim_matches(|c| c == '"' || c == '\'');
        if let Some(skill) = extract_skill_name_from_path(clean) {
            let verdict = check_skill_access(role, &skill, contract_skills);
            if !verdict.allowed {
                return verdict;
            }
        }
    }

    // 3. Block destructive git commands (S1)
    if trimmed.contains("git reset --hard")
        || trimmed.contains("git push --force")
        || trimmed.contains("git clean -fd")
    {
        return ChokeVerdict::deny(
            "destructive-git-s1",
            "Destructive git command blocked by S1 Choke policy",
        );
    }

    ChokeVerdict::allow()
}

/// Evaluates a `PreToolUse` JSON payload from Claude Code / Codex / Antigravity / OpenCode.
pub fn evaluate_pre_tool_hook(
    role: Role,
    payload: &Value,
    workspace_root: &str,
    state_dir: &str,
    contract_skills: Option<&[String]>,
) -> Value {
    let tool_name = payload
        .get("tool_name")
        .or_else(|| payload.get("tool"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let tool_input = payload
        .get("tool_input")
        .or_else(|| payload.get("input"))
        .unwrap_or(&Value::Null);

    let verdict = match tool_name {
        "Skill" | "skill" => {
            let skill = tool_input
                .get("skill")
                .or_else(|| tool_input.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            check_skill_access(role, skill, contract_skills)
        }
        "Read" | "read" | "read_file" => {
            let path = tool_input
                .get("file_path")
                .or_else(|| tool_input.get("path"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if let Some(skill) = extract_skill_name_from_path(path) {
                check_skill_access(role, &skill, contract_skills)
            } else {
                ChokeVerdict::allow()
            }
        }
        "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "write_file" | "edit_file" => {
            let path = tool_input
                .get("file_path")
                .or_else(|| tool_input.get("path"))
                .and_then(Value::as_str)
                .unwrap_or("");
            evaluate_write_path(role, path, workspace_root, state_dir)
        }
        "Bash" | "bash" | "run_command" => {
            let cmd = tool_input
                .get("command")
                .or_else(|| tool_input.get("CommandLine"))
                .and_then(Value::as_str)
                .unwrap_or("");
            evaluate_bash_command(role, cmd, contract_skills)
        }
        _ => ChokeVerdict::allow(),
    };

    if verdict.allowed {
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow"
            }
        })
    } else {
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": format!("[{}] {}", verdict.code, verdict.reason)
            }
        })
    }
}

/// Materializes an isolated Clean-Room Provider Home under `<stateDir>/homes/<provider>/<role>-<corr>/`
/// with strictly role-scoped MCP (`hotpot mcp`) and role-scoped Skill allowlists.
pub fn materialize_clean_room_home(
    state_dir: &Path,
    provider: &str,
    role: Role,
    correlation_id: &str,
    hotpot_bin: &Path,
    allowed_skills: &[String],
) -> Result<PathBuf, String> {
    let dir = state_dir
        .join("homes")
        .join(provider)
        .join(format!("{}-{correlation_id}", role.as_str()));
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create clean-room dir: {e}"))?;

    // 1. Role-scoped MCP configuration (`mcp.json`)
    let mcp_json = json!({
        "mcpServers": {
            "hotpot": {
                "type": "stdio",
                "command": hotpot_bin.to_string_lossy(),
                "args": [
                    "mcp",
                    "--role",
                    role.as_str(),
                    "--correlation",
                    correlation_id,
                    "--state-dir",
                    state_dir.to_string_lossy()
                ]
            }
        }
    });
    fs::write(
        dir.join("mcp.json"),
        serde_json::to_string_pretty(&mcp_json).unwrap() + "\n",
    )
    .map_err(|e| format!("failed to write mcp.json: {e}"))?;

    // 2. Provider-native PreToolUse hook & Skill allowlist settings (`settings.json`)
    let hook_cmd = format!(
        "{} hook --role {} --correlation {} --state-dir {}",
        hotpot_bin.to_string_lossy(),
        role.as_str(),
        correlation_id,
        state_dir.to_string_lossy()
    );
    let settings_json = json!({
        "role": role.as_str(),
        "allowedSkills": allowed_skills,
        "hooks": {
            "PreToolUse": [
                {
                    "matcher": "*",
                    "hooks": [
                        {
                            "type": "command",
                            "command": hook_cmd
                        }
                    ]
                }
            ]
        }
    });
    fs::write(
        dir.join("settings.json"),
        serde_json::to_string_pretty(&settings_json).unwrap() + "\n",
    )
    .map_err(|e| format!("failed to write settings.json: {e}"))?;

    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lead_is_blocked_from_loading_implementation_skills() {
        let ok = check_skill_access(Role::Lead, "codebase-design", None);
        assert!(ok.allowed);

        let blocked = check_skill_access(Role::Lead, "expo-router", None);
        assert!(!blocked.allowed);
        assert_eq!(blocked.code, "lead-skill-fence");
    }

    #[test]
    fn peer_is_blocked_from_owner_factory_skill_and_ungranted_skills() {
        let factory_blocked = check_skill_access(Role::Agent, "factory", None);
        assert!(!factory_blocked.allowed);
        assert_eq!(factory_blocked.code, "skill-fence");

        let granted = vec!["tdd".to_string(), "expo-ui".to_string()];
        assert!(check_skill_access(Role::Agent, "expo-ui", Some(&granted)).allowed);
        assert!(!check_skill_access(Role::Agent, "firebase-auth", Some(&granted)).allowed);
    }

    #[test]
    fn pre_tool_hook_blocks_lead_reading_implementation_skill_via_read_or_bash() {
        let read_payload = json!({
            "tool_name": "Read",
            "tool_input": {
                "file_path": "/Users/dev/.agents/skills/expo-router/SKILL.md"
            }
        });
        let out = evaluate_pre_tool_hook(Role::Lead, &read_payload, "/repo", "/state", None);
        assert_eq!(
            out["hookSpecificOutput"]["permissionDecision"],
            Value::String("deny".to_string())
        );

        let bash_payload = json!({
            "tool_name": "Bash",
            "tool_input": {
                "command": "cat ~/.agents/skills/expo-router/SKILL.md"
            }
        });
        let bash_out = evaluate_pre_tool_hook(Role::Lead, &bash_payload, "/repo", "/state", None);
        assert_eq!(
            bash_out["hookSpecificOutput"]["permissionDecision"],
            Value::String("deny".to_string())
        );
    }

    #[test]
    fn seat_write_fence_blocks_source_edits_but_allows_docs_and_glossary() {
        let deny_src = evaluate_write_path(
            Role::Lead,
            "/repo/src/main.rs",
            "/repo",
            "/state",
        );
        assert!(!deny_src.allowed);
        assert_eq!(deny_src.code, "seat-write-fence");

        let allow_glossary = evaluate_write_path(
            Role::Lead,
            "/repo/GLOSSARY.md",
            "/repo",
            "/state",
        );
        assert!(allow_glossary.allowed);
    }

    #[test]
    fn direct_hc_and_herdr_cli_calls_are_fenced() {
        let v = evaluate_bash_command(Role::Lead, "hc run --bg claude", None);
        assert!(!v.allowed);
        assert_eq!(v.code, "seat-hc-tool-fence");
    }
}
