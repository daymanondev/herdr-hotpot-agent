// Node tests for the Session Inspector's pure view-model helpers in
// `shared/protocol.js` (ticket 14): the kind → icon/label mapping, default
// expansion of Thinking blocks given seat activity, and PTY-target resolution.
// These helpers are the mod's whole decision layer; the gpui-kit view itself is
// covered by `node --check` and the static contract scan.

import test from "node:test";
import assert from "node:assert/strict";
import * as protocol from "../../shared/protocol.js";

// ---------------------------------------------------------------------------
// Kind → icon-descriptor / label mapping (ticket 08: zero emoji characters)
// ---------------------------------------------------------------------------

test("kind mapping yields vector chrome icon descriptors for thinking and tool_call", () => {
  assert.deepEqual(protocol.transcriptKindMeta("thinking").icon, {
    kind: "chrome",
    icon: "mods/workspace-chrome/layers.svg",
    tint: true,
  });
  assert.deepEqual(protocol.transcriptKindMeta("tool_call").icon, {
    kind: "chrome",
    icon: "mods/workspace-chrome/terminal.svg",
    tint: true,
  });
});

test("notices carry no icon and messages carry the vendor-icon marker", () => {
  assert.equal(protocol.transcriptKindMeta("notice").icon, null);
  assert.deepEqual(protocol.transcriptKindMeta("message").icon, { kind: "vendor" });
  for (const kind of ["attention", "attention_notice", "assignment_notice", "mailbox"]) {
    assert.equal(protocol.transcriptKindMeta(kind).icon, null, `notice kind ${kind}`);
  }
});

test("kind metadata never carries an emoji character and labels every kind", () => {
  const EMOJI = /\p{Extended_Pictographic}/u;
  const kinds = [
    "thinking",
    "tool_call",
    "notice",
    "attention",
    "attention_notice",
    "assignment_notice",
    "mailbox",
    "message",
    "weird-unknown",
    "",
  ];
  for (const kind of kinds) {
    const meta = protocol.transcriptKindMeta(kind);
    assert.equal(EMOJI.test(JSON.stringify(meta)), false, `kind ${kind} has no emoji`);
    assert.equal(typeof meta.label, "string");
    assert.ok(meta.label.length > 0, `kind ${kind || "empty"} still renders a label`);
  }
});

// ---------------------------------------------------------------------------
// Default expansion of Thinking blocks given seat activity
// ---------------------------------------------------------------------------

test("a seat's turn is active while it is working, running, live, busy, or blocked", () => {
  for (const status of ["working", "running", "live", "busy", "blocked"]) {
    assert.equal(protocol.isSeatActive({ status }), true, `status ${status}`);
    assert.equal(protocol.isSeatActive(status), true, `bare status ${status}`);
  }
});

test("a seat's turn is not active when idle, offline, finished, or unknown", () => {
  for (const status of ["idle", "offline", "done", "finished", "exited", "", undefined]) {
    assert.equal(protocol.isSeatActive({ status }), false, `status ${status}`);
    assert.equal(protocol.isSeatActive(status), false, `bare status ${status}`);
  }
  assert.equal(protocol.isSeatActive(null), false);
});

test("thinking items expand by default only for a seat whose turn is active", () => {
  const thinking = { kind: "thinking", body: "long reasoning body" };
  assert.equal(protocol.defaultItemExpanded(thinking, true), true);
  assert.equal(protocol.defaultItemExpanded(thinking, false), false);
});

test("non-thinking items render expanded regardless of seat activity", () => {
  for (const kind of ["message", "tool_call", "notice"]) {
    assert.equal(protocol.defaultItemExpanded({ kind }, true), true, `active ${kind}`);
    assert.equal(protocol.defaultItemExpanded({ kind }, false), true, `idle ${kind}`);
  }
});

// ---------------------------------------------------------------------------
// PTY-target resolution
// ---------------------------------------------------------------------------

test("a seat with a terminal address resolves to that address", () => {
  assert.equal(protocol.ptyTarget({ terminal: "term-lead-8" }), "term-lead-8");
});

test("a seat without a terminal resolves to null", () => {
  assert.equal(protocol.ptyTarget({ terminal: null }), null);
  assert.equal(protocol.ptyTarget({ terminal: "" }), null);
  assert.equal(protocol.ptyTarget({}), null);
  assert.equal(protocol.ptyTarget(null), null);
  assert.equal(protocol.ptyTarget(undefined), null);
});

test("notice kinds are distinguishable from other inspect item kinds", () => {
  for (const kind of ["notice", "attention", "assignment_notice", "mailbox"]) {
    assert.equal(protocol.isNoticeKind(kind), true, `kind ${kind}`);
  }
  for (const kind of ["thinking", "tool_call", "message"]) {
    assert.equal(protocol.isNoticeKind(kind), false, `kind ${kind}`);
  }
});

test("live tool-call statuses color as working, not idle", () => {
  const t = { working: "#work", done: "#done", idle: "#idle" };
  assert.equal(protocol.statusColor(t, "running"), "#work");
  assert.equal(protocol.statusColor(t, "live"), "#work");
  assert.equal(protocol.statusColor(t, "done"), "#done");
});
