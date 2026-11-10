//! Runtime Coordinator Layer: the role-asymmetric `hotpot mcp` stdio JSON-RPC 2.0
//! server backed by an append-only event log under `<HOTPOT_HOME>/runtime/v1/`.
//!
//! This slice delivers the exact per-role tool surface, the Lead `Assignment`
//! lifecycle (`Draft -> Dispatched -> Asking | HandedBack | Stopped`; terminal
//! states -> `Closed`), verified `baseCommit`, the `thinking` effort refusal, and
//! Writer Leases for writable assignments: the exclusive in-checkout lease after
//! clean-HEAD verification, plus up to three scope-disjoint isolated worktrees
//! provisioned with a warm build cache (see [`leases`]). The Peer surface is
//! strictly `ask`/`handoff` with identity, assignment, and workspace derived
//! server-side from the session correlation (never from tool arguments), and
//! every writable `handoff` validated against live Git `baseCommit..Candidate`
//! evidence (see [`evidence`]). Detached gate execution and supervisor tool
//! semantics land with their own runtime slices.

pub mod evidence;
pub mod ledger;
pub mod leases;

pub use ledger::{Ledger, project_slug, sha1_hex};
pub use leases::{IN_CHECKOUT_EXCLUSIVE, WriterLease};

use crate::guard::Role;
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use std::path::Path;

/// Canonical `Role` name on the MCP surface (`guard::Role::Agent` is a `Peer` here).
pub fn mcp_role_name(role: Role) -> &'static str {
    match role {
        Role::Supervisor => "supervisor",
        Role::Lead => "lead",
        Role::Agent => "peer",
    }
}

/// Spec: "Implementation Decisions -> Role-Asymmetric MCP Surface (`hotpot mcp`)".
pub const SUPERVISOR_TOOLS: &[&str] = &[
    "room_status",
    "runtime_findings",
    "message_lead",
    "attention_feedback",
    "lead_replace_prepare",
    "lead_replace_confirm",
    "lead_replace_cancel",
];

pub const LEAD_TOOLS: &[&str] = &[
    "assignment_create",
    "assignment_dispatch",
    "assignment_status",
    "assignment_answer",
    "assignment_accept",
    "assignment_close",
    "gate_run",
    "workspace_close",
    "lease_reclaim",
];

pub const PEER_TOOLS: &[&str] = &["ask", "handoff"];

/// The exact tool-name set a role may see in `tools/list` (and call).
pub fn role_tool_names(role: Role) -> &'static [&'static str] {
    match role {
        Role::Supervisor => SUPERVISOR_TOOLS,
        Role::Lead => LEAD_TOOLS,
        Role::Agent => PEER_TOOLS,
    }
}

/// Tool definitions (name + description) served by `tools/list` for `role`.
pub fn role_tool_defs(role: Role) -> Vec<Value> {
    let descriptions: &[(&str, &str)] = match role {
        Role::Supervisor => &[
            (
                "room_status",
                "Read the live room snapshot for this Project (seats, assignments, attention).",
            ),
            (
                "runtime_findings",
                "Read verified runtime findings for this Project.",
            ),
            (
                "message_lead",
                "Route a Human directive to the Project's Lead.",
            ),
            (
                "attention_feedback",
                "Rate a noisy Attention Letter to suppress or re-raise it.",
            ),
            (
                "lead_replace_prepare",
                "Request a structured handoff from the Lead at a verified quiet point.",
            ),
            (
                "lead_replace_confirm",
                "Archive the old Lead and launch a successor seeded with the reviewed handoff.",
            ),
            (
                "lead_replace_cancel",
                "Abort an in-flight Lead replacement.",
            ),
        ],
        Role::Lead => &[
            (
                "assignment_create",
                "Create a Draft Assignment with outcome, Disposition, verified baseCommit, optional Write Scope / serialOnly globs, Gate command, and thinking effort.",
            ),
            (
                "assignment_dispatch",
                "Dispatch a Draft Assignment to a Peer, registering its correlation -> (project, assignment, workspace) binding.",
            ),
            (
                "assignment_status",
                "Read the current state and all recorded fields of an Assignment.",
            ),
            (
                "assignment_answer",
                "Answer a Peer question on an Asking Assignment, returning it to Dispatched.",
            ),
            (
                "assignment_accept",
                "Accept a handed-back Assignment after integrating its Candidate.",
            ),
            (
                "assignment_close",
                "Stop in-flight work or finalize a terminal Assignment as Closed.",
            ),
            (
                "gate_run",
                "Start the Assignment Gate against its Candidate (execution is delivered by the detached Gate runner).",
            ),
            (
                "workspace_close",
                "Close an Assignment workspace and release isolated worktree resources.",
            ),
            ("lease_reclaim", "Reclaim an expired Writer Lease."),
        ],
        Role::Agent => &[
            (
                "ask",
                "Raise a question or a structured Challenge Signal (REOPEN_REQUEST, DEPENDENCY_REQUEST, BLOCKED) to the Lead for the current Assignment; identity, assignment, and workspace are derived server-side from the session correlation.",
            ),
            (
                "handoff",
                "Submit finished work for the current Assignment, validated against live Git baseCommit..Candidate evidence; identity, assignment, and workspace are derived server-side from the session correlation.",
            ),
        ],
    };
    descriptions
        .iter()
        .map(|(name, description)| json!({"name": name, "description": description}))
        .collect()
}

