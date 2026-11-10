// JSON assertions for the Hotpot layout keymap (ticket 14): the `Alt+T`
// binding that jumps from the Session Inspector into the selected seat's raw
// `hc` PTY terminal.
//
// Herdr binds keys in the app's `keys.json` (sections `global`,
// `layout:<name>`, `terminal`, `mod:<name>`); layout files in `layouts/` are
// structural node trees with no key bindings. The Hotpot layout's keymap is
// therefore this repo's `keys.json` under `layout:hotpot` — the section that
// is active while `layouts/hotpot.json` shows — and the binding routes to the
// PTY-focus action `hotpot-session:pty-focus`, delivered to the mod's
// `nextCommand()` key handling.

import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const readJson = (path) => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8"));

test("the hotpot layout parses and hosts the hotpot-session inspector block", () => {
  const layout = readJson("../../layouts/hotpot.json");
  assert.equal(layout.id, "root");
  const stack = layout.children
    .flatMap((child) => child.children || [])
    .find((child) => child.id === "work-body");
  const inspector = stack?.children?.find((child) => child.id === "main-stack")
    ?.children?.find((block) => block.id === "inspector");
  assert.equal(inspector?.content?.mod, "hotpot-session");
});

test("the hotpot keymap binds alt-t to the hotpot-session PTY-focus action", () => {
  const keys = readJson("../../keys.json");
  const section = keys["layout:hotpot"];
  assert.ok(section, "keys.json has a layout:hotpot section");
  assert.equal(section["alt-t"], "hotpot-session:pty-focus");
});
