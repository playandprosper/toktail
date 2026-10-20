// SPDX-License-Identifier: Apache-2.0
// Toktail portal: chatting with your own models, two ways.
//   Pip     Pip answers with one of your local models, using exact numbers
//           Toktail works out from your CSV (facts) and read-only tools it
//           can call (steps). Every number Pip quotes is in a card you can
//           open. Your system prompt, if you set one, replaces Pip's persona.
//   Direct  Only your messages and your system prompt go to the model,
//           nothing added. "Continue in chat" on a saved record uses this.
// One conversation is shared by the corner panel and Prompts → Chat, and is
// kept in this browser so a refresh doesn't lose it.
"use strict";

const PipChat = (() => {
  const SAVE_KEY = "pip.chat";
  const C = {
    messages: [], hosts: [], selection: store.get("pip.model") || "", draft: "",
    mode: store.get("pip.mode") === "direct" ? "direct" : "pip",
    system: "", context: true, temperature: 0.3, maxTokens: 0,
    busy: false, abort: null, open: false,
    // Threads: every conversation is a thread kept (encrypted) by Toktail.
    threadId: store.get("pip.thread") || "", threads: [], threadQuery: "", focus: false,
  };
  const views = [];
  let panel, launcher, modelLoading = null, returnFocus = null, sidebar = null, saveTimer = null;

  // ================================================================ persistence
  function save() {
    persistThread();
    try {
      const keep = C.messages.filter((m) => !m.pending).slice(-40).map((m) => ({ ...m, el: undefined }));
      let text = JSON.stringify({ messages: keep, system: C.system, context: C.context, temperature: C.temperature, maxTokens: C.maxTokens });
      if (text.length > 600000) text = JSON.stringify({ messages: keep.map((m) => ({ ...m, cards: [], steps: (m.steps || []).map((s) => ({ ...s, result: undefined })) })), system: C.system, context: C.context, temperature: C.temperature, maxTokens: C.maxTokens });
      localStorage.setItem("toktail." + SAVE_KEY, text);
    } catch (e) { /* private window or full: the chat just isn't kept */ }
  }
  function restore() {
    try {
      const v = JSON.parse(localStorage.getItem("toktail." + SAVE_KEY) || "null");
      if (!v) return;
      C.messages = Array.isArray(v.messages) ? v.messages : [];
      C.system = typeof v.system === "string" ? v.system : "";
      C.context = v.context !== false;
      if (Number.isFinite(v.temperature)) C.temperature = v.temperature;
      if (Number.isFinite(v.maxTokens)) C.maxTokens = v.maxTokens;
    } catch (e) { /* ignore a damaged copy */ }
  }

  // ================================================================ threads
  function threadBody() {
    const sel = selected();
    return { id: C.threadId, host: sel ? sel[0] : "", model: sel ? sel[1] : "", mode: C.mode, system: C.system, context: C.context, temperature: C.temperature, max_tokens: C.maxTokens || null,
      messages: C.messages.filter((m) => !m.pending).map((m) => ({ ...m, el: undefined })) };
  }
  /** Writes the current thread to Toktail (debounced); empty threads aren't kept. */
  function persistThread() {
    clearTimeout(saveTimer);
    if (!C.messages.some((m) => !m.pending && m.content)) return;
    saveTimer = setTimeout(async () => {
      try {
        const e = await postJSON("/api/threads", threadBody());
        if (e.id && e.id !== C.threadId) { C.threadId = e.id; store.set("pip.thread", e.id); }
        const i = C.threads.findIndex((t) => t.id === e.id);
        if (i >= 0) C.threads[i] = e; else C.threads.unshift(e);
        renderThreads();
      } catch (err) { /* kept in this browser meanwhile */ }
    }, 400);
  }
  async function loadThreads(q) {
    try {
      const r = await getJSON("/api/threads" + (q ? "?q=" + encodeURIComponent(q) : ""));
      C.threads = r.threads || [];
      renderThreads();
    } catch (e) { /* sidebar stays as it was */ }
  }
  async function openThread(id) {
    if (C.busy) return toast("Stop the current answer first.");
    try {
      const t = await getJSON("/api/threads/" + encodeURIComponent(id));
      C.threadId = t.id; store.set("pip.thread", t.id);
      C.messages = Array.isArray(t.messages) ? t.messages : [];
      C.system = t.system || ""; C.context = t.context !== false;
      if (Number.isFinite(t.temperature)) C.temperature = t.temperature;
      C.maxTokens = t.max_tokens || 0;
      if (t.mode === "direct" || t.mode === "pip") { C.mode = t.mode; store.set("pip.mode", t.mode); }
      if (t.host && t.model) { C.selection = JSON.stringify([t.host, t.model]); store.set("pip.model", C.selection); for (const v of views) v.model.value = C.selection; }
      C.draft = "";
      render(); renderThreads();
    } catch (e) { toast(e.message); }
  }
  function newThread() {
    if (C.busy) return;
    C.threadId = ""; store.set("pip.thread", "");
    C.messages = []; C.draft = "";
    try { localStorage.removeItem("toktail." + SAVE_KEY); } catch (e) { /* fine */ }
    render(); renderThreads();
  }
  async function renameThread(t) {
    const input = h("input", { type: "text", class: "wide", value: t.title, maxlength: "80", "aria-label": "Title" });
    const ok = await modal("Rename chat", input, [{ label: "Cancel", value: false }, { label: "Save", value: true, cls: "primary" }]);
    if (!ok) return;
    try { const e = await postJSON("/api/threads/" + encodeURIComponent(t.id) + "/meta", { title: input.value }); Object.assign(t, e); renderThreads(); } catch (err) { toast(err.message); }
  }
  async function pinThread(t) {
    try { const e = await postJSON("/api/threads/" + encodeURIComponent(t.id) + "/meta", { pinned: !t.pinned }); Object.assign(t, e); C.threads.sort((a, b) => (b.pinned - a.pinned) || (b.updated > a.updated ? 1 : -1)); renderThreads(); } catch (err) { toast(err.message); }
  }
  async function deleteThread(t) {
    if (!(await confirmBox("Delete this chat?", "“" + t.title + "” is removed from Toktail" + (S.meta && S.meta.capture && S.meta.capture.encryption === "per_file" ? " and its key destroyed" : "") + ". The token counts of its turns stay in your history.", "Delete", true))) return;
    try {
      await getJSON("/api/threads/" + encodeURIComponent(t.id), { method: "DELETE" });
      C.threads = C.threads.filter((x) => x.id !== t.id);
      if (C.threadId === t.id) newThread(); else renderThreads();
    } catch (err) { toast(err.message); }
  }
  function renderThreads() {
    if (!sidebar) return;
    const list = sidebar.querySelector(".thread-list");
    list.textContent = "";
    if (!C.threads.length) list.append(h("div", { class: "label-sm", style: "padding:8px", text: C.threadQuery ? "No chats match." : "Your chats appear here. Each one is kept encrypted by Toktail, apart from the proxy's prompt history." }));
    for (const t of C.threads) {
      const row = h("div", { class: "thread" + (t.id === C.threadId ? " sel" : "") + (t.pinned ? " pinned" : ""), role: "button", tabindex: "0", title: t.preview || "" },
        h("div", { class: "th-main" }, h("div", { class: "th-title" }, t.pinned ? span("📌 ", "") : null, document.createTextNode(t.title || "New chat")), h("div", { class: "label-sm", text: when(t.updated) + " · " + t.messages + (t.model ? " · " + t.model : "") })),
        h("div", { class: "th-tools" },
          h("button", { type: "button", class: "tiny", title: t.pinned ? "Unpin" : "Pin", "aria-label": t.pinned ? "Unpin" : "Pin", text: t.pinned ? "⊝" : "📌", onclick: (e) => { e.stopPropagation(); pinThread(t); } }),
          h("button", { type: "button", class: "tiny", title: "Rename", "aria-label": "Rename", text: "✎", onclick: (e) => { e.stopPropagation(); renameThread(t); } }),
          h("button", { type: "button", class: "tiny danger", title: "Delete", "aria-label": "Delete", text: "×", onclick: (e) => { e.stopPropagation(); deleteThread(t); } })));
      row.onclick = () => openThread(t.id);
      row.onkeydown = (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); openThread(t.id); } };
      list.append(row);
    }
  }
  function buildSidebar() {
    const search = h("input", { type: "search", placeholder: "Search chats", "aria-label": "Search chats", class: "wide" });
    let t;
    search.oninput = () => { C.threadQuery = search.value.trim(); clearTimeout(t); t = setTimeout(() => loadThreads(C.threadQuery), 250); };
    const add = h("button", { type: "button", class: "small primary", text: "+ New chat", onclick: newThread });
    sidebar = h("aside", { class: "thread-side", "aria-label": "Chats" }, h("div", { class: "thread-head" }, search, add), h("div", { class: "thread-list" }));
    return sidebar;
  }

  // ================================================================ focus mode
  /** Full-viewport chat: a modal over a blurred page, only the thread list,
   *  the conversation and the input. Never the window itself: the green
   *  button is for that. Esc, pressed twice, sizes it back down. */
  let escAt = 0;
  function setFocus(on) {
    C.focus = on;
    document.body.classList.toggle("chat-focus", on);
    for (const v of views) if (v.focusBtn) { v.focusBtn.textContent = on ? "⤡ Exit full screen" : "⤢ Full screen"; v.focusBtn.setAttribute("aria-pressed", String(on)); }
    if (on) { const v = views.find((x) => x.root === $("chat-card")); if (v) setTimeout(() => v.text.focus(), 50); }
    else escAt = 0;
  }
  /** Esc once warns, Esc again within 1.5 s leaves. Returns true when handled. */
  function focusEscape() {
    if (!C.focus) return false;
    const now = performance.now();
    if (now - escAt < 1500) { setFocus(false); return true; }
    escAt = now;
    toast("Press Esc again to exit full screen");
    return true;
  }

  // ================================================================ models
  async function loadModels(force) {
    if (modelLoading) return modelLoading;
    if (C.hosts.length && !force) return;
    modelLoading = (async () => {
      try {
        const r = await getJSON("/api/pip/models");
        C.hosts = r.hosts || [];
        for (const v of views) fillModels(v);
      } catch (e) {
        for (const v of views) v.status.textContent = "Couldn't list models: " + e.message;
      } finally { modelLoading = null; }
    })();
    return modelLoading;
  }
  function fillModels(v) {
    v.model.textContent = "";
    for (const host of C.hosts) {
      const g = h("optgroup", { label: host.name + (host.problem ? " (unreachable)" : "") });
      for (const m of host.models || []) g.append(h("option", { value: JSON.stringify([host.host, m]), text: m }));
      if (!(host.models || []).length) g.append(h("option", { value: "", disabled: true, text: host.problem ? "Not answering: " + host.problem : "No models loaded" }));
      v.model.append(g);
    }
    const opts = [...v.model.options].filter((o) => o.value);
    if (!opts.length) v.model.append(h("option", { value: "", text: "No chat models available" }));
    if (!opts.some((o) => o.value === C.selection)) C.selection = (opts[0] && opts[0].value) || "";
    v.model.value = C.selection;
    paintControls();
  }
  const selected = () => { try { return JSON.parse(C.selection); } catch (e) { return null; } };

  // ================================================================ markdown, safely
  // A small renderer that only ever creates elements and text nodes: never
  // innerHTML. Links go to records inside Toktail or open https pages.
  function inline(text) {
    const out = document.createDocumentFragment();
    const re = /(`[^`\n]+`)|(\*\*[^*\n]+\*\*)|(\[[^\]\n]+\]\([^)\s]+\))|(\*[^*\n]+\*)/g;
    let last = 0, m;
    while ((m = re.exec(text))) {
      if (m.index > last) out.append(text.slice(last, m.index));
      const t = m[0];
      if (m[1]) out.append(h("code", { text: t.slice(1, -1) }));
      else if (m[2]) out.append(h("strong", { text: t.slice(2, -2) }));
      else if (m[4]) out.append(h("em", { text: t.slice(1, -1) }));
      else {
        const label = t.slice(1, t.indexOf("]")), href = t.slice(t.indexOf("](") + 2, -1);
        if (/^#\/prompts\/[A-Za-z0-9-]{1,100}$/.test(href) || /^#\/(dashboard|models|savings|settings)(\/[a-z-]+)?$/.test(href)) out.append(h("a", { href, text: label, onclick: () => { if (C.open) close(); } }));
        else if (/^https:\/\/[^\s"<>]+$/.test(href)) out.append(h("a", { href, text: label, target: "_blank", rel: "noopener noreferrer" }));
        else out.append(label);
      }
      last = m.index + t.length;
    }
    if (last < text.length) out.append(text.slice(last));
    return out;
  }
  function markdown(src) {
    const box = h("div", { class: "md" });
    const lines = String(src || "").replace(/\r\n/g, "\n").split("\n");
    let i = 0, para = [];
    const flush = () => { if (para.length) { box.append(h("p", {}, inline(para.join("\n")))); para = []; } };
    while (i < lines.length) {
      const line = lines[i];
      if (/^```/.test(line)) {
        flush();
        const code = [];
        i++;
        while (i < lines.length && !/^```/.test(lines[i])) code.push(lines[i++]);
        i++;
        const text = code.join("\n");
        const copy = h("button", { type: "button", class: "small", text: "Copy", onclick: () => copyText(text) });
        box.append(h("pre", {}, copy, h("code", { text })));
        continue;
      }
      const head = /^(#{1,4})\s+(.*)$/.exec(line);
      if (head) { flush(); box.append(h("h4", {}, inline(head[2]))); i++; continue; }
      if (/^\s*([-*•]|\d+[.)])\s+/.test(line)) {
        flush();
        const ordered = /^\s*\d/.test(line);
        const list = h(ordered ? "ol" : "ul");
        while (i < lines.length && /^\s*([-*•]|\d+[.)])\s+/.test(lines[i])) {
          list.append(h("li", {}, inline(lines[i].replace(/^\s*([-*•]|\d+[.)])\s+/, ""))));
          i++;
        }
        box.append(list);
        continue;
      }
      if (/^\|.*\|\s*$/.test(line) && i + 1 < lines.length && /^\|[\s:|-]+\|\s*$/.test(lines[i + 1])) {
        flush();
        const cells = (l) => l.trim().replace(/^\||\|$/g, "").split("|").map((c) => c.trim());
        const t = h("table");
        const hr = t.createTHead().insertRow();
        for (const c of cells(line)) hr.append(h("th", { class: "l" }, inline(c)));
        i += 2;
        const tb = t.createTBody();
        while (i < lines.length && /^\|.*\|\s*$/.test(lines[i])) {
          const r = tb.insertRow();
          for (const c of cells(lines[i])) { const td = r.insertCell(); td.className = "l"; td.append(inline(c)); }
          i++;
        }
        box.append(h("div", { class: "tbl" }, t));
        continue;
      }
      if (!line.trim()) { flush(); i++; continue; }
      para.push(line);
      i++;
    }
    flush();
    return box;
  }
  function copyText(t) { if (navigator.clipboard) navigator.clipboard.writeText(t).then(() => toast("Copied")); }

  // ================================================================ receipts: what Pip looked at
  const N = (v) => (v == null ? "—" : typeof v === "number" ? fmt(v) : String(v));
  const usd2 = (v) => (v == null ? "—" : usd(v));
  function receipt(title, big, lines, extra, open) {
    const d = h("details", { class: "receipt" });
    if (open) d.open = true;
    d.append(h("summary", {}, h("span", { class: "k", text: title }), big != null ? h("b", { text: big }) : null));
    if (lines && lines.length) d.append(h("div", { class: "lines" }, lines.map(([k, v]) => h("div", { class: "line" }, span(k, ""), span(v, "")))));
    if (extra) d.append(extra);
    return d;
  }
  function card(c) {
    if (!c || typeof c !== "object") return null;
    switch (c.kind) {
      case "usage": {
        const lines = [["Input (read)", N(c.input)], ["Output (generated)", N(c.output)], ["Combined", N(c.combined)], ["Requests", N(c.requests)]];
        if (c.estimated_requests) lines.push(["Counted from the stream", N(c.estimated_requests)]);
        if (c.tokens_per_sec) lines.push(["Speed while generating", c.tokens_per_sec.toFixed(1) + " tok/s"]);
        if ((c.by_host || []).length > 1) for (const x of c.by_host) lines.push(["  " + x.host, N(x.combined)]);
        for (const g of (c.groups || []).slice(0, 12)) lines.push(["  " + g.key, N(g.combined)]);
        return receipt("CSV · " + (c.scope || "usage"), compact(c.combined || 0) + " tokens", lines, c.note ? h("div", { class: "note", text: c.note }) : null);
      }
      case "compare":
        return receipt("Compared · " + (c.a && c.a.scope), c.change_pct == null ? "—" : (c.change_pct >= 0 ? "+" : "") + c.change_pct + "%", [[c.a.scope, N(c.a.combined)], [c.b.scope, N(c.b.combined)]]);
      case "models":
        return receipt("Models · " + (c.scope || ""), (c.models || []).length + " models", (c.models || []).slice(0, 10).map((m) => [m.model, (m.tokens_per_sec != null ? m.tokens_per_sec.toFixed(1) + " tok/s" : "—") + " · " + (m.ttft_ms_p50 != null ? fmt(m.ttft_ms_p50) + " ms" : "—")]));
      case "requests":
        return receipt("Requests by " + c.sort + " · " + (c.scope || ""), (c.requests || []).length + " shown", null,
          h("div", {}, (c.requests || []).map((r) => h("div", { class: "rec" }, h("p", {}, r.href ? h("a", { href: r.href, text: r.model, onclick: () => C.open && close() }) : h("b", { text: r.model }), " · " + r.host + " · " + fmt(r.input) + " in · " + fmt(r.output) + " out" + (r.tokens_per_sec ? " · " + r.tokens_per_sec + " tok/s" : ""))))));
      case "cost":
        return receipt("Cloud price · " + (c.scope || ""), usd2(c.baseline_usd), [["At " + (c.baseline || "baseline"), usd2(c.baseline_usd)], ["Local electricity", usd2(c.electricity_usd)], ["Hours generating", String(c.busy_hours)]].concat((c.services || []).slice(0, 8).map((x) => [x.service, usd2(x.usd)])), h("div", { class: "note", text: c.note || "" }));
      case "savings":
        return receipt("Savings · against " + (c.baseline || ""), usd2(c.saved_usd), [["Cloud price", usd2(c.cloud_usd)], ["Hardware share", usd2(c.hardware_usd)], ["Electricity", usd2(c.electricity_usd)], ["Pace per month", usd2(c.pace_per_month_usd)]].concat((c.machines || []).map((m) => [m.name, m.paid_back_pct + "% paid back"])));
      case "live":
        return receipt("Right now", (c.in_flight || []).length + " in flight", (c.hosts || []).map((x) => [x.host + (x.answering ? "" : " (not answering)"), x.tokens_per_sec + " tok/s · " + x.in_flight + " in flight"]));
      case "budgets":
        return receipt("Budgets", (c.budgets || []).length + " set", (c.budgets || []).map((b) => [b.name, Math.round(b.ratio * 100) + "% · " + compact(b.used) + " of " + compact(b.limit)]));
      case "records": {
        const recs = c.records || [];
        const cov = c.coverage && c.coverage.complete === false ? h("div", { class: "note", text: "Searched " + fmt(c.coverage.indexed) + " of " + fmt(c.coverage.captured) + " saved records in scope." }) : null;
        const list = h("div", {}, recs.map((r) => {
          if (typeof r.id !== "string" || !/^[A-Za-z0-9-]{1,100}$/.test(r.id)) return null;
          return h("div", { class: "rec" },
            h("p", {}, h("a", { href: "#/prompts/" + encodeURIComponent(r.id), text: r.title || "Saved record", onclick: () => C.open && close() }), " · " + [r.host, r.model, r.date, r.outcome === "refused" ? "refused" : r.signal].filter(Boolean).join(" · ")),
            r.response ? h("pre", { text: r.response.slice(0, 600) }) : null);
        }));
        return receipt("Saved records", recs.length + " found", null, h("div", {}, list, cov));
      }
      case "record":
        return receipt("Record · " + (c.model || ""), null, null, h("div", { class: "rec" }, h("p", {}, h("a", { href: c.href, text: "Open record ↗", onclick: () => C.open && close() })), h("pre", { text: (c.response || "").slice(0, 600) })));
      default:
        return null;
    }
  }

  // ================================================================ rendering
  function renderMessage(m, isLast) {
    const row = h("div", { class: "msg-row " + m.role + (m.error ? " error" : "") });
    if (m.role === "user") {
      row.append(h("div", { class: "say" }, markdown(m.content)));
      return row;
    }
    row.append(h("img", { class: "ava" + (m.pending ? " thinking" : ""), src: "/pip.svg", alt: "" }));
    const say = h("div", { class: "say" });
    const trail = h("div", { class: "pip-trailbox" });
    // A tool often looks up what the facts already showed: one receipt each.
    // Same kind and scope = same receipt; the later lookup is the fresher one.
    const shown = new Map();
    const add = (c) => {
      const r = c ? card(c) : null;
      if (!r) return;
      const key = [c.kind, c.scope || (c.a && c.a.scope) || "", c.b && c.b.scope || "", c.sort || "", c.id || c.href || "", c.query || "", c.kind === "records" ? (c.records || []).map((r) => r.id).join(",") : ""].join("|");
      if (shown.has(key)) shown.get(key).replaceWith(r);
      else trail.append(r);
      shown.set(key, r);
    };
    for (const c of m.cards || []) add(c);
    for (const s of m.steps || []) {
      trail.append(h("div", { class: "pip-step" }, h("span", {}, h("b", { text: "Looked up " }), s.summary || s.tool)));
      add(s.result);
    }
    if (trail.childElementCount) say.append(trail);
    if (m.reasoning) say.append(h("details", { class: "thinking" }, h("summary", { text: "Thinking" }), h("div", { text: m.reasoning })));
    if (m.content) say.append(markdown(m.content));
    else if (m.pending) say.append(h("div", { class: "md" }, h("p", {}, span(m.steps && m.steps.length ? "Reading what I found…" : C.mode === "direct" ? "Waiting for the model…" : "Sniffing through your trail…", "label-sm"), h("span", { class: "cursor" }))));
    if (m.pending && m.content) say.lastChild.append(h("span", { class: "cursor" }));
    if (m.error) say.append(h("div", { class: "say-err", text: m.error }));
    const foot = h("div", { class: "say-foot" });
    if (m.stats) {
      const s = m.stats;
      foot.append(span(fmt(s.input || 0) + " in · " + fmt(s.output || 0) + " out" + (s.tps ? " · " + s.tps.toFixed(1) + " tok/s" : "") + (s.ttft ? " · first token " + fmt(s.ttft) + " ms" : ""), ""));
    }
    if (m.model) foot.append(span(m.model, ""));
    if (m.recordId) foot.append(h("a", { href: "#/prompts/" + encodeURIComponent(m.recordId), text: "Open record", onclick: () => C.open && close() }));
    if (!m.pending && m.content) foot.append(h("button", { type: "button", class: "small", text: "Copy", onclick: () => copyText(m.content) }));
    if (!m.pending && isLast && !C.busy) foot.append(h("button", { type: "button", class: "small", text: "Try again", onclick: regenerate }));
    if (foot.childElementCount) say.append(foot);
    row.append(say);
    return row;
  }
  function suggestions() {
    const hosts = (S.meta ? S.meta.hosts.filter((x) => !x.removed) : []);
    const host = hosts.length > 1 ? hosts[hosts.length - 1].name : "my hosts";
    if (C.mode === "direct") return ["Explain tail -f in one paragraph", "Write a haiku about a fox who counts tokens", "Refactor this function to use early returns"];
    return [
      "How many tokens did " + host + " process last month?",
      "Which model is fastest this week, and which is slowest?",
      "Compare this week with last week",
      "Show me the prompts the models refused to answer",
      "What would this month have cost on Claude Sonnet?",
      "Am I on track with my budgets?",
    ];
  }
  function render() {
    for (const v of views) {
      const nearEnd = v.log.scrollHeight - v.log.scrollTop - v.log.clientHeight < 120;
      v.log.textContent = "";
      if (!C.messages.length) {
        const w = h("div", { class: "pip-welcome" }, h("img", { src: "/pip.svg", alt: "" }),
          h("h2", { text: C.mode === "direct" ? "Straight to your model" : "A little fox. Your whole trail." }),
          h("p", { text: C.mode === "direct" ? "Only your messages and your system prompt are sent. Toktail still counts every token." : "Ask about your tokens, hosts, models, costs or saved prompts. Pip answers with your own model and shows the exact numbers it used." }));
        const list = h("div", { class: "pip-suggestions" });
        for (const t of suggestions()) list.append(h("button", { type: "button", class: "pip-suggestion", text: t, onclick: () => send(t) }));
        w.append(list);
        v.log.append(w);
      } else {
        C.messages.forEach((m, i) => v.log.append(renderMessage(m, i === C.messages.length - 1)));
      }
      if (nearEnd || !C.busy) v.log.scrollTop = v.log.scrollHeight;
    }
    paintControls();
  }
  function paintControls() {
    for (const v of views) {
      if (document.activeElement !== v.text) v.text.value = C.draft;
      v.send.disabled = C.busy || !C.selection;
      v.send.hidden = C.busy;
      v.stop.hidden = !C.busy;
      v.model.disabled = C.busy;
      for (const b of v.modeSeg.children) b.setAttribute("aria-pressed", String(b.dataset.m === C.mode));
      if (document.activeElement !== v.system) v.system.value = C.system;
      v.sysNote.textContent = C.mode === "direct" ? "Sent as the only system message." : "Replaces Pip's persona. Pip's rules for your numbers still apply.";
      v.sysBadge.hidden = !C.system.trim();
      v.contextRow.hidden = C.mode === "direct";
      v.context.checked = C.context;
      if (document.activeElement !== v.temperature) v.temperature.value = String(C.temperature);
      if (document.activeElement !== v.maxTokens) v.maxTokens.value = C.maxTokens ? String(C.maxTokens) : "";
      v.text.placeholder = C.mode === "direct" ? "Message your model…" : "Ask Pip about your tokens, models, costs…";
      const sel = selected();
      v.status.textContent = C.busy ? (C.mode === "direct" ? "Your model is answering…" : "Pip is answering…") :
        sel ? (C.mode === "direct" ? "Direct" : "Pip") + " · " + sel[1] + " on " + hostName(sel[0]) + " · counted by Toktail" + (S.meta && S.meta.demo ? " · demo replies are canned" : "") : "Pick a model to start";
    }
  }

  // ================================================================ sending
  function history() {
    return C.messages.filter((m) => !m.error && !m.pending && m.content).slice(-40).map((m) => ({ role: m.role, content: m.content }));
  }
  async function send(content) {
    content = String(content || "").trim();
    if (!content) return;
    if (C.busy) return toast("Pip is still answering. Stop it first.");
    if (!C.selection) { await loadModels(true); if (!C.selection) return toast("No chat model is available on your hosts right now."); }
    if (!C.threadId) { C.threadId = "t_" + Array.from(crypto.getRandomValues(new Uint8Array(8)), (b) => b.toString(16).padStart(2, "0")).join(""); store.set("pip.thread", C.threadId); }
    C.messages.push({ role: "user", content });
    C.draft = "";
    for (const v of views) { v.text.value = ""; v.text.style.height = ""; }
    return run();
  }
  async function regenerate() {
    if (C.busy) return;
    while (C.messages.length && C.messages[C.messages.length - 1].role === "assistant") C.messages.pop();
    if (!C.messages.length) return;
    return run();
  }
  async function run() {
    const sel = selected();
    if (!sel) return;
    const [host, model] = sel;
    const msgs = history();
    const reply = { role: "assistant", content: "", reasoning: "", pending: true, cards: [], steps: [], model, mode: C.mode };
    C.messages.push(reply);
    C.busy = true;
    C.abort = new AbortController();
    render();
    const started = Date.now();
    let firstAt = 0;
    try {
      const resp = await fetch("/api/pip/chat", {
        method: "POST", headers: { "Content-Type": "application/json" }, signal: C.abort.signal,
        body: JSON.stringify({ host, model, mode: C.mode, system: C.system || null, context: C.context, temperature: C.temperature, max_tokens: C.maxTokens || null, messages: msgs, thread: C.threadId || null }),
      });
      if (!resp.ok) throw new Error((await resp.text()) || "HTTP " + resp.status);
      const reader = resp.body.getReader(), dec = new TextDecoder();
      let buf = "", lastPaint = 0, finished = false;
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buf += dec.decode(value, { stream: true }).replace(/\r\n/g, "\n");
        let end;
        while ((end = buf.indexOf("\n\n")) >= 0) {
          const raw = buf.slice(0, end); buf = buf.slice(end + 2);
          const kind = (raw.split("\n").find((l) => l.startsWith("event:")) || "").slice(6).trim();
          const data = raw.split("\n").filter((l) => l.startsWith("data:")).map((l) => l.slice(5).trimStart()).join("\n");
          if (!data) continue;
          let item; try { item = JSON.parse(data); } catch (e) { continue; }
          if (kind === "facts") reply.cards = item.cards || [];
          else if (kind === "step") reply.steps.push(item);
          else if (kind === "sources") reply.sources = item.sources || [];
          else if (kind === "reasoning") reply.reasoning += item.text || "";
          else if (kind === "delta") { if (!firstAt && item.text && item.text.trim()) firstAt = Date.now(); reply.content += item.text || ""; }
          else if (kind === "usage") reply.stats = { input: +item.input || 0, output: +item.output || 0 };
          else if (kind === "error") throw new Error(item.message || "The model sent an error.");
          else if (kind === "done") finished = true;
        }
        if (performance.now() - lastPaint > 70) { render(); lastPaint = performance.now(); }
      }
      if (!finished && !reply.content) throw new Error("The model stopped before answering.");
      reply.content = reply.content.trim();
      if (reply.stats && firstAt) {
        reply.stats.ttft = firstAt - started;
        const secs = (Date.now() - firstAt) / 1000;
        if (secs > 0.3 && reply.stats.output) reply.stats.tps = reply.stats.output / secs;
      }
    } catch (e) {
      reply.error = e.name === "AbortError" ? "Stopped." : e.message;
    } finally {
      reply.pending = false;
      C.busy = false;
      C.abort = null;
      render();
      save();
      linkRecord(reply, host, model, started);
      if (typeof Main !== "undefined") Main.refreshMeta();
    }
  }
  /** Points the answer at the record Toktail just saved for it, if saving is on. */
  function linkRecord(reply, host, model, started) {
    setTimeout(async () => {
      try {
        const r = await getJSON("/api/recent?limit=8&hosts=" + encodeURIComponent(host));
        const hit = r.find((x) => x.model === model && x.captured && new Date(x.timestamp).getTime() >= started - 2000);
        if (hit) { reply.recordId = hit.request_id; render(); save(); }
      } catch (e) { /* no link then */ }
    }, 1500);
  }

  // ================================================================ building a view
  function mount(root, compactView) {
    if (root.dataset.pipMounted) return;
    root.dataset.pipMounted = "true";
    const model = h("select", { "aria-label": "Chat model", class: "pip-model" });
    model.onchange = () => { C.selection = model.value; store.set("pip.model", C.selection); for (const v of views) v.model.value = C.selection; paintControls(); };
    const modeSeg = h("div", { class: "seg", role: "group", "aria-label": "Chat mode" },
      h("button", { type: "button", "data-m": "pip", text: "Pip", title: "Pip answers with your Toktail data" }),
      h("button", { type: "button", "data-m": "direct", text: "Direct", title: "Only your messages and system prompt" }));
    modeSeg.onclick = (e) => { const b = e.target.closest("button"); if (!b || C.busy) return; C.mode = b.dataset.m; store.set("pip.mode", C.mode); render(); };
    const refresh = h("button", { type: "button", class: "small", text: "↻", title: "Refresh models", "aria-label": "Refresh models" });
    refresh.onclick = () => loadModels(true);
    const clear = h("button", { type: "button", class: "small", text: "New chat" });
    clear.onclick = () => { if (C.busy) return; newThread(); };
    const focusBtn = compactView ? null : h("button", { type: "button", class: "small", text: "⤢ Full screen", "aria-pressed": "false", title: "Fill the view with the chat (Esc twice to leave)" });
    if (focusBtn) focusBtn.onclick = () => setFocus(!C.focus);

    const system = h("textarea", { rows: 3, placeholder: "System prompt (optional), e.g. You are a terse senior Rust reviewer.", "aria-label": "System prompt", spellcheck: "false" });
    system.oninput = () => { C.system = system.value; for (const v of views) if (v.system !== system) v.system.value = C.system; for (const v of views) v.sysBadge.hidden = !C.system.trim(); save(); };
    const sysNote = h("div", { class: "label-sm" });
    const context = h("input", { type: "checkbox" });
    context.onchange = () => { C.context = context.checked; save(); paintControls(); };
    const contextRow = h("label", {}, context, "Use my Toktail data (facts and tools)");
    const temperature = h("input", { type: "number", min: 0, max: 2, step: 0.1, "aria-label": "Temperature" });
    temperature.onchange = () => { C.temperature = Math.max(0, Math.min(2, Number(temperature.value) || 0)); save(); paintControls(); };
    const maxTokens = h("input", { type: "number", min: 0, max: 32768, step: 64, placeholder: "auto", "aria-label": "Max tokens" });
    maxTokens.onchange = () => { C.maxTokens = Math.max(0, Math.min(32768, Math.round(Number(maxTokens.value) || 0))); save(); paintControls(); };
    const sysBadge = h("span", { class: "sys-badge", text: "system prompt on", hidden: true });
    const options = h("details", { class: "pip-chat-options" },
      h("summary", {}, "System prompt & settings", sysBadge),
      h("div", { class: "opt-grid" }, system, sysNote,
        h("div", { class: "opt-row" }, contextRow, h("label", {}, "Temperature", temperature), h("label", {}, "Max tokens", maxTokens))));
    if (!compactView) options.open = !!C.system;

    const log = h("div", { class: "pip-log", "aria-live": "polite" });
    const text = h("textarea", { rows: compactView ? 2 : 3, "aria-label": "Message" });
    text.oninput = () => { C.draft = text.value; for (const v of views) if (v.text !== text) v.text.value = C.draft; text.style.height = "auto"; text.style.height = Math.min(180, text.scrollHeight) + "px"; };
    const sendBtn = h("button", { type: "submit", class: "primary", text: "Send" });
    const stop = h("button", { type: "button", class: "danger", text: "Stop", hidden: true });
    stop.onclick = () => C.abort && C.abort.abort();
    const form = h("form", { class: "pip-compose" }, text, stop, sendBtn);
    form.onsubmit = (e) => { e.preventDefault(); send(text.value); };
    text.onkeydown = (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); form.requestSubmit(); } };
    const status = h("div", { class: "pip-chat-status" });
    const v = { root, model, modeSeg, log, text, send: sendBtn, stop, status, system, sysNote, sysBadge, context, contextRow, temperature, maxTokens, focusBtn };
    views.push(v);
    const toolbar = h("div", { class: "pip-chat-toolbar" }, model, modeSeg, refresh, clear);
    if (focusBtn) toolbar.append(h("span", { class: "grow" }), focusBtn);
    root.append(toolbar, options, log, form, status);
    fillModels(v);
  }

  // ================================================================ panel
  function open(question) {
    if (!panel) return;
    if (!C.open) returnFocus = document.activeElement;
    C.open = true;
    panel.hidden = false;
    launcher.setAttribute("aria-expanded", "true");
    render();
    loadModels();
    const v = views.find((x) => x.root === panel.querySelector(".pip-panel-body"));
    setTimeout(() => v && v.text.focus(), 30);
    if (question) { if (C.mode === "direct") { C.mode = "pip"; store.set("pip.mode", "pip"); } send(question); }
  }
  function close() {
    C.open = false;
    if (panel) panel.hidden = true;
    if (launcher) launcher.setAttribute("aria-expanded", "false");
    if (returnFocus && returnFocus.focus) returnFocus.focus();
  }
  function showPage() { close(); render(); loadModels(); loadThreads(C.threadQuery); }

  /** Re-runs a saved request in Direct mode: same system prompt, same turns, same model. */
  async function continueRecord(id, row) {
    if (C.busy) return toast("Stop the current answer first.");
    try {
      const [p, r] = await Promise.all([getJSON("/api/records/" + encodeURIComponent(id) + "/prompt"), getJSON("/api/records/" + encodeURIComponent(id) + "/response")]);
      C.threadId = ""; store.set("pip.thread", "");
      C.messages = [];
      C.system = "";
      for (const m of p.content.messages || []) {
        if (m.role === "system") C.system = (C.system ? C.system + "\n\n" : "") + m.text;
        else C.messages.push({ role: m.role === "assistant" ? "assistant" : "user", content: m.text });
      }
      if (r.content && r.content.content) C.messages.push({ role: "assistant", content: r.content.content, model: row.model, recordId: id });
      C.mode = "direct";
      store.set("pip.mode", "direct");
      C.selection = JSON.stringify([row.host, row.model]);
      store.set("pip.model", C.selection);
      save();
      location.hash = "#/prompts/chat";
      for (const v of views) fillModels(v);
      render();
      toast("Continuing in Direct mode with the record's own system prompt.");
    } catch (e) { toast(e.message); }
  }

  function init() {
    restore();
    launcher = h("button", { class: "pip-launcher", type: "button", "aria-label": "Ask Pip", "aria-controls": "pip-panel", "aria-expanded": "false", title: "Ask Pip (/)" }, h("img", { src: "/pip.svg", alt: "" }), h("span", { text: "Ask Pip" }), h("kbd", { text: "/" }));
    launcher.onclick = () => (C.open ? close() : open());
    panel = h("section", { id: "pip-panel", class: "pip-panel", hidden: true, role: "dialog", "aria-label": "Chat with Pip" });
    const closeBtn = h("button", { type: "button", class: "small", text: "×", "aria-label": "Close" });
    closeBtn.onclick = close;
    const expand = h("a", { href: "#/prompts/chat", class: "btn", text: "Expand", title: "Open the chat full page", onclick: close });
    const body = h("div", { class: "pip-panel-body" });
    panel.append(h("header", { class: "pip-panel-head" }, h("img", { src: "/pip.svg", alt: "" }),
      h("div", { class: "t" }, h("b", { text: "Pip" }), h("small", {}, "pip> counting with your own models", h("span", { class: "caret" }))), expand, closeBtn), body);
    document.body.append(launcher, panel);
    mount(body, true);
    const card = $("chat-card");
    const wrap = h("div", { class: "chat-wrap" });
    card.parentNode.insertBefore(wrap, card);
    wrap.append(buildSidebar(), card);
    mount(card, false);
    render();
    document.addEventListener("keydown", (e) => {
      if (e.key !== "Escape" || document.querySelector("dialog[open]")) return;
      if (focusEscape()) { e.preventDefault(); return; }
      if (C.open) { e.preventDefault(); close(); }
    });
  }

  return { init, open, close, showPage, continueRecord, send, isOpen: () => C.open };
})();

