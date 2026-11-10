// Panel restructure helpers (ticket nui/05): the section-visibility model
// behind *Quiet When Quiet*, the `Start` argv sequence over an unlaunched Seat
// Slot, the header preset-menu mapping, and the child Peer tree shape under
// Lead. All pure over one `hotpot status` snapshot shape.

import test from "node:test";
import assert from "node:assert/strict";

import {
  sectionVisibility,
  seatSlotsModel,
  setupApplyArgs,
  seatLaunchArgs,
  startSlotSteps,
  SETUP_APPLY_TIMEOUT_MS,
  presetMenu,
  PRESET_NAMES,
} from "../../shared/protocol.js";

// A quiet, fresh-Project snapshot: nothing provisioned, nothing launched, no
// letters, no assignments, no mailbox traffic — the panel's quietest state.
const QUIET_SNAPSHOT = {
  provisioned: false,
  hasWorkspaceProtocol: false,
  profiles: {
    supervisor: { agent: "claude", model: "claude-opus-4-6", thinking: "high" },
    lead: { agent: "codex", model: "gpt-5.1-codex", thinking: "high" },
    peer: { agent: "codex", model: "gpt-5.1-codex", thinking: "medium" },
  },
  seats: [],
  tasks: [],
  attention: [],
  mailbox: [],
};

test("a quiet snapshot collapses attention to All clear", () => {
  const sections = sectionVisibility(QUIET_SNAPSHOT);
  assert.equal(sections.attention.quiet, true);
  assert.equal(sections.attention.count, 0);
});

test("a quiet snapshot hides the mailbox (Activity) section entirely", () => {
  const sections = sectionVisibility(QUIET_SNAPSHOT);
  assert.equal(sections.activity.hidden, true);
  assert.equal(sections.activity.count, 0);
});

test("a snapshot with no assignments folds Assignments to its header line", () => {
  const sections = sectionVisibility(QUIET_SNAPSHOT);
  assert.equal(sections.assignments.folded, true);
  assert.equal(sections.assignments.count, 0);
});

test("each surface comes back once its snapshot carries items", () => {
  const sections = sectionVisibility({
    ...QUIET_SNAPSHOT,
    attention: [{ kind: "incident", key: "att_1", text: "incident text" }],
    mailbox: [{ from: "lead", to: "peer-1", kind: "claim", summary: "claim asg_001" }],
    tasks: [{ id: "asg_001", state: "Dispatched", disposition: "Engineer" }],
  });
  assert.equal(sections.attention.quiet, false);
  assert.equal(sections.attention.count, 1);
  assert.equal(sections.activity.hidden, false);
  assert.equal(sections.activity.count, 1);
  assert.equal(sections.assignments.folded, false);
  assert.equal(sections.assignments.count, 1);
});

test("the Seats count covers the two always-present slots plus their peers", () => {
  const quiet = sectionVisibility(QUIET_SNAPSHOT);
  assert.equal(quiet.seats.count, 2);

  const withPeers = sectionVisibility({
    ...QUIET_SNAPSHOT,
    seats: [
      { id: "peer-a", role: "peer", status: "working", assignment: "asg_001" },
      { id: "peer-b", role: "peer", status: "idle", assignment: "asg_002" },
    ],
    tasks: [
      { id: "asg_001", disposition: "Engineer" },
      { id: "asg_002", disposition: "Reviewer" },
    ],
  });
  assert.equal(withPeers.seats.count, 4);
});

test("a null snapshot is still fully quiet and safe", () => {
  const sections = sectionVisibility(null);
  assert.equal(sections.attention.quiet, true);
  assert.equal(sections.activity.hidden, true);
  assert.equal(sections.assignments.folded, true);
  assert.equal(sections.seats.count, 2);
});

// ---------------------------------------------------------------------------
// The `Start` exec sequence over an unlaunched Seat Slot
// ---------------------------------------------------------------------------

// A fresh Project through the panel's own model: both slots unlaunched with
// their configured profile defaults, nothing provisioned yet.
function freshSlots() {
  return seatSlotsModel(QUIET_SNAPSHOT);
}

test("a fresh-Project snapshot still yields both slots unlaunched with profile defaults", () => {
  const slots = freshSlots();
  for (const role of ["supervisor", "lead"]) {
    const slot = slots[role];
    assert.equal(slot.launched, false, `${role} slot is unlaunched`);
    assert.equal(slot.status, "unlaunched");
    assert.deepEqual(
      { agent: slot.agent, model: slot.model, thinking: slot.thinking },
      QUIET_SNAPSHOT.profiles[role],
      `${role} slot carries the role's profile defaults`,
    );
  }
  assert.deepEqual(slots.lead.peers, []);
});

test("the setup argv is exactly `setup --apply`", () => {
  assert.deepEqual(setupApplyArgs(), ["setup", "--apply"]);
});

test("the seat launch argv carries the role, its profile agent, and the project folder", () => {
  assert.deepEqual(
    seatLaunchArgs("supervisor", "claude", "/repos/herdr-hotpot-agent"),
    ["seat", "launch", "supervisor", "--agent", "claude", "--project", "/repos/herdr-hotpot-agent"],
  );
});