/// A JSON-RPC 2.0 error returned by a tool call or the dispatcher itself.
pub struct RpcError {
    pub code: i32,
    pub message: String,
}

impl RpcError {
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
        }
    }

    pub fn not_implemented(message: impl Into<String>) -> Self {
        Self {
            code: -32000,
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
        }
    }
}

fn rpc_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn rpc_error_response(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Runs the stdio JSON-RPC 2.0 server loop: one response line per request line.
/// `correlation` is the server-side session identity pin a Peer seat derives its
/// assignment binding from (`--correlation` / `HOTPOT_CORRELATION` at launch).
pub fn serve_mcp(
    role: Role,
    project_root: &Path,
    state_dir: &Path,
    correlation: Option<&str>,
) -> Result<(), String> {
    let ledger = Ledger::open(state_dir, project_root).map_err(|e| e.to_string())?;
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let Ok(raw) = line else { break };
        if raw.trim().is_empty() {
            continue;
        }
        if let Some(reply) = handle_line(&ledger, role, project_root, correlation, &raw) {
            let _ = writeln!(stdout, "{reply}");
            let _ = stdout.flush();
        }
    }
    Ok(())
}

/// Handles one raw request line; returns `None` for JSON-RPC notifications (no `id`).
fn handle_line(
    ledger: &Ledger,
    role: Role,
    project_root: &Path,
    correlation: Option<&str>,
    raw: &str,
) -> Option<Value> {
    let req: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => {
            return Some(rpc_error_response(
                Value::Null,
                -32700,
                "parse error: request line is not valid JSON",
            ));
        }
    };
    // A request without an `id` key is a notification: never answer it.
    let id = req.get("id").cloned()?;
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

    match method {
        "initialize" => Some(rpc_response(
            id,
            json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "hotpot", "version": env!("CARGO_PKG_VERSION") }
            }),
        )),
        "tools/list" => Some(rpc_response(id, json!({ "tools": role_tool_defs(role) }))),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            if name.is_empty() {
                return Some(rpc_error_response(
                    id,
                    -32602,
                    "tools/call requires a tool name",
                ));
            }
            if !role_tool_names(role).contains(&name) {
                return Some(rpc_error_response(
                    id,
                    -32602,
                    &format!(
                        "unknown tool `{name}` for role `{}` (the MCP surface is role-asymmetric)",
                        mcp_role_name(role)
                    ),
                ));
            }
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match call_tool(ledger, project_root, correlation, name, &arguments) {
                Ok(result) => Some(rpc_response(id, result)),
                Err(e) => Some(rpc_error_response(id, e.code, &e.message)),
            }
        }
        "" => Some(rpc_error_response(
            id,
            -32600,
            "invalid request: missing method",
        )),
        other => Some(rpc_error_response(
            id,
            -32601,
            &format!("method not found: {other}"),
        )),
    }
}

