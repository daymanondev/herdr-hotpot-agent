// Static contract test for the Session Inspector mod (ticket 14): mods run in
// the herdr-app gpui runtime, never a browser or Node context — no timers
// beyond `cx.timer`, no DOM globals. Ticket 13's shared scan generalizes this
// to every mod; this test covers `mods/hotpot-session/main.js`.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const FORBIDDEN = /setTimeout|setInterval|document\.|window\./;

test("the session inspector mod uses no browser or Node timer globals", () => {
  const source = readFileSync(
    new URL("../../mods/hotpot-session/main.js", import.meta.url),
    "utf8",
  );
  const offenders = source.split("\n").map((line, i) => [i + 1, line]).filter(([, line]) => FORBIDDEN.test(line));
  assert.deepEqual(
    offenders.map(([line]) => line),
    [],
    "mods/ must use cx.timer / cx.spawn instead of setTimeout/setInterval and has no DOM access",
  );
});

// ---------------------------------------------------------------------------
// Ticket 05: source hygiene over the right-hand panel (mods/hotpot/main.js).
// The panel speaks Herdr's native visual grammar — no emoji characters, no
// "█" block bars, no ALL-CAPS section-heading strings, and no empty
// placeholder boxes (strict *Quiet When Quiet*). The "├"/"└" tree connectors
// and "·" separators are permitted native chrome, and short acronyms like
// "SLP" are badges, never section headings.
// ---------------------------------------------------------------------------

