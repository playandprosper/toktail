// SPDX-License-Identifier: Apache-2.0
// Toktail portal: the command palette (⌘K / Ctrl-K) and keyboard shortcuts.
// Jump to any page, host, model or setting, change the range, or type a
// question and hand it to Pip.
"use strict";

const Palette = (() => {
  let items = [], shown = [], cursor = 0;
  const dlg = () => $("palette");

  function go(hash) { location.hash = hash; }
  function setRange(r) { S.range = r; store.set("range", r); go("#/dashboard"); Dashboard.load(); }
  function focusHost(id) { S.hosts = id ? new Set([id]) : null; store.set("hosts", id || ""); go("#/dashboard"); Dashboard.load(); }

  function build() {
    const list = [];
    const add = (group, icon, label, run, hint) => list.push({ group, icon, label, run, hint: hint || "" });
    add("Pages", "◆", "Dashboard", () => go("#/dashboard"), "g d");
    add("Pages", "✎", "Prompts: saved prompts and responses", () => go("#/prompts"), "g p");
    add("Pages", "🦊", "Chat with Pip (full page)", () => go("#/prompts/chat"), "g c");
    add("Pages", "⚡", "Models: speed, wait and reliability", () => go("#/models"), "g m");
    add("Pages", "$", "Savings: hardware and lifecycle", () => go("#/savings"), "g s");
    add("Pages", "⚙", "Settings", () => go("#/settings"), "g ,");
    for (const [id, name] of [["hosts", "Hosts"], ["capture", "Saving prompts"], ["encryption", "Encryption"], ["s3", "S3 mirror"], ["budgets", "Budgets and alerts"], ["pip", "Pip and the menu bar"], ["costs", "Cost comparison"], ["hardware", "Hardware"], ["api", "Metrics and API"]])
      add("Settings", "⚙", "Settings → " + name, () => go("#/settings/" + id));
    for (const [r, name] of [["today", "Today"], ["yesterday", "Yesterday"], ["7", "Last 7 days"], ["30", "Last 30 days"], ["90", "Last 90 days"], ["mtd", "This month"], ["all", "All time"]])
      add("Range", "◷", "Show " + name.toLowerCase(), () => setRange(r));
    if (S.meta) {
      add("Hosts", "●", "All hosts", () => focusHost(null));
      for (const x of S.meta.hosts.filter((y) => !y.removed)) add("Hosts", "●", "Only " + x.name, () => focusHost(x.id), x.id);
      for (const m of S.meta.models.slice(0, 60)) add("Models", "▲", m.model, () => go("#/models/" + encodeURIComponent(m.model)), compact(m.combined) + " tokens");
    }
    add("Actions", "◐", isDark() ? "Switch to light mode" : "Switch to dark mode", () => $("btn-theme").click(), "t");
    add("Actions", "↺", "Replay the trail", () => { go("#/dashboard"); setTimeout(() => $("btn-replay").click(), 300); }, "r");
    add("Actions", "⇩", "Download the raw CSV for this range", () => $("btn-raw-csv").click());
    add("Actions", "⌂", "Open Toktail's data folder", () => $("btn-data-folder").click());
    if (S.meta && S.meta.s3.enabled) add("Actions", "☁", "Sync to S3 now", () => $("btn-sync").click());
    add("Actions", "🔔", "Show alerts", () => Main.toggleAlerts(true));
    add("Actions", "?", "Keyboard shortcuts", shortcuts, "?");
    return list;
  }

  /** Subsequence match with a bonus for word starts and runs. */
  function score(q, text) {
    if (!q) return 1;
    const t = text.toLowerCase();
    const i = t.indexOf(q);
    if (i >= 0) return 100 - i + (i === 0 || /\W/.test(t[i - 1]) ? 30 : 0);
    let pos = 0, s = 0, run = 0;
    for (const ch of q) {
      const j = t.indexOf(ch, pos);
      if (j < 0) return 0;
      run = j === pos ? run + 1 : 0;
      s += 1 + run * 2 + (j === 0 || /\W/.test(t[j - 1]) ? 3 : 0);
      pos = j + 1;
    }
    return s;
  }

  function filter() {
    const raw = $("palette-input").value.trim();
    const q = raw.toLowerCase();
    shown = items.map((it) => ({ it, s: score(q, it.label + " " + it.group) })).filter((x) => x.s > 0).sort((a, b) => b.s - a.s).slice(0, 40).map((x) => x.it);
    if (!q) shown = items.filter((it) => ["Pages", "Range", "Actions"].includes(it.group)).slice(0, 18);
    if (raw) {
      shown.unshift({ group: "Ask", icon: "🦊", label: "Ask Pip: " + raw, run: () => PipChat.open(raw), hint: "↵" });
      shown.push({ group: "Search", icon: "⌕", label: "Search saved prompts for “" + raw + "”", run: () => { go("#/prompts"); setTimeout(() => { const s = $("rec-search"); if (s) { s.value = raw; s.dispatchEvent(new Event("input")); } }, 250); } });
    }
    cursor = 0;
    paint();
  }

  function paint() {
    const ul = $("palette-list");
    ul.textContent = "";
    let group = null;
    shown.forEach((it, i) => {
      if (it.group !== group) { group = it.group; ul.append(h("li", { class: "sep", role: "presentation", text: group })); }
      const li = h("li", { role: "option", "aria-selected": String(i === cursor), "data-i": i }, h("span", { class: "ic", text: it.icon, "aria-hidden": "true" }), h("span", { text: it.label }), h("small", { text: it.hint }));
      li.addEventListener("mousemove", () => { if (cursor !== i) { cursor = i; mark(); } });
      li.addEventListener("click", () => choose(i));
      ul.append(li);
    });
    if (!shown.length) ul.append(h("li", { class: "sep", text: "Nothing matches" }));
  }
  function mark() {
    for (const li of $("palette-list").querySelectorAll("li[data-i]")) {
      const on = +li.dataset.i === cursor;
      li.setAttribute("aria-selected", String(on));
      if (on) li.scrollIntoView({ block: "nearest" });
    }
  }
  function choose(i) {
    const it = shown[i];
    if (!it) return;
    close();
    setTimeout(it.run, 10);
  }

  function open() {
    if (dlg().open) return;
    items = build();
    $("palette-input").value = "";
    filter();
    dlg().showModal();
    $("palette-input").focus();
  }
  function close() { if (dlg().open) dlg().close(); }

  function shortcuts() {
    const rows = [["⌘K", "Jump anywhere, or ask Pip"], ["/", "Ask Pip"], ["g d", "Dashboard"], ["g p", "Prompts"], ["g c", "Chat"], ["g m", "Models"], ["g s", "Savings"], ["g ,", "Settings"], ["t", "Light or dark mode"], ["r", "Replay the trail"], ["Esc", "Close what's open"], ["?", "This list"]];
    const grid = h("div", { class: "shortcuts" });
    for (const [k, d] of rows) grid.append(h("span", {}, ...k.split(" ").map((x) => h("kbd", { text: x }))), span(d, ""));
    modal("Keyboard shortcuts", grid, [{ label: "Got it", value: true, cls: "primary" }]);
  }

  let gPending = 0;
  function typing(e) {
    const t = e.target;
    return t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName));
  }
  function onKey(e) {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") { e.preventDefault(); dlg().open ? close() : open(); return; }
    if (dlg().open) {
      if (e.key === "ArrowDown") { e.preventDefault(); cursor = Math.min(shown.length - 1, cursor + 1); mark(); }
      else if (e.key === "ArrowUp") { e.preventDefault(); cursor = Math.max(0, cursor - 1); mark(); }
      else if (e.key === "Enter") { e.preventDefault(); choose(cursor); }
      return;
    }
    if (typing(e) || e.metaKey || e.ctrlKey || e.altKey || document.querySelector("dialog[open]")) return;
    const now = performance.now();
    if (gPending && now - gPending < 900) {
      gPending = 0;
      const map = { d: "#/dashboard", p: "#/prompts", c: "#/prompts/chat", m: "#/models", s: "#/savings", ",": "#/settings" };
      if (map[e.key]) { e.preventDefault(); go(map[e.key]); }
      return;
    }
    if (e.key === "g") { gPending = now; return; }
    if (e.key === "/") { e.preventDefault(); PipChat.open(); }
    else if (e.key === "?") { e.preventDefault(); shortcuts(); }
    else if (e.key === "t") $("btn-theme").click();
    else if (e.key === "r" && S.page === "dashboard") $("btn-replay").click();
  }

  function init() {
    document.addEventListener("keydown", onKey);
    $("palette-input").addEventListener("input", filter);
    $("palette").addEventListener("click", (e) => { if (e.target === dlg()) close(); });
    $("palette-form").addEventListener("submit", (e) => { e.preventDefault(); choose(cursor); });
    $("btn-palette").addEventListener("click", open);
    if (/Win|Linux/.test(navigator.platform)) { const k = $("btn-palette").querySelector("kbd"); if (k) k.textContent = "Ctrl K"; }
  }
  return { init, open, close, shortcuts };
})();