/// Dispatches a role-fenced `tools/call` to its handler.
fn call_tool(
    ledger: &Ledger,
    project_root: &Path,
    correlation: Option<&str>,
    tool: &str,
    args: &Value,
) -> Result<Value, RpcError> {
    match tool {
        "assignment_create" => assignment_create(ledger, project_root, args),
        "assignment_dispatch" => assignment_dispatch(ledger, project_root, args),
        "assignment_status" => assignment_status(ledger, args),
        "assignment_answer" => assignment_answer(ledger, args),
        "assignment_accept" => assignment_accept(ledger, args),
        "assignment_close" => assignment_close(ledger, args),
        "gate_run" => gate_run(ledger, args),
        "workspace_close" => workspace_close(ledger, project_root, args),
        "lease_reclaim" => lease_reclaim(ledger, args),
        "ask" => peer_ask(ledger, correlation, args),
        "handoff" => peer_handoff(ledger, correlation, args),
        _ => Err(RpcError::not_implemented(format!(
            "`{tool}` semantics land with the assignment lifecycle slice"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Assignment projection (rebuilt from the ledger; no second database)
// ---------------------------------------------------------------------------

/// The `AssignmentState` names of the spec's state machine.
pub const ASSIGNMENT_STATES: &[&str] = &[
    "Draft",
    "Dispatched",
    "Asking",
    "HandedBack",
    "Stopped",
    "GateRunning",
    "Accepted",
    "ReworkRequested",
    "Rejected",
    "Abandoned",
    "Closed",
];

/// Permitted `Disposition` stances for a `Peer` brief.
pub const DISPOSITIONS: &[&str] = &["Engineer", "Architect", "Reviewer", "Scout"];

/// Permitted per-brief thinking efforts; `ultra`/`ultracode` are refused.
pub const THINKING_EFFORTS: &[&str] = &["low", "medium", "high"];
pub const REFUSED_THINKING_EFFORTS: &[&str] = &["ultra", "ultracode"];

/// One `Assignment` (`asg_<id>`) as projected from the ledger.
#[derive(Debug, Clone)]
pub struct Assignment {
    pub id: String,
    pub state: String,
    pub outcome: String,
    pub disposition: String,
    pub base_commit: String,
    pub write_scope: Vec<String>,
    pub serial_only: Vec<String>,
    pub gate: String,
    pub thinking: Option<String>,
    pub thinking_reason: Option<String>,
    pub correlation: Option<String>,
    pub workspace: Option<String>,
    /// The Lead's latest `assignment_answered` text, visible to the Peer seat.
    pub last_answer: Option<String>,
    /// The verified `Candidate` commit of the latest accepted handback.
    pub candidate: Option<String>,
    /// Unix-seconds timestamp of the handback that is still unread by the Lead
    /// (feeds the unread-handback Attention Letter).
    pub handed_back_at: Option<u64>,
}

impl Assignment {
    fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "state": self.state,
            "outcome": self.outcome,
            "disposition": self.disposition,
            "baseCommit": self.base_commit,
            "writeScope": self.write_scope,
            "serialOnly": self.serial_only,
            "gate": self.gate,
            "thinking": self.thinking,
            "thinkingReason": self.thinking_reason,
            "correlation": self.correlation,
            "workspace": self.workspace,
            "lastAnswer": self.last_answer,
            "candidate": self.candidate,
            "handedBackAt": self.handed_back_at,
        })
    }
}

/// All assignments and active Writer Leases of one project ledger, rebuilt by
/// replaying the event log.
#[derive(Debug, Default)]
pub struct RuntimeState {
    assignments: std::collections::BTreeMap<String, Assignment>,
    leases: std::collections::BTreeMap<String, WriterLease>,
}

impl RuntimeState {
    pub fn load(ledger: &Ledger) -> Self {
        Self::from_events(&ledger.read_events())
    }

    /// Rebuilds the projection from raw ledger events (used by the evidence
    /// recorder, which reads ledgers without a project root).
    pub fn from_events(events: &[Value]) -> Self {
        let mut state = Self::default();
        for event in events {
            state.apply(event);
        }
        state
    }

    pub fn assignment(&self, id: &str) -> Option<&Assignment> {
        self.assignments.get(id)
    }

    /// The active Writer Lease held for `assignment`, if any.
    pub fn lease_for(&self, assignment: &str) -> Option<&WriterLease> {
        self.leases.get(assignment)
    }

    /// Every currently held Writer Lease (release order = acquisition order).
    pub fn active_leases(&self) -> Vec<&WriterLease> {
        self.leases.values().collect()
    }

    fn apply(&mut self, event: &Value) {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        let id = event
            .get("assignment")
            .and_then(Value::as_str)
            .unwrap_or("");
        if id.is_empty() {
            return;
        }
        match kind {
            "assignment_created" => {
                self.assignments.insert(
                    id.to_string(),
                    Assignment {
                        id: id.to_string(),
                        state: "Draft".to_string(),
                        outcome: str_field(event, "outcome"),
                        disposition: str_field(event, "disposition"),
                        base_commit: str_field(event, "baseCommit"),
                        write_scope: str_array_field(event, "writeScope"),
                        serial_only: str_array_field(event, "serialOnly"),
                        gate: str_field(event, "gate"),
                        thinking: opt_str_field(event, "thinking"),
                        thinking_reason: opt_str_field(event, "thinkingReason"),
                        correlation: None,
                        workspace: None,
                        last_answer: None,
                        candidate: None,
                        handed_back_at: None,
                    },
                );
            }
            "writer_lease_acquired" => {
                self.leases.insert(
                    id.to_string(),
                    WriterLease {
                        assignment: id.to_string(),
                        mode: str_field(event, "mode"),
                        worktree: opt_str_field(event, "worktree"),
                        cargo_target_dir: opt_str_field(event, "cargoTargetDir"),
                        worktree_include: str_array_field(event, "worktreeInclude"),
                    },
                );
            }
            "writer_lease_released" | "writer_lease_reclaimed" => {
                self.leases.remove(id);
            }
            "assignment_dispatched" => {
                if let Some(a) = self.assignments.get_mut(id) {
                    a.correlation = opt_str_field(event, "correlation");
                    a.workspace = opt_str_field(event, "workspace");
                    if let Some(next) = transition(&a.state, kind) {
                        a.state = next.to_string();
                    }
                }
            }
            "assignment_answered" => {
                if let Some(a) = self.assignments.get_mut(id) {
                    a.last_answer = opt_str_field(event, "answer");
                    if let Some(next) = transition(&a.state, kind) {
                        a.state = next.to_string();
                    }
                }
            }
            "assignment_handed_back" => {
                if let Some(a) = self.assignments.get_mut(id) {
                    a.candidate = opt_str_field(event, "candidate");
                    a.handed_back_at = event.get("ts").and_then(Value::as_u64);
                    if let Some(next) = transition(&a.state, kind) {
                        a.state = next.to_string();
                    }
                }
            }
            _ => {
                if let Some(a) = self.assignments.get_mut(id)
                    && let Some(next) = transition(&a.state, kind)
                {
                    a.state = next.to_string();
                }
            }
        }
    }
}

/// The spec's `AssignmentState` machine:
/// `Draft -> Dispatched -> Asking | HandedBack | Stopped`;
/// `HandedBack -> GateRunning (optional) -> Accepted | ReworkRequested | Rejected | Abandoned`;
/// `Accepted | Rejected | Abandoned -> Closed`.
/// Returns the next state when `kind` is a legal transition from `state`.
fn transition(state: &str, kind: &str) -> Option<&'static str> {
    match (state, kind) {
        ("Draft", "assignment_dispatched") => Some("Dispatched"),
        ("Dispatched", "assignment_asked") => Some("Asking"),
        ("Asking", "assignment_answered") => Some("Dispatched"),
        ("Dispatched", "assignment_handed_back")
        | ("Asking", "assignment_handed_back")
        | ("ReworkRequested", "assignment_handed_back") => Some("HandedBack"),
        ("HandedBack", "gate_started") => Some("GateRunning"),
        ("HandedBack", "assignment_accepted") | ("GateRunning", "assignment_accepted") => {
            Some("Accepted")
        }
        ("HandedBack", "assignment_rework_requested")
        | ("GateRunning", "assignment_rework_requested") => Some("ReworkRequested"),
        ("HandedBack", "assignment_rejected") | ("GateRunning", "assignment_rejected") => {
            Some("Rejected")
        }
        ("Draft", "assignment_stopped")
        | ("Dispatched", "assignment_stopped")
        | ("Asking", "assignment_stopped") => Some("Stopped"),
        ("HandedBack", "assignment_abandoned") | ("GateRunning", "assignment_abandoned") => {
            Some("Abandoned")
        }
        ("Accepted", "assignment_closed")
        | ("Rejected", "assignment_closed")
        | ("Abandoned", "assignment_closed") => Some("Closed"),
        _ => None,
    }
}

fn str_field(event: &Value, key: &str) -> String {
    event
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn opt_str_field(event: &Value, key: &str) -> Option<String> {
    event
        .get(key)
        .and_then(Value::as_str)
        .map(|s| s.to_string())
}

fn str_array_field(event: &Value, key: &str) -> Vec<String> {
    event
        .get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Argument helpers
// ---------------------------------------------------------------------------

fn arg_str(args: &Value, tool: &str, key: &str) -> Result<String, RpcError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            RpcError::invalid_params(format!("{tool}: `{key}` is required and must be non-empty"))
        })
}

