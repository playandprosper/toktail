// SPDX-License-Identifier: Apache-2.0
// Checks the portal's scripts without a browser: every file parses, and the
// pure helpers in common.js (number formats, dates, CSV escaping) behave.
//   node scripts/test_ui_data.cjs
"use strict";
const fs = require("fs");
const path = require("path");
const vm = require("vm");
const assert = require("assert");

const dir = path.join(__dirname, "..", "assets");
const scripts = fs.readdirSync(dir).filter((f) => f.endsWith(".js"));
for (const f of scripts) {
  try { new vm.Script(fs.readFileSync(path.join(dir, f), "utf8"), { filename: f }); }
  catch (e) { console.error("✗ " + f + ": " + e.message); process.exit(1); }
}
console.log("✓ " + scripts.length + " portal scripts parse");

// The page loads these in this order; every global they use must exist.
const html = fs.readFileSync(path.join(dir, "index.html"), "utf8");
const order = [...html.matchAll(/<script src="\/([\w-]+\.js)"><\/script>/g)].map((m) => m[1]);
for (const f of scripts) assert.ok(order.includes(f), f + " is never loaded by index.html");
const served = fs.readFileSync(path.join(__dirname, "..", "src", "portal.rs"), "utf8");
for (const f of order) assert.ok(served.includes('"/' + f + '"'), f + " isn't served by portal.rs");
console.log("✓ index.html loads every script and portal.rs serves them");

// common.js in a tiny fake page.
const el = () => ({ style: {}, dataset: {}, append() {}, addEventListener() {}, setAttribute() {}, classList: { add() {}, remove() {}, toggle() {} } });
const sandbox = {
  console, Intl, Math, Date, Number, String, JSON, Map, Set, BigInt, URL, URLSearchParams, setTimeout, clearTimeout,
  document: { getElementById: () => el(), documentElement: { dataset: {} }, createElement: el, createElementNS: el, createTextNode: (t) => ({ t }) },
  localStorage: { getItem: () => null, setItem() {} },
  getComputedStyle: () => ({ getPropertyValue: () => "" }),
  matchMedia: () => ({ matches: false, addEventListener() {} }),
  Pip: { reduceMotion: { matches: false } },
  performance: { now: () => 0 }, requestAnimationFrame: () => 0,
};
vm.createContext(sandbox);
vm.runInContext(fs.readFileSync(path.join(dir, "common.js"), "utf8") + "\n;globalThis.__t = { compact, words, usd, csvEscape, addDays, dayLabel, monthLabel, pct, mv };", sandbox);
const t = sandbox.__t;
assert.strictEqual(t.compact(999), "999");
assert.strictEqual(t.compact(1234), "1.23K");
assert.strictEqual(t.compact(17e12), "17T");
assert.strictEqual(t.words(17e12), "17 trillion");
assert.strictEqual(t.words(257265583), "257 million");
assert.strictEqual(t.usd(0), "$0");
assert.strictEqual(t.usd(0.004), "<$0.01");
assert.strictEqual(t.csvEscape('a,"b"'), '"a,""b"""');
assert.strictEqual(t.addDays("2024-02-28", 1), "2024-02-29");
assert.strictEqual(t.addDays("2026-01-01", -1), "2025-12-31");
assert.strictEqual(t.monthLabel("2026-10"), "Oct 2026");
assert.strictEqual(t.mv({ input: 3, output: 4 }, "combined"), 7);
assert.strictEqual(t.mv({ input: 3, output: 4 }, "output"), 4);
console.log("✓ formatting, dates and CSV helpers");
