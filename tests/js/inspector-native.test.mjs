// Session Inspector native redesign helpers (ticket 08, acceptance criteria
// 1 + 3): the pure tool-call collapse model that owns the compact row's
// default-collapsed state and toggle semantics, and the native header model
// (vendor icon descriptor, `Terminal · Alt+T` action, geometric context bar).
// The mod renders these models with gpui-kit; the model under test is the
// whole decision layer, node-testable outside herdr.

import test from "node:test";
import assert from "node:assert/strict";

import {
  toolCallCollapsedModel,
  inspectorHeaderModel,
  contextBarModel,
  defaultItemExpanded,
} from "../../shared/protocol.js";

// A `knownAgents()` fixture mirroring Herdr's program table shape
// (gpui-kit.d.ts `Program`: `key`, `icon`, `tint`, ...).
const REGISTRY = [
  { key: "claude", name: "Claude Code", icon: "icons/claude.svg", tint: true },
  { key: "codex", name: "Codex", icon: "icons/codex.png", tint: false },
  { key: "pi", name: "Pi", icon: "icons/pi.svg", tint: true },
];

// A transcript fixture shaped like `hotpot inspect`'s tool_call items
// (`src/inspector/mod.rs`: title = tool name, body = input payload, status =
// done once answered).
const CALL = {
  role: "assistant",
  kind: "tool_call",
  title: "lead_set_contract",
  body: '{"task":"T-01","gate":"cargo test"}',
  status: "done",
};

// ---------------------------------------------------------------------------
// Tool-call collapse model (acceptance criterion 1)
// ---------------------------------------------------------------------------

test("tool_call items default to the compact collapsed row regardless of seat activity", () => {
  for (const seatActive of [true, false]) {
    const model = toolCallCollapsedModel(CALL, seatActive);
    assert.equal(model.applies, true, `seatActive ${seatActive}`);
    assert.equal(model.collapsed, true, `seatActive ${seatActive}`);
  }
});

test("a pinned override resolves open and closed rows", () => {
  assert.equal(toolCallCollapsedModel(CALL, true, false).collapsed, false, "pinned open");
  assert.equal(toolCallCollapsedModel(CALL, true, true).collapsed, true, "pinned closed");
  assert.equal(toolCallCollapsedModel(CALL, true, null).collapsed, true, "null follows the default");
});

test("toggling open and closed is an involution back to the collapsed default", () => {
  const first = toolCallCollapsedModel(CALL, true); // collapsed default
  assert.equal(first.toggle(), false, "the first click opens the row");
  const second = toolCallCollapsedModel(CALL, true, first.toggle());
  assert.equal(second.collapsed, false);
  assert.equal(second.toggle(), true, "the second click closes it again");
  const third = toolCallCollapsedModel(CALL, true, second.toggle());
  assert.equal(third.collapsed, true, "back at the collapsed default");
});

test("the tool-call model never touches thinking items", () => {
  const thinking = { kind: "thinking", body: "long reasoning body" };
  const model = toolCallCollapsedModel(thinking, true);
  assert.equal(model.applies, false, "thinking stays outside the tool-call model");
  assert.equal(model.collapsed, null, "thinking collapse stays defaultItemExpanded's call");
});

test("thinking keeps its expand-on-active defaults alongside the tool-call model", () => {
  const thinking = { kind: "thinking", body: "long reasoning body" };
  assert.equal(defaultItemExpanded(thinking, true), true, "expanded while the turn is active");
  assert.equal(defaultItemExpanded(thinking, false), false, "collapsed once the seat rests");
});

// ---------------------------------------------------------------------------
// Native header model (acceptance criterion 3)
// ---------------------------------------------------------------------------

const SEAT = {
  id: "seat-lead-1",
  title: "Lead",
  role: "lead",
  agent: "codex",
  model: "gpt-5.1-codex",
  terminal: "term-lead-1",
  context: { cacheRead: 90000, freshInput: 50000, output: 10000 },
};

test("a seat with a live terminal resolves the Terminal action enabled", () => {
  const header = inspectorHeaderModel(SEAT, REGISTRY);
  assert.equal(header.terminalAction.label, "Terminal · Alt+T");
  assert.equal(header.terminalAction.enabled, true);
  assert.equal(header.terminalAction.hint, null);
});

test("a seat without a terminal resolves the Terminal action disabled with a hint", () => {
  const header = inspectorHeaderModel({ ...SEAT, terminal: null }, REGISTRY);
  assert.equal(header.terminalAction.enabled, false);
  assert.equal(typeof header.terminalAction.hint, "string");
  assert.ok(header.terminalAction.hint.length > 0, "the disabled action says why");
});

test("the header context bar reuses contextBarModel, ticking at 150k and not at 149k", () => {
  const at150k = inspectorHeaderModel(
    { ...SEAT, context: { cacheRead: 90000, freshInput: 50000, output: 10000 } },
    REGISTRY,
  );
  assert.deepEqual(
    at150k.context,
    contextBarModel({ cacheRead: 90000, freshInput: 50000, output: 10000 }),
  );
  assert.equal(at150k.context.total, 150000);
  assert.equal(at150k.context.rotationTick, true, "150k sits at the mark, so it ticks");

  const at149k = inspectorHeaderModel(
    { ...SEAT, context: { cacheRead: 90000, freshInput: 49000, output: 10000 } },
    REGISTRY,
  );
  assert.equal(at149k.context.total, 149000);
  assert.equal(at149k.context.rotationTick, false, "149k sits under the mark, so it does not tick");
});

test("the header renders the seat's vendor icon through the 06 descriptor rule", () => {
  const header = inspectorHeaderModel(SEAT, REGISTRY);
  assert.deepEqual(header.icon, { kind: "vendor", icon: "icons/codex.png", tint: false });
});

test("the header resolves the agent from the seat entry's provider alias", () => {
  const header = inspectorHeaderModel(
    { title: "Peer · asg_001", role: "peer", provider: "claude", terminal: "term-p" },
    REGISTRY,
  );
  assert.deepEqual(header.icon, { kind: "vendor", icon: "icons/claude.svg", tint: true });
});

test("an agent the registry lacks falls back to the terminal glyph, and no seat at all stays total", () => {
  assert.equal(inspectorHeaderModel({ ...SEAT, agent: "zcode" }, REGISTRY).icon.kind, "fallback");
  const empty = inspectorHeaderModel(null, []);
  assert.deepEqual(empty.icon, {
    kind: "fallback",
    icon: "mods/workspace-chrome/terminal.svg",
    tint: true,
  });
  assert.equal(empty.terminalAction.enabled, false);
  assert.equal(empty.context.total, 0);
});