fn arg_opt_str_array(args: &Value, tool: &str, key: &str) -> Result<Option<Vec<String>>, RpcError> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => Ok(Some(
            items
                .iter()
                .map(|v| {
                    v.as_str().map(|s| s.to_string()).ok_or_else(|| {
                        RpcError::invalid_params(format!(
                            "{tool}: `{key}` must be an array of glob strings"
                        ))
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        )),
        Some(_) => Err(RpcError::invalid_params(format!(
            "{tool}: `{key}` must be an array of glob strings"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Tool handlers
// ---------------------------------------------------------------------------

/// Verifies `commit` resolves to an existing commit object in the project git repo.
fn verify_base_commit(project_root: &Path, commit: &str) -> Result<(), RpcError> {
    let spec = format!("{commit}^{{commit}}");
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(project_root)
        .args(["cat-file", "-e", &spec])
        .output()
        .map_err(|e| RpcError::internal(format!("failed to invoke git: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(RpcError::invalid_params(format!(
            "assignment_create: baseCommit `{commit}` does not exist in the project git repository ({})",
            project_root.display()
        )))
    }
}

fn assignment_create(
    ledger: &Ledger,
    project_root: &Path,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_create";

    let outcome = arg_str(args, TOOL, "outcome")?;
    let disposition_raw = arg_str(args, TOOL, "disposition")?;
    let disposition = DISPOSITIONS
        .iter()
        .find(|d| d.eq_ignore_ascii_case(&disposition_raw))
        .ok_or_else(|| {
            RpcError::invalid_params(format!(
                "{TOOL}: invalid Disposition `{disposition_raw}`: permitted Dispositions are {}",
                DISPOSITIONS.join("|")
            ))
        })?;
    let base_commit = arg_str(args, TOOL, "baseCommit")?;
    verify_base_commit(project_root, &base_commit)?;
    let gate = arg_str(args, TOOL, "gate")?;
    let write_scope = arg_opt_str_array(args, TOOL, "writeScope")?.unwrap_or_default();
    let serial_only = arg_opt_str_array(args, TOOL, "serialOnly")?.unwrap_or_default();

    let thinking = match args.get("thinking") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let effort = s.trim();
            if REFUSED_THINKING_EFFORTS.contains(&effort) {
                return Err(RpcError::invalid_params(format!(
                    "{TOOL}: thinking effort `{effort}` refused: ultra/ultracode are not permitted brief efforts; use low|medium|high"
                )));
            }
            if THINKING_EFFORTS.contains(&effort) {
                Some(effort.to_string())
            } else {
                return Err(RpcError::invalid_params(format!(
                    "{TOOL}: invalid thinking effort `{effort}`: permitted efforts are low|medium|high"
                )));
            }
        }
        Some(_) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: `thinking` must be a string effort (low|medium|high)"
            )));
        }
    };
    let thinking_reason = match &thinking {
        Some(_) => Some(arg_str(args, TOOL, "thinkingReason")?),
        None => args
            .get("thinkingReason")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    };

    // Deterministic, replay-stable id: the Nth assignment created in this ledger.
    let ordinal = ledger
        .read_events()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("assignment_created"))
        .count()
        + 1;
    let id = format!("asg_{ordinal:03}");

    let mut event = json!({
        "type": "assignment_created",
        "assignment": id,
        "outcome": outcome,
        "disposition": disposition,
        "baseCommit": base_commit,
        "writeScope": write_scope,
        "serialOnly": serial_only,
        "gate": gate,
    });
    if let Some(t) = &thinking {
        event["thinking"] = json!(t);
    }
    if let Some(r) = &thinking_reason {
        event["thinkingReason"] = json!(r);
    }
    ledger
        .append(event)
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;

    let state = RuntimeState::load(ledger);
    Ok(state
        .assignment(&id)
        .expect("just-created assignment replays")
        .to_json())
}

fn assignment_status(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_status";
    let id = arg_str(args, TOOL, "assignment")?;
    RuntimeState::load(ledger)
        .assignment(&id)
        .map(|a| a.to_json())
        .ok_or_else(|| RpcError::invalid_params(format!("{TOOL}: unknown assignment `{id}`")))
}