test("Start on an unprovisioned slot runs setup --apply first, then the launch", () => {
  const slots = freshSlots();
  const steps = startSlotSteps(slots.supervisor, QUIET_SNAPSHOT, "/repos/demo");
  assert.equal(steps.length, 2);
  assert.deepEqual(steps[0].args, ["setup", "--apply"]);
  assert.equal(steps[0].timeoutMs, SETUP_APPLY_TIMEOUT_MS);
  assert.deepEqual(steps[1].args, [
    "seat",
    "launch",
    "supervisor",
    "--agent",
    "claude",
    "--project",
    "/repos/demo",
  ]);
  assert.equal(steps[1].timeoutMs, null);
});

test("Start on a provisioned project launches directly, with no setup step", () => {
  const slots = seatSlotsModel({ ...QUIET_SNAPSHOT, provisioned: true });
  const steps = startSlotSteps(slots.lead, { provisioned: true }, "/repos/demo");
  assert.equal(steps.length, 1);
  assert.deepEqual(steps[0].args, [
    "seat",
    "launch",
    "lead",
    "--agent",
    "codex",
    "--project",
    "/repos/demo",
  ]);
});

test("a snapshot that predates `provisioned` (undefined) skips setup too", () => {
  const slots = seatSlotsModel({ profiles: QUIET_SNAPSHOT.profiles });
  const steps = startSlotSteps(slots.lead, {}, "/repos/demo");
  assert.equal(steps.length, 1);
  assert.equal(steps[0].args[0], "seat");
});

test("the launch argv builder refuses unknown roles, families, and folders", () => {
  assert.throws(() => seatLaunchArgs("owner", "claude", "/repos/demo"));
  assert.throws(() => seatLaunchArgs("lead", "zcode", "/repos/demo"));
  assert.throws(() => seatLaunchArgs("lead", "codex", ""));
  assert.throws(() => seatLaunchArgs("lead", "codex", null));
});

test("Start over a slot with no configured agent throws before any exec", () => {
  const slots = seatSlotsModel(null);
  assert.throws(() => startSlotSteps(slots.supervisor, null, "/repos/demo"));
});

// ---------------------------------------------------------------------------
// The header preset-menu mapping
// ---------------------------------------------------------------------------

test("the preset menu lists the three named presets under stable unique ids", () => {
  const menu = presetMenu();
  assert.deepEqual(
    menu.map((item) => item.name),
    PRESET_NAMES,
  );
  assert.deepEqual(
    menu.map((item) => item.id),
    ["deep-architecture", "fast-iteration", "budget"],
  );
  assert.equal(new Set(menu.map((item) => item.id)).size, menu.length, "menu ids are unique");
});

test("the preset menu carries each full preset name — the dropdown is their only surface", () => {
  for (const item of presetMenu()) {
    assert.equal(
      PRESET_NAMES.includes(item.name),
      true,
      `${item.name} is one of the named presets`,
    );
    assert.equal(item.name.includes("…"), false, `${item.name} is never truncated`);
    assert.equal(item.name.includes("..."), false, `${item.name} is never truncated`);
  }
});

// ---------------------------------------------------------------------------
// The child Peer tree under Lead
// ---------------------------------------------------------------------------

// Two dispatched peers under one live Lead: the tree shape the panel renders.
const TWO_PEER_SNAPSHOT = {
  provisioned: true,
  profiles: QUIET_SNAPSHOT.profiles,
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
    {
      id: "peer-b",
      role: "peer",
      agent: "pi",
      status: "blocked",
      assignment: "asg_002",
    },
  ],
  tasks: [
    { id: "asg_001", disposition: "Engineer", verdict: "green", attempt: 2 },
    { id: "asg_002", disposition: "Reviewer", state: "Dispatched" },
  ],
};

test("two peers render as exactly two rows — ├ then the last └ — under Lead", () => {
  const peers = seatSlotsModel(TWO_PEER_SNAPSHOT).lead.peers;
  assert.equal(peers.length, 2);
  assert.deepEqual(
    peers.map((peer) => peer.connector),
    ["├", "└"],
  );
});

test("each peer row carries its one-line payload: assignment, disposition, gate, attempt", () => {
  const [first, second] = seatSlotsModel(TWO_PEER_SNAPSHOT).lead.peers;

  assert.equal(first.assignment, "asg_001");
  assert.equal(first.disposition, "Engineer");
  assert.equal(first.gate, "green");
  assert.equal(first.attempt, 2);
  assert.equal(second.assignment, "asg_002");
  assert.equal(second.disposition, "Reviewer");
  assert.equal(second.gate, null);
  assert.equal(second.attempt, null);
});

test("a peer with a terminal exposes it; a peer without one exposes none", () => {
  const [withTerminal, withoutTerminal] = seatSlotsModel(TWO_PEER_SNAPSHOT).lead.peers;
  assert.equal(withTerminal.terminal, "term-a");
  assert.equal(withoutTerminal.terminal, null);
});

test("a single peer is the last row and takes the └ connector", () => {
  const peers = seatSlotsModel({
    ...TWO_PEER_SNAPSHOT,
    seats: TWO_PEER_SNAPSHOT.seats.filter((seat) => seat.role !== "peer" || seat.id === "peer-a"),
    tasks: [TWO_PEER_SNAPSHOT.tasks[0]],
  }).lead.peers;
  assert.deepEqual(peers.map((peer) => peer.connector), ["└"]);
});
