// The Seat Slot view model (ticket nui/01, acceptance criterion 4): the pure
// helper that always yields the Project's Supervisor and Lead slots — launched
// with live seat data when a matching seat exists, else `unlaunched` with the
// role's resolved profile defaults — plus the child Peer rows attached under
// Lead. Degrades safely on null and partial snapshots.

import test from "node:test";
import assert from "node:assert/strict";

import { seatSlotsModel } from "../../shared/protocol.js";

// A worked live snapshot shaped exactly like `hotpot status` serves it: a live
// supervisor and lead seat, one dispatched peer, and the resolved profiles.
const LIVE_SNAPSHOT = {
  provisioned: true,
  hasWorkspaceProtocol: true,
  profiles: {
    supervisor: { agent: "claude", model: "claude-opus-4-6", thinking: "high" },
    lead: { agent: "codex", model: "gpt-5.1-codex", thinking: "high" },
    peer: { agent: "codex", model: "gpt-5.1-codex", thinking: "medium" },
  },
  seats: [
    {
      id: "claude-supervisor",
      role: "supervisor",
      agent: "claude",
      model: "claude-opus-4-6",
      thinking: "high",
      status: "live",
      terminal: "term-sup-7",
    },
    {
      id: "codex-lead",
      role: "lead",
      agent: "codex",
      model: "gpt-5.1-codex",
      thinking: "high",
      status: "idle",
      terminal: "term-lead-8",
    },
    {
      id: "pi-peer-asg_001",
      role: "peer",
      agent: "pi",
      model: "zai-glm-chatter",
      thinking: "high",
      status: "working",
      terminal: "term-peer-9",
      assignment: "asg_001",
    },
  ],
  tasks: [
    {
      id: "asg_001",
      state: "HandedBack",
      disposition: "Engineer",
      verdict: "green",
      attempt: 1,
      terminal: "term-peer-9",
    },
  ],
};

test("a live snapshot yields launched supervisor and lead slots with their seat data", () => {
  const model = seatSlotsModel(LIVE_SNAPSHOT);

  assert.equal(model.supervisor.launched, true);
  assert.equal(model.supervisor.status, "live");
  assert.equal(model.supervisor.agent, "claude");
  assert.equal(model.supervisor.model, "claude-opus-4-6");
  assert.equal(model.supervisor.thinking, "high");
  assert.equal(model.supervisor.terminal, "term-sup-7");
  assert.equal(model.supervisor.seat.id, "claude-supervisor");

  assert.equal(model.lead.launched, true);
  assert.equal(model.lead.status, "idle");
  assert.equal(model.lead.agent, "codex");
  assert.equal(model.lead.terminal, "term-lead-8");
  assert.equal(model.lead.seat.id, "codex-lead");
});

test("the dispatched peers attach under lead with assignment, disposition, gate, attempt, terminal", () => {
  const model = seatSlotsModel(LIVE_SNAPSHOT);
  assert.equal(Array.isArray(model.lead.peers), true);
  assert.equal(model.lead.peers.length, 1);

  const peer = model.lead.peers[0];
  assert.equal(peer.id, "pi-peer-asg_001");
  assert.equal(peer.role, "peer");
  assert.equal(peer.assignment, "asg_001");
  assert.equal(peer.disposition, "Engineer");
  assert.equal(peer.gate, "green");
  assert.equal(peer.attempt, 1);
  assert.equal(peer.terminal, "term-peer-9");
  assert.equal(peer.agent, "pi");
  assert.equal(peer.model, "zai-glm-chatter");
  assert.equal(peer.status, "working");
});

test("an empty snapshot still yields both slots unlaunched with the profile defaults", () => {
  const snapshot = {
    provisioned: false,
    hasWorkspaceProtocol: false,
    profiles: LIVE_SNAPSHOT.profiles,
    seats: [],
    tasks: [],
  };
  const model = seatSlotsModel(snapshot);

  for (const role of ["supervisor", "lead"]) {
    const slot = model[role];
    assert.equal(slot.launched, false, `${role} slot is unlaunched`);
    assert.equal(slot.status, "unlaunched");
    assert.equal(slot.seat, null);
    assert.equal(slot.terminal, null);
    assert.deepEqual(
      { agent: slot.agent, model: slot.model, thinking: slot.thinking },
      snapshot.profiles[role],
      `${role} slot carries the resolved profile defaults`,
    );
  }
  assert.deepEqual(model.lead.peers, []);
});

test("a null snapshot yields a safe empty model without throwing", () => {
  const model = seatSlotsModel(null);
  for (const role of ["supervisor", "lead"]) {
    const slot = model[role];
    assert.equal(slot.launched, false);
    assert.equal(slot.status, "unlaunched");
    assert.equal(slot.agent, null);
    assert.equal(slot.model, null);
    assert.equal(slot.thinking, null);
    assert.equal(slot.seat, null);
    assert.equal(slot.terminal, null);
  }
  assert.deepEqual(model.lead.peers, []);
});

test("a partial snapshot (no profiles, roleless seats) degrades safely", () => {
  const model = seatSlotsModel({ seats: [{ id: "mystery" }] });
  assert.equal(model.supervisor.launched, false);
  assert.equal(model.lead.launched, false);
  assert.equal(model.supervisor.agent, null);
  assert.equal(model.lead.peers.length, 0);

  // A snapshot with no seats array at all is equally safe.
  const bare = seatSlotsModel({ profiles: { lead: { agent: "pi" } } });
  assert.equal(bare.lead.launched, false);
  assert.equal(bare.lead.agent, "pi");
  assert.equal(bare.lead.model, null);
});