/// Loads the current assignment projection or fails with a JSON-RPC error.
fn require_assignment(ledger: &Ledger, tool: &str, id: &str) -> Result<Assignment, RpcError> {
    RuntimeState::load(ledger)
        .assignment(id)
        .cloned()
        .ok_or_else(|| RpcError::invalid_params(format!("{tool}: unknown assignment `{id}`")))
}

/// Fails unless the assignment currently sits in one of `allowed` states.
fn require_state(
    tool: &str,
    action: &str,
    asg: &Assignment,
    allowed: &[&str],
) -> Result<(), RpcError> {
    if allowed.contains(&asg.state.as_str()) {
        Ok(())
    } else {
        Err(RpcError::invalid_params(format!(
            "{tool}: illegal transition: assignment `{}` is `{}`; {action} requires {}",
            asg.id,
            asg.state,
            allowed.join("|")
        )))
    }
}

/// Appends one lifecycle event and returns the reloaded assignment projection.
fn append_and_reload(ledger: &Ledger, tool: &str, event: Value) -> Result<Value, RpcError> {
    let id = event
        .get("assignment")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    ledger
        .append(event)
        .map_err(|e| RpcError::internal(format!("{tool}: {e}")))?;
    let state = RuntimeState::load(ledger);
    Ok(state
        .assignment(&id)
        .expect("assignment exists before its lifecycle event")
        .to_json())
}

fn assignment_dispatch(
    ledger: &Ledger,
    project_root: &Path,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_dispatch";
    let id = arg_str(args, TOOL, "assignment")?;
    let asg = require_assignment(ledger, TOOL, &id)?;
    require_state(TOOL, "dispatch", &asg, &["Draft"])?;

    // Writability is declared by a non-empty Write Scope: read-only assignments
    // (e.g. a Reviewer) acquire no Writer Lease and never touch the checkout.
    let writable = !asg.write_scope.is_empty();
    let isolation = leases::parse_isolation(args)?;
    let state = RuntimeState::load(ledger);
    let mut provision: Option<leases::WorktreeProvision> = None;
    if writable {
        match isolation {
            leases::Isolation::Checkout => {
                // The exclusive checkout lease blocks writes from all other
                // seats — including Lead itself — until it is released (spec
                // user story 20).
                if let Some(holder) = state.active_leases().first() {
                    return Err(RpcError::invalid_params(format!(
                        "{TOOL}: writer_exclusive: the checkout writer lease is held by `{}` (mode `{}`); writable dispatch is refused for all seats (including Lead) until the lease is released",
                        holder.assignment, holder.mode
                    )));
                }
                leases::verify_checkout_preconditions(project_root, &asg.base_commit)?;
            }
            leases::Isolation::Worktree => {
                // Isolated worktree writers: at most LEASE_CAP concurrent, with
                // pairwise-disjoint Write Scope and serialOnly globs against the
                // active worktree leases (spec user story 21).
                let peers: Vec<(String, Vec<String>, Vec<String>)> = state
                    .active_leases()
                    .iter()
                    .filter(|l| l.mode == leases::ISOLATED_WORKTREE)
                    .map(|l| {
                        let peer = state.assignment(&l.assignment);
                        (
                            l.assignment.clone(),
                            peer.map(|a| a.write_scope.clone()).unwrap_or_default(),
                            peer.map(|a| a.serial_only.clone()).unwrap_or_default(),
                        )
                    })
                    .collect();
                if peers.len() >= leases::LEASE_CAP {
                    let holders = peers
                        .iter()
                        .map(|(id, ..)| format!("`{id}`"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(RpcError::invalid_params(format!(
                        "{TOOL}: lease_cap: the isolated worktree lease cap of {} concurrent writers is reached (held by {holders}); release a lease before dispatching another isolated writer",
                        leases::LEASE_CAP
                    )));
                }
                leases::check_scope_disjointness(
                    TOOL,
                    &asg.write_scope,
                    &asg.serial_only,
                    &peers,
                )?;
                provision = Some(leases::provision_worktree(
                    project_root,
                    ledger.project_dir(),
                    &id,
                    &asg.base_commit,
                )?);
            }
        }
    }

    // The correlation -> (project, assignment, workspace) binding is what the peer
    // identity slice resolves server-side; peers never supply their own ids.
    let ordinal = ledger
        .read_events()
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("assignment_dispatched"))
        .count()
        + 1;
    let correlation = args
        .get("correlation")
        .and_then(Value::as_str)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| format!("corr_{ordinal:03}"));
    // An isolated writer's workspace is its provisioned worktree; every other
    // dispatch works in the project checkout.
    let workspace = match &provision {
        Some(p) => p.worktree.to_string_lossy().to_string(),
        None => args
            .get("workspace")
            .and_then(Value::as_str)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                std::fs::canonicalize(project_root)
                    .unwrap_or_else(|_| project_root.to_path_buf())
                    .to_string_lossy()
                    .to_string()
            }),
    };

    ledger
        .append(json!({
            "type": "assignment_dispatched",
            "assignment": id,
            "correlation": correlation,
            "workspace": workspace,
        }))
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    if writable {
        let event = match &provision {
            Some(p) => leases::lease_acquired_event(
                &id,
                leases::ISOLATED_WORKTREE,
                Some(&p.worktree),
                Some(&p.cargo_target_dir),
                &p.worktree_include,
            ),
            None => leases::lease_acquired_event(
                &id,
                leases::IN_CHECKOUT_EXCLUSIVE,
                None,
                None,
                &[],
            ),
        };
        ledger
            .append(event)
            .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    }

    let state = RuntimeState::load(ledger);
    let mut result = state
        .assignment(&id)
        .expect("assignment exists before its lifecycle event")
        .to_json();
    if let Some(lease) = state.lease_for(&id) {
        result["lease"] = json!({
            "mode": lease.mode,
            "worktree": lease.worktree,
            "cargoTargetDir": lease.cargo_target_dir,
            "worktreeInclude": lease.worktree_include,
        });
    }
    Ok(result)
}

