//! SLP State Machine: 4-Part Contract validation, Ledger projection, and Role-scoped MCP tools.

use crate::guard::Role;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const REQUIRED_CONTRACT_SECTIONS: &[&str] = &[
    "1. Bounded Outcome",
    "2. Contract Boundary & Regression Invariants",
    "3. Excluded / Deferred",
    "4. Verification",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeatStatus {
    pub id: String,
    pub role: String,
    pub title: String,
    pub provider: String,
    pub model: String,
    pub status: String,
    pub terminal: Option<String>,
    pub context_tokens: u64,
    pub activity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskStatus {
    pub id: String,
    pub disposition: String,
    pub status: String,
    pub verdict: Option<String>,
    pub attempt: u32,
    pub summary: String,
    pub provider: String,
    pub model: String,
    pub agent_id: String,
    pub terminal: Option<String>,
    pub context_tokens: u64,
    pub activity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MailboxEntry {
    pub from: String,
    pub to: String,
    pub kind: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HotpotStatusSnapshot {
    pub project: String,
    pub workspace: String,
    pub mode: String,
    pub seats: Vec<SeatStatus>,
    pub tasks: Vec<TaskStatus>,
    pub mailbox: Vec<MailboxEntry>,
}

/// Mechanically validates that a Lead-authored SLP Contract description contains all 4 canonical sections.
pub fn validate_contract_description(description: &str) -> Result<(), String> {
    if description.chars().count() > 2500 {
        return Err("Contract description exceeds 2,500-character ceiling".to_string());
    }
    for section in REQUIRED_CONTRACT_SECTIONS {
        if !description.contains(section) {
            return Err(format!(
                "invalid-contract: missing required section `{section}`"
            ));
        }
    }
    Ok(())
}

/// Returns the strictly role-scoped MCP tool definitions for `role`.
pub fn role_scoped_mcp_tools(role: Role) -> Vec<Value> {
    match role {
        Role::Supervisor => vec![
            json!({
                "name": "supervisor_dispatch_lead",
                "description": "Dispatch a clarified Owner directive to the Project's Lead seat."
            }),
            json!({
                "name": "supervisor_accept_task",
                "description": "Accept a green Task Attempt on the Owner's explicit word."
            }),
            json!({
                "name": "hotpot_status",
                "description": "Read the current Project's SLP status snapshot."
            }),
        ],
        Role::Lead => vec![
            json!({
                "name": "lead_set_contract",
                "description": "Set a 4-Part Contract and mechanical Gate for a Task."
            }),
            json!({
                "name": "lead_spawn_agent",
                "description": "Spawn a fenced Peer in an isolated Task worktree via herdr-core (hc)."
            }),
            json!({
                "name": "lead_accept_task",
                "description": "Accept a green Attempt after inspecting git diff and land the Task worktree."
            }),
            json!({
                "name": "lead_repoke_agent",
                "description": "Send a reconciliation message to a live Peer session."
            }),
            json!({
                "name": "lead_close_task",
                "description": "Retire an unaccepted or superseded Task and clean up its worktree."
            }),
            json!({
                "name": "lead_report_to_supervisor",
                "description": "Report wave completion or escalations back to Supervisor."
            }),
            json!({
                "name": "hotpot_status",
                "description": "Read the current Project's SLP status snapshot."
            }),
        ],
        Role::Agent => vec![
            json!({
                "name": "agent_claim",
                "description": "Submit a 6-Field Handoff and commit SHA to trigger the mechanical Gate."
            }),
            json!({
                "name": "agent_escalate_blocker",
                "description": "Escalate a REOPEN_REQUEST, DEPENDENCY_REQUEST, or BLOCKED premise challenge to Lead."
            }),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_validation_enforces_four_canonical_sections() {
        let bad = "Just fix the login bug and run tests.";
        assert!(validate_contract_description(bad).is_err());

        let good = "\
1. Bounded Outcome\nImplement token parser.\n\
2. Contract Boundary & Regression Invariants\nKeep existing CLI intact.\n\
3. Excluded / Deferred\nUI changes deferred.\n\
4. Verification\ncargo test";
        assert!(validate_contract_description(good).is_ok());
    }

    #[test]
    fn peer_mcp_slice_never_exposes_lead_or_supervisor_tools() {
        let peer_tools = role_scoped_mcp_tools(Role::Agent);
        let names: Vec<&str> = peer_tools
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, vec!["agent_claim", "agent_escalate_blocker"]);
    }
}
