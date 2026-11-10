// Node tests for the space/exec plumbing shared by both mods in
// `shared/protocol.js` (review follow-up on tickets 13/14): the active-Space
// pick, the workspace-folder fallback, the standard `hotpot` exec options, and
// the transcript parent directory the Session Inspector's `watchFolder`
// subscribes to. Pure helpers — no herdr import, so they run under node --test.

import test from "node:test";
import assert from "node:assert/strict";
import * as protocol from "../../shared/protocol.js";

// ---------------------------------------------------------------------------
// activeSpace
// ---------------------------------------------------------------------------

test("activeSpace prefers the active space, else the first, else null", () => {
  const a = { name: "a", active: false };
  const b = { name: "b", active: true };
  assert.equal(protocol.activeSpace([a, b]), b);
  assert.equal(protocol.activeSpace([a]), a, "no active space falls back to the first");
  assert.equal(protocol.activeSpace([]), null);
  assert.equal(protocol.activeSpace(null), null, "a missing listing is not a crash");
  assert.equal(protocol.activeSpace(undefined), null);
});

// ---------------------------------------------------------------------------
// spaceFolder
// ---------------------------------------------------------------------------

test("spaceFolder uses the space's folder, else the caller's fallback", () => {
  assert.equal(protocol.spaceFolder({ folder: "/repo" }, "/home/fallback"), "/repo");
  assert.equal(protocol.spaceFolder({ folder: "" }, "/home/fallback"), "/home/fallback");
  assert.equal(protocol.spaceFolder({}, "/home/fallback"), "/home/fallback");
  assert.equal(protocol.spaceFolder(null, "/home/fallback"), "/home/fallback");
});

// ---------------------------------------------------------------------------
// hotpotExecOptions
// ---------------------------------------------------------------------------

test("hotpotExecOptions shapes the exec call both mods make", () => {
  assert.deepEqual(protocol.hotpotExecOptions({ machine: "mac-2" }, "/repo"), {
    cwd: "/repo",
    on: "mac-2",
    timeout: 5000,
  });
  assert.deepEqual(protocol.hotpotExecOptions(null, "/repo"), {
    cwd: "/repo",
    on: null,
    timeout: 5000,
  }, "a missing space machines no host");
  assert.equal(protocol.hotpotExecOptions({}, "/repo", 250).timeout, 250);
});

// ---------------------------------------------------------------------------
// parentDir (the Session Inspector's watchFolder target)
// ---------------------------------------------------------------------------

test("parentDir returns the directory of a transcript path", () => {
  assert.equal(
    protocol.parentDir("/home/.hotpot/roles/claude/lead/projects/sess-live.jsonl"),
    "/home/.hotpot/roles/claude/lead/projects",
  );
  assert.equal(protocol.parentDir("dir/sess.jsonl"), "dir");
  assert.equal(protocol.parentDir("C:\\hotpot\\sess.jsonl"), "C:\\hotpot", "windows separators too");
});

test("parentDir yields null for paths without a watchable parent", () => {
  assert.equal(protocol.parentDir(null), null);
  assert.equal(protocol.parentDir(undefined), null);
  assert.equal(protocol.parentDir(""), null);
  assert.equal(protocol.parentDir("sess.jsonl"), null, "bare filename has no parent dir");
  assert.equal(protocol.parentDir("/sess.jsonl"), null, "filesystem root is not a watch target");
});
