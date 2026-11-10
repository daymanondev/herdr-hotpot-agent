// Attention Ack/Dismiss actions (ticket nui/07, acceptance criterion 1): the
// pure argv builder behind the panel's inline `Ack` / `Dismiss` buttons and the
// seat a letter's row-click selects. The exec'd CLI itself is ticket nui/04's
// Rust seam (`hotpot attention feedback <att_id> <useful|noisy>`); this file
// pins the panel-side contract: argv shape, fail-closed action validation, and
// every letter kind the panel surfaces rendering a row target plus both
// actions.

import test from "node:test";
import assert from "node:assert/strict";

import {
  ATTENTION_KIND_RANK,
  attentionView,
  attentionFeedbackArgs,
  attentionSeatTarget,
} from "../../shared/protocol.js";

// ---------------------------------------------------------------------------
// The feedback argv builder
// ---------------------------------------------------------------------------

test("Ack builds the `attention feedback <att_id> useful` argv", () => {
  assert.deepEqual(attentionFeedbackArgs("att_incident_seq6", "useful"), [
    "attention",
    "feedback",
    "att_incident_seq6",
    "useful",
  ]);
});

test("Dismiss builds the `attention feedback <att_id> noisy` argv", () => {
  assert.deepEqual(attentionFeedbackArgs("att_stalled_permission_perm_9", "noisy"), [
    "attention",
    "feedback",
    "att_stalled_permission_perm_9",
    "noisy",
  ]);
});

test("any action other than useful|noisy throws before any exec", () => {
  for (const action of [
    "ack",
    "dismiss",
    "mute",
    "snooze",
    "downrank",
    "USEFUL",
    "",
    null,
    undefined,
  ]) {
    assert.throws(
      () => attentionFeedbackArgs("att_incident_seq6", action),
      undefined,
      `action ${JSON.stringify(action)} is refused`,
    );
  }
});

test("a letter key that is not a non-empty string throws before any exec", () => {
  for (const key of ["", "   ", null, undefined]) {
    assert.throws(() => attentionFeedbackArgs(key, "useful"));
  }
});

// ---------------------------------------------------------------------------
// Every letter kind the panel surfaces: row-click target plus both actions
// ---------------------------------------------------------------------------

// One live room: a Lead plus the peer holding asg_001, the seat shapes
// `hotpot status` serves (`src/status.rs`: id, role, assignment, terminal).
const SIX_KIND_SNAPSHOT = {
  seats: [
    { id: "lead-1", role: "lead", agent: "codex", status: "idle", terminal: "term-lead-1" },
    {
      id: "peer-a",
      role: "peer",
      agent: "claude",
      status: "working",
      terminal: "term-a",
      assignment: "asg_001",
    },
  ],
  attention: [
    { key: "att_needs_human_seq5", kind: "needs-human", text: "NEEDS-HUMAN: sign off" },
    { key: "att_incident_seq6", kind: "incident", text: "INCIDENT: registry down" },
    {
      key: "att_stalled_permission_perm_9",
      kind: "stalled-permission",
      text: "permission perm_9 awaiting a decision",
      permission: "perm_9",
      assignment: "asg_001",
    },
    {
      key: "att_unread_handback_asg_001",
      kind: "unread-handback",
      text: "peer handback on asg_001 awaits Lead review",
      assignment: "asg_001",
    },
    {
      key: "att_repeated_failure_asg_001",
      kind: "repeated-failure",
      text: "gate failed 3 times in a row on asg_001",
      assignment: "asg_001",
    },
    { key: "att_lead_context_rotation", kind: "lead-context-rotation", text: "rotation mark" },
  ],
};

test("the fixture covers exactly the six kinds the backend ranks", () => {
  const view = attentionView(SIX_KIND_SNAPSHOT);
  assert.deepEqual(
    view.items.map((item) => item.kind).sort(),
    Object.keys(ATTENTION_KIND_RANK).sort(),
    "one letter per ranked kind — the panel's whole attention surface",
  );
});

test("letters that name an assignment select that assignment's peer seat", () => {
  const byKind = Object.fromEntries(
    attentionView(SIX_KIND_SNAPSHOT).items.map((item) => [item.kind, item]),
  );
  for (const kind of ["stalled-permission", "unread-handback", "repeated-failure"]) {
    const seat = attentionSeatTarget(byKind[kind], SIX_KIND_SNAPSHOT);
    assert.equal(seat?.id, "peer-a", `${kind} row-click selects the affected peer`);
  }
});

test("Lead-side letters select the Lead seat", () => {
  const byKind = Object.fromEntries(
    attentionView(SIX_KIND_SNAPSHOT).items.map((item) => [item.kind, item]),
  );
  for (const kind of ["needs-human", "incident", "lead-context-rotation"]) {
    const seat = attentionSeatTarget(byKind[kind], SIX_KIND_SNAPSHOT);
    assert.equal(seat?.id, "lead-1", `${kind} row-click selects the Lead`);
  }
});

test("every letter kind carries both actions as valid feedback argv", () => {
  for (const item of attentionView(SIX_KIND_SNAPSHOT).items) {
    assert.deepEqual(attentionFeedbackArgs(item.key, "useful"), [
      "attention",
      "feedback",
      item.key,
      "useful",
    ]);
    assert.deepEqual(attentionFeedbackArgs(item.key, "noisy"), [
      "attention",
      "feedback",
      item.key,
      "noisy",
    ]);
  }
});

test("a letter whose assignment has no live peer falls back to the Lead", () => {
  const seat = attentionSeatTarget(
    { key: "att_unread_handback_asg_002", kind: "unread-handback", assignment: "asg_002" },
    SIX_KIND_SNAPSHOT,
  );
  assert.equal(seat?.id, "lead-1");
});

test("a room with no seats resolves no target — the row renders without one", () => {
  const letter = { key: "att_incident_seq6", kind: "incident" };
  assert.equal(attentionSeatTarget(letter, { seats: [] }), null);
  assert.equal(attentionSeatTarget(letter, null), null);
});
