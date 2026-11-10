// Native panel chrome helpers (ticket 06, acceptance criteria 1 + 2):
// the pure agent → vendor-icon descriptor the panel renders through Herdr's
// `knownAgents()` registry, and the quiet geometric seat state markers that
// replace text spinners. The mod renders these descriptors with gpui-kit
// (`svg(icon).text_color(...)` when `tint`, else `image(icon)`); the model
// under test is the whole decision layer, node-testable outside herdr.

import test from "node:test";
import assert from "node:assert/strict";

import {
  agentIconModel,
  seatStatusMarker,
  markerToneColor,
  applyPickerPatch,
  profileSetArgs,
} from "../../shared/protocol.js";

// A `knownAgents()` fixture mirroring Herdr's program table shape
// (gpui-kit.d.ts `Program`: `key`, `icon`, `tint`, ...).
const REGISTRY = [
  { key: "claude", name: "Claude Code", icon: "icons/claude.svg", tint: true },
  { key: "codex", name: "Codex", icon: "icons/codex.png", tint: false },
  { key: "pi", name: "Pi", icon: "icons/pi.svg", tint: true },
  { key: "shell", name: "zsh", icon: null, tint: false },
];

// ---------------------------------------------------------------------------
// Agent → vendor icon descriptor (acceptance criterion 1)
// ---------------------------------------------------------------------------

test("claude, codex, and pi resolve to their vendor icon with the registry's tint flag", () => {
  assert.deepEqual(agentIconModel("claude", REGISTRY), {
    kind: "vendor",
    icon: "icons/claude.svg",
    tint: true,
  });
  assert.deepEqual(agentIconModel("codex", REGISTRY), {
    kind: "vendor",
    icon: "icons/codex.png",
    tint: false,
  });
  assert.deepEqual(agentIconModel("pi", REGISTRY), {
    kind: "vendor",
    icon: "icons/pi.svg",
    tint: true,
  });
});

test("an agent the registry lacks resolves to the terminal fallback descriptor", () => {
  assert.deepEqual(agentIconModel("zcode", REGISTRY), {
    kind: "fallback",
    icon: "mods/workspace-chrome/terminal.svg",
    tint: true,
  });
});

test("a registry entry without a usable icon falls back too", () => {
  assert.equal(agentIconModel("shell", REGISTRY).kind, "fallback");
  assert.equal(agentIconModel("claude", []).kind, "fallback");
  assert.equal(agentIconModel("claude", null).kind, "fallback");
});

// ---------------------------------------------------------------------------
// Seat status → geometric state marker (acceptance criterion 2)
// ---------------------------------------------------------------------------

test("active working statuses map to the spinner marker spinning at 900ms", () => {
  for (const status of ["working", "running", "live", "busy"]) {
    assert.deepEqual(seatStatusMarker(status), {
      kind: "spinner",
      icon: "mods/workspace-chrome/spinner.svg",
      spinMs: 900,
      tone: "working",
    }, `status ${status}`);
  }
});

test("idle maps to a 7px ring, done and unlaunched to 7px dots", () => {
  assert.deepEqual(seatStatusMarker("idle"), { kind: "ring", size: 7, tone: "idle" });
  assert.deepEqual(seatStatusMarker("done"), { kind: "dot", size: 7, tone: "done" });
  assert.deepEqual(seatStatusMarker("unlaunched"), { kind: "dot", size: 7, tone: "dim" });
});

test("blocked maps to the tinted exclamation badge", () => {
  assert.deepEqual(seatStatusMarker("blocked"), { kind: "badge", glyph: "!", tone: "warning" });
});

test("unknown statuses get a neutral ring, never an exception color", () => {
  for (const status of ["weird", "offline", "", null, undefined]) {
    const marker = seatStatusMarker(status);
    assert.equal(marker.kind, "ring", `status ${status} is a ring`);
    assert.equal(marker.tone, "neutral", `status ${status} is neutral`);
    assert.notEqual(marker.tone, "warning", `status ${status} never warns`);
  }
});

// ---------------------------------------------------------------------------
// Marker tone → theme color (quiet palette; only the exception warns)
// ---------------------------------------------------------------------------

const TONE_THEME = {
  working: "#work",
  accent: "#acc",
  blocked: "#blk",
  ansi1: "#a1",
  done: "#done",
  ansi2: "#a2",
  idle: "#idle",
  dim: "#dim",
};

test("marker tones resolve to theme colors, neutral staying quiet", () => {
  assert.equal(markerToneColor(TONE_THEME, "working"), "#work");
  assert.equal(markerToneColor(TONE_THEME, "idle"), "#idle");
  assert.equal(markerToneColor(TONE_THEME, "done"), "#done");
  assert.equal(markerToneColor(TONE_THEME, "dim"), "#dim");
  assert.equal(markerToneColor(TONE_THEME, "warning"), "#blk");
  assert.equal(markerToneColor(TONE_THEME, "neutral"), "#idle");
  assert.notEqual(markerToneColor(TONE_THEME, "neutral"), "#blk");
});

test("marker tones fall back like statusColor when a theme key is absent", () => {
  const bare = { accent: "#acc", ansi1: "#a1", ansi2: "#a2", dim: "#dim" };
  assert.equal(markerToneColor(bare, "working"), "#acc");
  assert.equal(markerToneColor(bare, "warning"), "#a1");
  assert.equal(markerToneColor(bare, "done"), "#a2");
  assert.equal(markerToneColor(bare, "neutral"), "#dim");
});

// ---------------------------------------------------------------------------
// The pill selector's selection model (acceptance criterion 4): each click is
// a patch over the draft that persists immediately through `profileSetArgs`.
// ---------------------------------------------------------------------------

const DRAFT = { agent: "codex", model: "gpt-5.1-codex", thinking: "high" };

test("a thinking patch keeps the family and model and returns a persistable selection", () => {
  const next = applyPickerPatch(DRAFT, { thinking: "medium" });
  assert.deepEqual(next, { agent: "codex", model: "gpt-5.1-codex", thinking: "medium" });
  assert.deepEqual(
    profileSetArgs(next.agent, "lead", next.model, next.thinking),
    ["profile", "set", "codex", "lead", "gpt-5.1-codex", "--thinking", "medium"],
  );
});

test("a family switch resets the model to the new family's first option", () => {
  const next = applyPickerPatch(DRAFT, { agent: "pi" });
  assert.equal(next.agent, "pi");
  assert.equal(next.model, "zai-glm-chatter", "the codex model is not a pi option");
  assert.equal(next.thinking, "high");
});

test("a model pick under another family carries the family with it", () => {
  const next = applyPickerPatch(DRAFT, { agent: "claude", model: "claude-opus-4-6" });
  assert.deepEqual(next, {
    agent: "claude",
    model: "claude-opus-4-6",
    thinking: "high",
  });
});

test("a patch naming an unknown family is refused before anything execs", () => {
  assert.throws(() => applyPickerPatch(DRAFT, { agent: "zcode" }));
});
