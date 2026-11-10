// Pill pickers, preset profiles, and the profile argv builder (ticket 13,
// acceptance criteria 4 + 5).
//
// The panel's inline `agent/model · thinking` pills offer exactly the verified
// agent families (`src/runtime/mod.rs: AGENT_FAMILIES`) with concrete model
// options and the `low|medium|high` thinking efforts (`ultra`/`ultracode` are
// refused, spec user story 19). The named room presets apply a coherent
// per-role map, and every change persists through the minimal
// `hotpot profile set` write path whose argv this builder produces.

import test from "node:test";
import assert from "node:assert/strict";

import {
  AGENT_OPTIONS,
  MODEL_OPTIONS,
  THINKING_OPTIONS,
  PRESET_NAMES,
  PRESET_PROFILES,
  profileSetArgs,
} from "../../shared/protocol.js";

test("the pill's thinking option list is exactly low, medium, high", () => {
  assert.deepEqual(THINKING_OPTIONS, ["low", "medium", "high"]);
  assert.equal(THINKING_OPTIONS.includes("ultra"), false);
  assert.equal(THINKING_OPTIONS.includes("ultracode"), false);
});

test("the pill's agent options are the verified families, each with model options", () => {
  assert.deepEqual(AGENT_OPTIONS, ["codex", "claude", "pi"]);
  for (const agent of AGENT_OPTIONS) {
    const models = MODEL_OPTIONS[agent];
    assert.ok(Array.isArray(models) && models.length > 0, `${agent} offers model options`);
    for (const model of models) {
      assert.equal(typeof model, "string", `${agent} model ${model} is a plain name`);
    }
  }
});

test("the named presets are Deep Architecture, Fast Iteration, and Budget", () => {
  assert.deepEqual(PRESET_NAMES, ["Deep Architecture", "Fast Iteration", "Budget"]);
});

test("every preset maps to a concrete per-role agent/model/thinking payload", () => {
  for (const name of PRESET_NAMES) {
    const preset = PRESET_PROFILES[name];
    for (const role of ["supervisor", "lead", "peer"]) {
      const payload = preset?.[role];
      assert.ok(payload, `${name} covers ${role}`);
      assert.ok(AGENT_OPTIONS.includes(payload.agent), `${name}/${role} agent is a verified family`);
      assert.ok(
        MODEL_OPTIONS[payload.agent].includes(payload.model),
        `${name}/${role} model is one of the ${payload.agent} pill options`,
      );
      assert.ok(
        THINKING_OPTIONS.includes(payload.thinking),
        `${name}/${role} thinking is a permitted effort`,
      );
    }
  }
});

test("the presets are coherent: Deep Architecture thinks high, Budget thinks low", () => {
  assert.equal(PRESET_PROFILES["Deep Architecture"].lead.thinking, "high");
  assert.equal(PRESET_PROFILES["Deep Architecture"].lead.model, "gpt-5.1-codex");
  assert.equal(PRESET_PROFILES.Budget.lead.thinking, "low");
});

test("the argv builder produces the expected hotpot profile set invocation", () => {
  assert.deepEqual(
    profileSetArgs("codex", "lead", "gpt-5.1-codex", "high"),
    ["profile", "set", "codex", "lead", "gpt-5.1-codex", "--thinking", "high"],
  );
});

test("the argv builder refuses ultra/ultracode and unknown efforts before exec", () => {
  for (const effort of ["ultra", "ultracode", "MAXIMUM", ""]) {
    assert.throws(() => profileSetArgs("claude", "peer", "claude-haiku-4-5", effort), undefined, `refuses ${effort}`);
  }
});

test("the argv builder refuses unknown roles and agent families", () => {
  assert.throws(() => profileSetArgs("claude", "owner", "claude-haiku-4-5", "low"));
  assert.throws(() => profileSetArgs("zcode", "lead", "whatever", "low"));
});