const PANEL_SOURCE_URL = new URL("../../mods/hotpot/main.js", import.meta.url);
const EMOJI = /\p{Extended_Pictographic}/u;
const BLOCK_BAR = /█/;
// A quoted string literal consisting solely of uppercase words, separators,
// and brackets, with at least one run of four or more uppercase letters.
const ALL_CAPS_HEADING = /["'`][A-Z0-9 ()&/↔→·-]*[A-Z]{4,}[A-Z0-9 ()&/↔→·-]*["'`]/;
const PLACEHOLDER_BOXES = [
  "No seats launched",
  "No Assignments in this Project yet",
  "Mailbox idle",
  "All quiet — no stalled permissions",
];

function panelHygieneOffenders(source) {
  const offenders = [];
  if (EMOJI.test(source)) offenders.push("an emoji character");
  if (BLOCK_BAR.test(source)) offenders.push('the "█" block character');
  const heading = source.match(ALL_CAPS_HEADING);
  if (heading) offenders.push(`the ALL-CAPS heading string ${heading[0]}`);
  for (const placeholder of PLACEHOLDER_BOXES) {
    if (source.includes(placeholder)) offenders.push(`the empty-placeholder text "${placeholder}"`);
  }
  return offenders;
}

test("the panel hygiene patterns bite (self-check on known-bad fixtures)", () => {
  assert.ok(EMOJI.test('div().child("🍲 Hotpot")'), "emoji soup prefix is emoji");
  assert.ok(EMOJI.test('"⚙"'), "the gear glyph is emoji");
  assert.ok(EMOJI.test('"PTY ↗"'), "the arrow suffix is emoji");
  assert.ok(BLOCK_BAR.test('"█".repeat(cells)'), "a block-glyph bar is a block bar");
  assert.ok(ALL_CAPS_HEADING.test('.child("NEEDS ATTENTION")'));
  assert.ok(ALL_CAPS_HEADING.test('.child("ASSIGNMENTS")'));
  assert.ok(ALL_CAPS_HEADING.test('.child("ROOM PRESETS")'));
  assert.ok(ALL_CAPS_HEADING.test('.child("LIVE MAILBOX (LEAD ↔ PEER)")'));
});

test("the panel hygiene patterns spare native chrome (self-check on known-good fixtures)", () => {
  assert.equal(EMOJI.test("├ asg_001 · gate green · 1/3"), false, "tree connectors are not emoji");
  assert.equal(EMOJI.test("└ lead → peer"), false, "the last-child connector is not emoji");
  assert.equal(EMOJI.test("⟳ rotate 150k"), false, "the rotation glyph is not emoji");
  assert.equal(ALL_CAPS_HEADING.test('.child("SLP")'), false, "a 3-letter acronym badge is not a heading");
  assert.equal(ALL_CAPS_HEADING.test('.child("Seats")'), false, "sentence-case headings are fine");
  assert.equal(ALL_CAPS_HEADING.test('.child("gate green · 1/3")'), false, "sentence-case badges are fine");
});

test("the right-hand panel carries no emoji, block bars, ALL-CAPS headings, or placeholder boxes", () => {
  const source = readFileSync(PANEL_SOURCE_URL, "utf8");
  assert.deepEqual(panelHygieneOffenders(source), []);
});

// ---------------------------------------------------------------------------
// Ticket 06: the panel's native design system at the rendering boundary —
// the geometric context bar (a 3px track with a 1px rotation tick), the
// native spinner, and vendor icons resolved through Herdr's `knownAgents()`
// registry (the `image()` branch renders untinted vendor icons; `svg()` the
// tinted ones). Text stand-ins are gone: no `BAR_CELLS` cell rendering, no
// braille spinner frames, no `●` text dots.
// ---------------------------------------------------------------------------

const NATIVE_CHROME_REQUIRED = [
  [".h(3)", "the 3px context-bar track"],
  [".w(1)", "the 1px rotation tick"],
  ["spinner.svg", "the native spinner glyph"],
  [".spin(", "the spinner's native spin (its 900ms cadence is pinned by the marker model tests)"],
  ["knownAgents(", "Herdr's agent registry"],
  ["image(", "the untinted vendor-icon branch"],
];

const NATIVE_CHROME_FORBIDDEN = [
  [/BAR_CELLS/, "the ASCII cell-bar constant"],
  [/⠋/, "a text spinner frame"],
  [/●/, "a text status dot"],
];

test("the panel renders the native chrome pieces the design system names", () => {
  const source = readFileSync(PANEL_SOURCE_URL, "utf8");
  const missing = NATIVE_CHROME_REQUIRED.filter(([needle]) => !source.includes(needle)).map(
    ([, label]) => label,
  );
  assert.deepEqual(missing, [], "every native chrome piece is rendered");
});

test("the panel leaves no text stand-ins for the native markers and bars", () => {
  const source = readFileSync(PANEL_SOURCE_URL, "utf8");
  const offenders = NATIVE_CHROME_FORBIDDEN.filter(([pattern]) => pattern.test(source)).map(
    ([, label]) => label,
  );
  assert.deepEqual(offenders, [], "no text stand-in survives");
});

// ---------------------------------------------------------------------------
// Ticket 07: the panel's three dispatch surfaces at the wiring boundary —
// inline attention Ack/Dismiss, the bottom Directive Composer, and the
// onboarding chip — all through `exec("hotpot", ...)` under `cx.spawn`, with
// busy states and a post-action refresh. The pure decision layer is covered by
// the node tests over `shared/protocol.js`; this pins that the mod actually
// wires it.
// ---------------------------------------------------------------------------

const DISPATCH_SURFACES_REQUIRED = [
  ["attentionFeedbackArgs(", "the Ack/Dismiss feedback argv builder"],
  ["attentionSeatTarget(", "the letter row-click seat selection"],
  ['"useful", "Ack"', "the inline Ack action"],
  ['"noisy", "Dismiss"', "the inline Dismiss action"],
  ["directiveDispatchSteps(", "the composer's launch-then-prompt sequence"],
  ["onboardingChipModel(", "the onboarding chip's visibility gate"],
  ["onboardingDispatchSteps(", "the onboarding chip's dispatch"],
  ["InputState.new(cx", "the composer's input state"],
  ['on("submit"', "Enter submits the composer"],
  ["cx.spawn(", "dispatches run under cx.spawn, never blocking render"],
  ["feedbackBusy", "the feedback busy state"],
  ["composerBusy", "the composer busy state"],
  ["onboardingBusy", "the onboarding busy state"],
  ['"Onboard project"', "the 1-click onboarding chip"],
  ['"Send"', "the composer's Send button"],
];

test("the panel wires all three dispatch surfaces through exec with busy states", () => {
  const source = readFileSync(PANEL_SOURCE_URL, "utf8");
  const missing = DISPATCH_SURFACES_REQUIRED.filter(([needle]) => !source.includes(needle)).map(
    ([, label]) => label,
  );
  assert.deepEqual(missing, [], "every dispatch surface is wired");
});

// ---------------------------------------------------------------------------
// Ticket 08: the Session Inspector's native redesign at the source-hygiene and
// wiring boundary — zero emoji characters and no "█" block bars across the
// inspector mod and both synced protocol.js copies (the shared file is the
// single source of truth, so an emoji anywhere in it surfaces in both), the
// forbidden-API scan over the pure protocol copies, and the native pieces the
// redesign names (vendor icons through Herdr's registry, native chevrons, the
// compact 28px tool-call row, the 3px context bar, the Terminal action).
// ---------------------------------------------------------------------------

const INSPECTOR_NATIVE_SOURCES = [
  "../../mods/hotpot-session/main.js",
  "../../mods/hotpot/protocol.js",
  "../../mods/hotpot-session/protocol.js",
];

function sourceHygieneOffenders(source) {
  const offenders = [];
  if (EMOJI.test(source)) offenders.push("an emoji character");
  if (BLOCK_BAR.test(source)) offenders.push('the "█" block character');
  return offenders;
}

// Known-bad fixtures the scan must flag over every covered source: the old
// inspector header eye, the old kind-icon emojis, the old expand/collapse
// glyphs, the keyboard prefix, and the ASCII block bar.
const HYGIENE_FIXTURES = [
  ['div().child(`👁 ${title}`)', "the old eye-prefixed header title"],
  ['return { icon: "💭", label: "Thinking" }', "an emoji kind icon"],
  ['child(isCollapsed ? "▶ Expand" : "▼ Collapse")', "the old text expand/collapse glyphs"],
  ['child("⌨ Raw PTY · Alt+T")', "the old keyboard-prefixed button"],
  ['"█".repeat(cells)', "a block-glyph bar"],
];

test("the inspector hygiene scan bites on every covered source (self-check on known-bad fixtures)", () => {
  for (const rel of INSPECTOR_NATIVE_SOURCES) {
    for (const [fixture, label] of HYGIENE_FIXTURES) {
      assert.ok(
        sourceHygieneOffenders(fixture).length > 0,
        `${rel}: the scan flags ${label}`,
      );
    }
  }
});

test("the session inspector and both synced protocol copies carry no emoji and no block bars", () => {
  for (const rel of INSPECTOR_NATIVE_SOURCES) {
    const source = readFileSync(new URL(rel, import.meta.url), "utf8");
    assert.deepEqual(sourceHygieneOffenders(source), [], rel);
  }
});

test("the synced protocol copies use no browser or Node timer globals", () => {
  for (const rel of ["../../mods/hotpot/protocol.js", "../../mods/hotpot-session/protocol.js"]) {
    const source = readFileSync(new URL(rel, import.meta.url), "utf8");
    const offenders = source
      .split("\n")
      .map((line, i) => [i + 1, line])
      .filter(([, line]) => FORBIDDEN.test(line));
    assert.deepEqual(
      offenders.map(([line]) => line),
      [],
      `${rel} stays a pure module with no DOM or timer globals`,
    );
  }
});

const INSPECTOR_NATIVE_REQUIRED = [
  ["knownAgents(", "Herdr's agent registry for the header vendor icon"],
  ["agentIconModel(", "the 06 icon-descriptor rule"],
  ["inspectorHeaderModel(", "the native header model (which owns the context bar)"],
  ["terminalAction", "the Terminal action model"],
  ["toolCallCollapsedModel(", "the tool-call collapse model"],
  ["defaultItemExpanded(", "the thinking expand-on-active default"],
  ["chevron-down.svg", "the expanded-row native chevron"],
  ["chevron-right.svg", "the collapsed-row native chevron"],
  [".h(28)", "the compact one-line tool-call row"],
  [".h(3)", "the 3px context-bar track"],
  ["terminal.svg", "the terminal glyph (fallback icon and button)"],
];

test("the session inspector renders the native redesign pieces the ticket names", () => {
  const source = readFileSync(new URL("../../mods/hotpot-session/main.js", import.meta.url), "utf8");
  const missing = INSPECTOR_NATIVE_REQUIRED.filter(([needle]) => !source.includes(needle)).map(
    ([, label]) => label,
  );
  assert.deepEqual(missing, [], "every native redesign piece is rendered");
});
