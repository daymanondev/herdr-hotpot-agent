// Directive Composer & onboarding chip (ticket nui/07, acceptance criteria 2
// and 3): the pure argv builder behind the composer's Send (`hotpot seat
// launch ... --prompt <text>`, the CLI ticket nui/04 added), its launch-then-
// prompt exec sequence, and the onboarding chip's visibility model over the
// `hotpot status` snapshot. The exec'd CLI behavior is nui/04's Rust seam; no
// duplication here.

import test from "node:test";
import assert from "node:assert/strict";

import {
  directiveDispatchArgs,
  directiveDispatchSteps,
  onboardingChipModel,
  onboardingDispatchSteps,
  ONBOARDING_DIRECTIVE_TEXT,
  SETUP_APPLY_TIMEOUT_MS,
} from "../../shared/protocol.js";

// ---------------------------------------------------------------------------
// The dispatch argv builder
// ---------------------------------------------------------------------------

test("a directive to Lead builds the seat launch argv with the prompt flag", () => {
  assert.deepEqual(
    directiveDispatchArgs("lead", "codex", "/repos/demo", "Draft the release plan"),
    [
      "seat",
      "launch",
      "lead",
      "--agent",
      "codex",
      "--project",
      "/repos/demo",
      "--prompt",
      "Draft the release plan",
    ],
  );
});

test("the composer targets Supervisor the same way", () => {
  assert.deepEqual(
    directiveDispatchArgs("supervisor", "claude", "/repos/demo", "Watch the room"),
    [
      "seat",
      "launch",
      "supervisor",
      "--agent",
      "claude",
      "--project",
      "/repos/demo",
      "--prompt",
      "Watch the room",
    ],
  );
});

test("the target defaults to lead when no role is given", () => {
  assert.deepEqual(directiveDispatchArgs(undefined, "codex", "/repos/demo", "Go"), [
    "seat",
    "launch",
    "lead",
    "--agent",
    "codex",
    "--project",
    "/repos/demo",
    "--prompt",
    "Go",
  ]);
});

test("roles beyond Lead and Supervisor are refused — the composer addresses 1:1 seats only", () => {
  assert.throws(() => directiveDispatchArgs("peer", "codex", "/repos/demo", "Go"));
  assert.throws(() => directiveDispatchArgs("owner", "codex", "/repos/demo", "Go"));
});

test("empty directive text throws before any exec", () => {
  for (const text of ["", "   ", "\t\n", null, undefined]) {
    assert.throws(
      () => directiveDispatchArgs("lead", "codex", "/repos/demo", text),
      undefined,
      `text ${JSON.stringify(text)} is refused`,
    );
  }
});

test("unknown agent families and empty folders throw before any exec", () => {
  assert.throws(() => directiveDispatchArgs("lead", "zcode", "/repos/demo", "Go"));
  assert.throws(() => directiveDispatchArgs("lead", "codex", "", "Go"));
  assert.throws(() => directiveDispatchArgs("lead", "codex", null, "Go"));
});

// ---------------------------------------------------------------------------
// The launch-then-prompt exec sequence
// ---------------------------------------------------------------------------

test("an unprovisioned Project provisions first, then dispatches the directive", () => {
  const steps = directiveDispatchSteps(
    { provisioned: false },
    "lead",
    "codex",
    "/repos/demo",
    "Go",
  );
  assert.equal(steps.length, 2);
  assert.deepEqual(steps[0].args, ["setup", "--apply"]);
  assert.equal(steps[0].timeoutMs, SETUP_APPLY_TIMEOUT_MS);
  assert.deepEqual(steps[1].args, directiveDispatchArgs("lead", "codex", "/repos/demo", "Go"));
  assert.equal(steps[1].timeoutMs, null);
});

test("a provisioned Project dispatches the directive in one step", () => {
  const steps = directiveDispatchSteps(
    { provisioned: true },
    "supervisor",
    "claude",
    "/repos/demo",
    "Watch the room",
  );
  assert.equal(steps.length, 1);
  assert.deepEqual(steps[0].args, [
    "seat",
    "launch",
    "supervisor",
    "--agent",
    "claude",
    "--project",
    "/repos/demo",
    "--prompt",
    "Watch the room",
  ]);
});

// ---------------------------------------------------------------------------
// The onboarding chip
// ---------------------------------------------------------------------------

test("the chip is visible exactly while the Project lacks WORKSPACE_PROTOCOL.md", () => {
  assert.equal(onboardingChipModel({ hasWorkspaceProtocol: false }).visible, true);
  assert.equal(onboardingChipModel({ hasWorkspaceProtocol: true }).visible, false);
});

test("a snapshot that predates the flag stays hidden — the chip never guesses", () => {
  assert.equal(onboardingChipModel({}).visible, false);
  assert.equal(onboardingChipModel(null).visible, false);
});

test("the onboarding directive names the hotpot-project-onboarding skill", () => {
  assert.ok(ONBOARDING_DIRECTIVE_TEXT.includes("hotpot-project-onboarding"));
  assert.ok(ONBOARDING_DIRECTIVE_TEXT.includes("WORKSPACE_PROTOCOL.md"));
});

test("the chip dispatches the same launch-then-prompt flow targeting Lead", () => {
  const steps = onboardingDispatchSteps({ provisioned: true }, "codex", "/repos/demo");
  assert.equal(steps.length, 1);
  assert.deepEqual(
    steps[0].args,
    directiveDispatchArgs("lead", "codex", "/repos/demo", ONBOARDING_DIRECTIVE_TEXT),
  );
});

test("the chip provisions first on an unprovisioned Project", () => {
  const steps = onboardingDispatchSteps({ provisioned: false }, "codex", "/repos/demo");
  assert.equal(steps.length, 2);
  assert.deepEqual(steps[0].args, ["setup", "--apply"]);
  assert.equal(steps[0].timeoutMs, SETUP_APPLY_TIMEOUT_MS);
});