/// `workspace_close`: release the assignment's Writer Lease and clean/retain its
/// isolated worktree per assignment state — a handed-back candidate (evidence
/// awaiting manual integration) is retained, anything else is cleaned up.
fn workspace_close(
    ledger: &Ledger,
    project_root: &Path,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "workspace_close";
    let id = arg_str(args, TOOL, "assignment")?;
    let state = RuntimeState::load(ledger);
    let lease = state
        .lease_for(&id)
        .ok_or_else(|| {
            RpcError::invalid_params(format!(
                "{TOOL}: no active writer lease for `{id}`; workspace_close releases a lease held by a dispatched assignment"
            ))
        })?
        .clone();
    let asg = state
        .assignment(&id)
        .expect("a lease only exists for a recorded assignment")
        .clone();

    let action = if lease.mode == leases::ISOLATED_WORKTREE
        && let Some(worktree) = &lease.worktree
    {
        if ["HandedBack", "GateRunning"].contains(&asg.state.as_str()) {
            "retained"
        } else {
            leases::remove_worktree(project_root, Path::new(worktree))?;
            "removed"
        }
    } else {
        "none"
    };

    ledger
        .append(json!({
            "type": "writer_lease_released",
            "assignment": id,
            "worktreeAction": action,
        }))
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    let state = RuntimeState::load(ledger);
    let mut result = state
        .assignment(&id)
        .expect("assignment exists before its lifecycle event")
        .to_json();
    result["lease"] = json!({"released": true, "worktreeAction": action});
    Ok(result)
}

/// `lease_reclaim`: Lead force-reclaims a stale Writer Lease, appending an audit
/// event and freeing the writer slot. The isolated worktree, if any, stays on
/// disk for Lead to integrate or discard deliberately.
fn lease_reclaim(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "lease_reclaim";
    let id = arg_str(args, TOOL, "assignment")?;
    let state = RuntimeState::load(ledger);
    let lease = state
        .lease_for(&id)
        .ok_or_else(|| {
            RpcError::invalid_params(format!(
                "{TOOL}: no active writer lease for `{id}`; lease_reclaim force-frees a held lease"
            ))
        })?
        .clone();

    ledger
        .append(json!({
            "type": "writer_lease_reclaimed",
            "assignment": id,
            "mode": lease.mode,
            "worktreeAction": "retained",
        }))
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
    let state = RuntimeState::load(ledger);
    let mut result = state
        .assignment(&id)
        .expect("assignment exists before its lifecycle event")
        .to_json();
    result["lease"] = json!({
        "reclaimed": true,
        "mode": lease.mode,
        "worktree": lease.worktree,
        "worktreeAction": "retained",
    });
    Ok(result)
}

fn assignment_answer(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_answer";
    let id = arg_str(args, TOOL, "assignment")?;
    let answer = arg_str(args, TOOL, "answer")?;
    let asg = require_assignment(ledger, TOOL, &id)?;
    require_state(TOOL, "answering", &asg, &["Asking"])?;
    append_and_reload(
        ledger,
        TOOL,
        json!({"type": "assignment_answered", "assignment": id, "answer": answer}),
    )
}

fn assignment_accept(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_accept";
    let id = arg_str(args, TOOL, "assignment")?;
    let asg = require_assignment(ledger, TOOL, &id)?;
    require_state(TOOL, "accepting", &asg, &["HandedBack", "GateRunning"])?;
    let mut event = json!({"type": "assignment_accepted", "assignment": id});
    if let Some(note) = args.get("note").and_then(Value::as_str) {
        event["note"] = json!(note);
    }
    append_and_reload(ledger, TOOL, event)
}

fn assignment_close(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "assignment_close";
    let id = arg_str(args, TOOL, "assignment")?;
    let asg = require_assignment(ledger, TOOL, &id)?;

    // In-flight work is stopped; handed-back work is abandoned; a terminal
    // disposition (Accepted | Rejected | Abandoned) is finalized as Closed.
    let kind = match asg.state.as_str() {
        "Draft" | "Dispatched" | "Asking" => "assignment_stopped",
        "HandedBack" | "GateRunning" => "assignment_abandoned",
        "Accepted" | "Rejected" | "Abandoned" => "assignment_closed",
        "Closed" | "Stopped" => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: illegal transition: assignment `{id}` is already `{}`",
                asg.state
            )));
        }
        other => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: illegal transition: assignment `{id}` is `{other}`; it must reach a terminal disposition before closing"
            )));
        }
    };
    append_and_reload(ledger, TOOL, json!({"type": kind, "assignment": id}))
}

