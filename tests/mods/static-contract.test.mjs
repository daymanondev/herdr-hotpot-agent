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
