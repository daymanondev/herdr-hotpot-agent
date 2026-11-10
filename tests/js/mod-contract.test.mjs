// Static mod-contract test (ticket 13, acceptance criterion 6): Herdr mods are
// gpui-kit JavaScript only — no timers (`setTimeout`/`setInterval`; the mods use
// `cx.timer`), and no DOM globals (`document.`, `window.`). This scan fails the
// check suite the moment one leaks into any `mods/**/*.js` source.

import test from "node:test";
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const MODS_ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..", "mods");
const FORBIDDEN = /setTimeout|setInterval|document\.|window\./;

function jsFilesUnder(dir) {
  const files = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      files.push(...jsFilesUnder(path));
    } else if (entry.isFile() && entry.name.endsWith(".js")) {
      files.push(path);
    }
  }
  return files;
}

test("the forbidden-API pattern actually bites (self-check on known-bad samples)", () => {
  assert.ok(FORBIDDEN.test("setTimeout(() => {}, 0);"));
  assert.ok(FORBIDDEN.test("setInterval(poll, 1000);"));
  assert.ok(FORBIDDEN.test("document.querySelector('#x');"));
  assert.ok(FORBIDDEN.test("window.location.href;"));
  assert.equal(FORBIDDEN.test("cx.timer.every(2500, refresh);"), false);
});

test("the scanner reaches the real mod sources (self-check, not zero files)", () => {
  const files = jsFilesUnder(MODS_ROOT);
  assert.ok(files.length >= 4, `expected the hotpot mods' .js files, found ${files.length}`);
  const names = files.map((f) => f.split("/").slice(-2).join("/"));
  assert.ok(names.includes("hotpot/main.js"), `hotpot/main.js scanned: ${names.join(", ")}`);
});

test("no forbidden APIs appear anywhere under mods/**/*.js", () => {
  const offenders = jsFilesUnder(MODS_ROOT)
    .map((path) => ({ path, source: readFileSync(path, "utf8") }))
    .filter(({ source }) => FORBIDDEN.test(source))
    .map(({ path, source }) => {
      const line = source.split("\n").findIndex((l) => FORBIDDEN.test(l)) + 1;
      return `${path}:${line}`;
    });
  assert.deepEqual(offenders, [], "mods must use cx.timer and gpui-kit only");
});