fn gate_run(ledger: &Ledger, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "gate_run";
    let id = arg_str(args, TOOL, "assignment")?;
    let asg = require_assignment(ledger, TOOL, &id)?;
    require_state(TOOL, "starting the gate", &asg, &["HandedBack"])?;
    let gate = asg.gate.clone();
    let result = append_and_reload(
        ledger,
        TOOL,
        json!({"type": "gate_started", "assignment": id, "gate": gate}),
    )?;
    // Execution is delivered by the detached Gate runner slice; this slice only
    // records the HandedBack -> GateRunning transition.
    Ok(json!({
        "id": id,
        "state": result["state"],
        "gate": result["gate"],
        "status": "pending",
        "note": "gate execution is delivered by the detached Gate runner"
    }))
}

// ---------------------------------------------------------------------------
// Peer tools (server-derived identity; spec user stories 23-24)
// ---------------------------------------------------------------------------

/// Structured `Challenge Signal` stances a Peer may raise via `ask`
/// (spec: Role-Asymmetric MCP Surface, Peer paragraph).
pub const CHALLENGE_SIGNALS: &[&str] = &["REOPEN_REQUEST", "DEPENDENCY_REQUEST", "BLOCKED"];

/// Argument keys a Peer tool call must never carry: identity, assignment, and
/// workspace are resolved server-side from the session correlation, never from
/// tool arguments (spec user story 23).
const PEER_IDENTITY_ARG_KEYS: &[&str] = &[
    "assignment",
    "assignment_id",
    "seat",
    "workspace",
    "correlation",
    "project",
];

/// Refuses any peer tool call that tries to supply its own identity binding.
fn refuse_identity_args(tool: &str, args: &Value) -> Result<(), RpcError> {
    for key in PEER_IDENTITY_ARG_KEYS {
        if args.get(key).is_some_and(|v| !v.is_null()) {
            return Err(RpcError::invalid_params(format!(
                "{tool}: `{key}` in tool args is refused: identity, assignment, and workspace are derived server-side from the session correlation"
            )));
        }
    }
    Ok(())
}

/// Resolves the peer's binding server-side: the most recently dispatched
/// assignment registered with this correlation (the binding is written by
/// `assignment_dispatch`), together with its workspace.
fn resolve_peer_binding(
    ledger: &Ledger,
    tool: &str,
    correlation: Option<&str>,
) -> Result<Assignment, RpcError> {
    let Some(correlation) = correlation.map(str::trim).filter(|c| !c.is_empty()) else {
        return Err(RpcError::invalid_params(format!(
            "{tool}: peer identity requires a session correlation (--correlation / HOTPOT_CORRELATION, injected at seat launch); the server never derives identity from tool arguments"
        )));
    };
    let state = RuntimeState::load(ledger);
    // The latest `assignment_dispatched` event carrying this correlation wins:
    // the ledger, not the caller, decides which assignment the seat works on.
    let bound = ledger
        .read_events()
        .iter()
        .rev()
        .find(|e| {
            e.get("type").and_then(Value::as_str) == Some("assignment_dispatched")
                && e.get("correlation").and_then(Value::as_str) == Some(correlation)
        })
        .and_then(|e| e.get("assignment").and_then(Value::as_str))
        .and_then(|id| state.assignment(id));
    let assignment = bound.ok_or_else(|| {
        RpcError::invalid_params(format!(
            "{tool}: correlation `{correlation}` is not bound to any dispatched assignment in this project"
        ))
    })?;
    if assignment
        .workspace
        .as_deref()
        .map(str::trim)
        .is_none_or(str::is_empty)
    {
        return Err(RpcError::internal(format!(
            "{tool}: the binding for correlation `{correlation}` records no workspace; re-dispatch the assignment"
        )));
    }
    Ok(assignment.clone())
}

/// Peer `ask`: raises a question (optionally a structured `Challenge Signal`)
/// for the Lead as a ledger event answerable via `assignment_answer`.
fn peer_ask(ledger: &Ledger, correlation: Option<&str>, args: &Value) -> Result<Value, RpcError> {
    const TOOL: &str = "ask";
    refuse_identity_args(TOOL, args)?;
    let question = arg_str(args, TOOL, "question")?;
    let signal = match args.get("signal") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => {
            let value = s.trim();
            if CHALLENGE_SIGNALS.contains(&value) {
                Some(value.to_string())
            } else {
                return Err(RpcError::invalid_params(format!(
                    "{TOOL}: invalid Challenge Signal `{value}`: permitted signals are {}",
                    CHALLENGE_SIGNALS.join("|")
                )));
            }
        }
        Some(_) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: `signal` must be a Challenge Signal string ({})",
                CHALLENGE_SIGNALS.join("|")
            )))
        }
    };

    let asg = resolve_peer_binding(ledger, TOOL, correlation)?;
    require_state(TOOL, "asking", &asg, &["Dispatched", "Asking"])?;

    let mut event = json!({
        "type": "assignment_asked",
        "assignment": asg.id,
        "correlation": asg.correlation,
        "question": question,
    });
    if let Some(signal) = &signal {
        event["signal"] = json!(signal);
    }
    ledger
        .append(event)
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;

    let reloaded = RuntimeState::load(ledger)
        .assignment(&asg.id)
        .expect("assignment exists before its lifecycle event")
        .clone();
    let mut result = json!({
        "state": reloaded.state,
        "question": question,
        "routedTo": "lead",
    });
    if let Some(signal) = &signal {
        result["signal"] = json!(signal);
    }
    if let Some(answer) = &reloaded.last_answer {
        result["lastAnswer"] = json!(answer);
    }
    Ok(result)
}

