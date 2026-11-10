// Attention-First view model tests (ticket 13, acceptance criterion 2).
//
// The helpers under test are the pure view-model layer of `shared/protocol.js`:
// they consume the `hotpot status` snapshot contract (ticket 12) and produce the
// ordering / quiet flag the right-hand panel renders. Fixtures mirror the letter
// JSON the Rust engine serves (`src/runtime/attention.rs`: `key`, `kind`,
// `text`, optional `downranked`).

import test from "node:test";
import assert from "node:assert/strict";

import { attentionView } from "../../shared/protocol.js";

// A scrambled fixture: every letter kind the panel ranks, given out of order so
// only a real sort produces the spec's ranking.
const LETTERS = [
  {
    key: "att_unread_handback_asg_001",
    kind: "unread-handback",
    text: "[hotpot attention att_unread_handback_asg_001] peer handback on asg_001 awaits Lead review",
    assignment: "asg_001",
  },
  {
    key: "att_stalled_permission_perm_9",
    kind: "stalled-permission",
    text: "[hotpot attention att_stalled_permission_perm_9] permission `perm_9` awaiting a decision",
    permission: "perm_9",
  },
  {
    key: "att_incident_seq6",
    kind: "incident",
    text: "[hotpot attention att_incident_seq6] Lead raised INCIDENT: registry down",
    marker: "INCIDENT:",
  },
  {
    key: "att_needs_human_seq5",
    kind: "needs-human",
    text: "[hotpot attention att_needs_human_seq5] Lead raised NEEDS-HUMAN: sign off the release",
    marker: "NEEDS-HUMAN:",
  },
];

test("a quiet snapshot yields quiet:true with zero exception-colored items", () => {
  for (const quietSnapshot of [{ attention: [] }, {}, null, undefined]) {
    const view = attentionView(quietSnapshot);
    assert.equal(view.quiet, true, `quiet for ${JSON.stringify(quietSnapshot)}`);
    assert.deepEqual(view.items, [], "no items render, so nothing gets warning colors");
  }
});

test("letters order NEEDS-HUMAN / INCIDENT above stalled-permission above unread-handback", () => {
  const view = attentionView({ attention: LETTERS });
  assert.equal(view.quiet, false);
  assert.deepEqual(
    view.items.map((item) => item.key),
    [
      "att_needs_human_seq5",
      "att_incident_seq6",
      "att_stalled_permission_perm_9",
      "att_unread_handback_asg_001",
    ],
  );
});

test("every surfaced item is keyed by its att_<id> identity", () => {
  const view = attentionView({ attention: LETTERS });
  for (const item of view.items) {
    assert.match(item.key, /^att_.+/, "attention keys carry the att_ prefix");
  }
});

test("only exception items are flagged for warning colors; downranked letters stay neutral", () => {
  const downranked = { ...LETTERS[0], downranked: true };
  const view = attentionView({ attention: [LETTERS[1], downranked] });
  const byKey = Object.fromEntries(view.items.map((item) => [item.key, item]));
  assert.equal(byKey.att_stalled_permission_perm_9.exception, true);
  assert.equal(byKey.att_unread_handback_asg_001.exception, false, "downranked letters render neutral");
});
