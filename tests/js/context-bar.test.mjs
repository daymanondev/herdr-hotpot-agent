// Segmented context-bar model tests (ticket 13, acceptance criterion 3).
//
// The seat cards replace the single token badge with a segmented
// `Cache Read` / `Fresh Input` / `Output` bar plus a rotation-tick marker.
// The model under test derives everything from the `context` object of a
// `hotpot status` seat (ticket 12: `{cacheRead, freshInput, output, total,
// rotationTick}`) — segments sum to the total, and the tick fires only at or
// above the 150k rotation mark (`src/runtime/attention.rs:
// ROTATION_MARK_TOKENS`).

import test from "node:test";
import assert from "node:assert/strict";

import { contextBarModel, ROTATION_MARK_TOKENS } from "../../shared/protocol.js";

test("the rotation mark constant mirrors the engine's 150k mark", () => {
  assert.equal(ROTATION_MARK_TOKENS, 150000);
});

test("segments computed from cacheRead/freshInput/output sum to the total", () => {
  const bar = contextBarModel({ cacheRead: 100000, freshInput: 40000, output: 5000 });
  assert.equal(bar.total, 145000);
  const byKind = Object.fromEntries(bar.segments.map((segment) => [segment.kind, segment]));
  assert.equal(byKind.cacheRead.value, 100000);
  assert.equal(byKind.freshInput.value, 40000);
  assert.equal(byKind.output.value, 5000);
  assert.equal(
    bar.segments.reduce((sum, segment) => sum + segment.value, 0),
    bar.total,
    "the three segments are exactly the total",
  );
  assert.equal(bar.total, byKind.cacheRead.value + byKind.freshInput.value + byKind.output.value);
});

test("segment fractions partition one bar (and stay zero on an empty seat)", () => {
  const bar = contextBarModel({ cacheRead: 60000, freshInput: 30000, output: 30000 });
  const fractionSum = bar.segments.reduce((sum, segment) => sum + segment.fraction, 0);
  assert.ok(Math.abs(fractionSum - 1) < 1e-9, `fractions sum to 1, got ${fractionSum}`);

  const empty = contextBarModel({ cacheRead: 0, freshInput: 0, output: 0 });
  assert.equal(empty.total, 0);
  for (const segment of empty.segments) {
    assert.equal(segment.fraction, 0, "no divide-by-zero on an idle seat");
  }
});

test("the rotation tick is set at 150k and not at 149k", () => {
  const at150k = contextBarModel({ cacheRead: 90000, freshInput: 50000, output: 10000 });
  assert.equal(at150k.total, 150000);
  assert.equal(at150k.rotationTick, true, "150k sits at the mark, so it ticks");

  const at149k = contextBarModel({ cacheRead: 90000, freshInput: 49000, output: 10000 });
  assert.equal(at149k.total, 149000);
  assert.equal(at149k.rotationTick, false, "149k sits under the mark, so it does not tick");
});

test("a seat with no usage events renders an empty, unticked bar", () => {
  for (const missing of [undefined, null, {}]) {
    const bar = contextBarModel(missing);
    assert.equal(bar.total, 0);
    assert.equal(bar.rotationTick, false);
    assert.equal(bar.segments.length, 3);
  }
});

test("segment labels are the spec's Cache Read / Fresh Input / Output", () => {
  const bar = contextBarModel({ cacheRead: 1, freshInput: 2, output: 3 });
  assert.deepEqual(
    bar.segments.map((segment) => segment.label),
    ["Cache Read", "Fresh Input", "Output"],
  );
});