/// Peer `handoff`: submits finished work, validated against live Git evidence.
///
/// Writable assignments are validated in order (spec user story 24): a clean
/// working tree, a `Candidate` derived from `baseCommit..HEAD` with at least one
/// commit, the required Gate run after the peer's last write, and finally a
/// `scope.exceeded` flag (not a refusal) for changed paths outside the declared
/// Write Scope. Read-only assignments only require a clean tree and make no
/// candidate or lease claims.
fn peer_handoff(
    ledger: &Ledger,
    correlation: Option<&str>,
    args: &Value,
) -> Result<Value, RpcError> {
    const TOOL: &str = "handoff";
    refuse_identity_args(TOOL, args)?;
    let summary = arg_str(args, TOOL, "summary")?;
    let asg = resolve_peer_binding(ledger, TOOL, correlation)?;
    require_state(
        TOOL,
        "handing back",
        &asg,
        &["Dispatched", "Asking", "ReworkRequested"],
    )?;
    let workspace = Path::new(asg.workspace.as_deref().unwrap_or_default());

    // (1) Clean working tree in the peer's workspace.
    let dirty = leases::porcelain_entries(workspace)?;
    if !dirty.is_empty() {
        let sample: Vec<String> = dirty.iter().take(3).cloned().collect();
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: git precondition failed: the working tree of {} is not clean (`git status --porcelain=v1 --untracked-files=all` reports {} entr{}): {}; commit or revert everything before handing back",
            workspace.display(),
            dirty.len(),
            if dirty.len() == 1 { "y" } else { "ies" },
            sample.join(", ")
        )));
    }

    // Read-only assignments (no Write Scope) claim neither candidate nor lease.
    if asg.write_scope.is_empty() {
        ledger
            .append(json!({
                "type": "assignment_handed_back",
                "assignment": asg.id,
                "correlation": asg.correlation,
                "summary": summary,
            }))
            .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;
        let reloaded = RuntimeState::load(ledger)
            .assignment(&asg.id)
            .expect("assignment exists before its lifecycle event")
            .clone();
        return Ok(json!({
            "state": reloaded.state,
            "assignment": asg.id,
            "candidate": Value::Null,
            "readOnly": true,
            "handedBackAt": reloaded.handed_back_at,
        }));
    }

    // (2) Candidate: at least one commit in baseCommit..HEAD.
    let candidate = leases::head_commit(workspace)?;
    let range = format!("{}..{}", asg.base_commit, candidate);
    let count = workspace_git(workspace, &["rev-list", "--count", &range])?
        .trim()
        .parse::<u64>()
        .unwrap_or(0);
    if count == 0 {
        return Err(RpcError::invalid_params(format!(
            "{TOOL}: no Candidate: the workspace HEAD `{candidate}` has no commits past the assignment baseCommit `{}`; baseCommit..HEAD must contain at least one commit before handing back",
            asg.base_commit
        )));
    }

    // (3) The required Gate ran after the peer's last write (ledger evidence).
    let events = ledger.read_events();
    let last_write = evidence::latest_evidence_seq(&events, &asg.id, evidence::WRITE_EVENT);
    let last_gate = evidence::latest_evidence_seq(&events, &asg.id, evidence::GATE_EVENT);
    match (last_gate, last_write) {
        (None, _) => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: gate precondition failed: no gate-run record for `{}`; run the assignment Gate (`{}`) in the workspace after the last write, then hand off again",
                asg.id, asg.gate
            )));
        }
        (Some(gate), Some(write)) if gate < write => {
            return Err(RpcError::invalid_params(format!(
                "{TOOL}: gate precondition failed: the Gate last ran before the peer's last write (gate seq {gate} < write seq {write}); re-run the Gate (`{}`) after the final write, then hand off again",
                asg.gate
            )));
        }
        _ => {}
    }

    // (4) Changed paths outside the declared Write Scope are flagged, not refused.
    let changed = workspace_git(workspace, &["diff", "--name-only", &range])?;
    let violations: Vec<String> = changed
        .lines()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .filter(|p| !asg.write_scope.iter().any(|glob| leases::glob_matches(glob, p)))
        .map(str::to_string)
        .collect();

    let mut event = json!({
        "type": "assignment_handed_back",
        "assignment": asg.id,
        "correlation": asg.correlation,
        "summary": summary,
        "candidate": candidate,
        "baseCommit": asg.base_commit,
    });
    if !violations.is_empty() {
        event["scopeViolations"] = json!(violations);
    }
    ledger
        .append(event)
        .map_err(|e| RpcError::internal(format!("{TOOL}: {e}")))?;

    let reloaded = RuntimeState::load(ledger)
        .assignment(&asg.id)
        .expect("assignment exists before its lifecycle event")
        .clone();
    Ok(json!({
        "state": reloaded.state,
        "assignment": asg.id,
        "candidate": candidate,
        "baseCommit": asg.base_commit,
        "range": range,
        "scope": {"exceeded": !violations.is_empty(), "paths": violations},
        "handedBackAt": reloaded.handed_back_at,
    }))
}

/// Runs `git -C <workspace> <args..>` and returns trimmed stdout on success.
fn workspace_git(workspace: &Path, args: &[&str]) -> Result<String, RpcError> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(args)
        .output()
        .map_err(|e| RpcError::internal(format!("failed to invoke git: {e}")))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(RpcError::internal(format!(
            "git {:?} failed in {}: {}",
            args,
            workspace.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}
