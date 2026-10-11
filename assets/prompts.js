// SPDX-License-Identifier: Apache-2.0
// Toktail portal: the Prompts page. Browse saved prompts and responses
// (hidden until you click to reveal), see how each request ranked against
// everything else, annotate, delete, and chat with a host through Toktail.
"use strict";

const Prompts = (() => {
  const P = { rows: [], total: 0, offset: 0, sel: null, checked: new Set(), loading: false, reqSeq: 0, chat: [], streaming: false, abort: null };
  const LIMIT = 50;

  // ================================================================ list
  function params(offset) {
    const p = new URLSearchParams({ limit: LIMIT, offset, sort: $("rec-sort").value });
    const q = $("rec-search").value.trim();
    if (q) p.set("search", q);
    if ($("rec-host").value) p.set("hosts", $("rec-host").value);
    if ($("rec-model").value) p.set("models", $("rec-model").value);
    if ($("rec-starred").checked) p.set("starred", "true");
    if ($("rec-all").checked) p.set("scope", "all");
    const r = $("rec-range").value;
    if (r !== "all" && S.meta) {
      const today = S.meta.today;
      p.set("from", r === "today" ? today : addDays(today, -(+r - 1)));
      p.set("to", today);
    }
    return p;
  }
  async function loadList(append) {
    const seq = ++P.reqSeq;
    const offset = append ? P.offset : 0;
    P.loading = true;
    try {
      const r = await getJSON("/api/records?" + params(offset));
      if (seq !== P.reqSeq) return;
      P.total = r.total;
      P.rows = append ? P.rows.concat(r.rows) : r.rows;
      P.offset = offset + r.rows.length;
      if (!append) P.checked.clear();
      renderList(r.saving);
    } catch (e) {
      toast("Couldn't load records: " + e.message);
    } finally {
      P.loading = false;
    }
  }
  function fillFilters() {
    const hs = $("rec-host"), ms = $("rec-model");
    const keepH = hs.value, keepM = ms.value;
    hs.textContent = ""; ms.textContent = "";
    hs.append(h("option", { value: "", text: "All hosts" }));
    for (const x of S.meta.hosts) hs.append(h("option", { value: x.id, text: x.name }));
    ms.append(h("option", { value: "", text: "All models" }));
    for (const m of S.meta.models) ms.append(h("option", { value: m.model, text: m.model }));
    hs.value = keepH; ms.value = keepM;
  }
  function renderBanner() {
    const b = $("prompts-banner");
    const c = S.meta.capture;
    b.textContent = "";
    b.className = "banner";
    if (!c.enabled) {
      b.hidden = false;
      b.append(h("b", { text: "Saving prompts and responses is off. " }), "Toktail only counts tokens and keeps prompt fingerprints. ", h("a", { href: "#/settings/capture", text: "Turn on saving" }), " to read, search and re-run your prompts here. Earlier requests stay counted but can't be opened.");
    } else if (c.problem) {
      b.hidden = false; b.classList.add("warn");
      b.append(h("b", { text: "Not saving right now: " }), c.problem + " ", h("a", { href: "#/settings/encryption", text: "Fix in Settings" }), ". Nothing is written unencrypted while this is unresolved.");
    } else if (c.error) {
      b.hidden = false; b.classList.add("warn");
      b.append(h("b", { text: "Last save failed: " }), c.error);
    } else b.hidden = true;
    $("prompts-sub").textContent = c.enabled
      ? "Saved to " + c.dir + (c.encryption === "off" ? " (compressed)" : c.encryption === "per_file" ? " (each file with its own key)" : " (encrypted with your password)") + (S.meta.s3.enabled ? ", mirrored to S3" + (c.pending_upload ? " (" + fmt(c.pending_upload) + " waiting to upload)" : "") : "") + "."
      : "Every saved prompt and response, with how it performed.";
  }
  function renderList() {
    $("rec-count").textContent = fmt(P.total) + ($("rec-all").checked ? " requests" : " saved records");
    $("rec-count-sub").textContent = P.total ? "Showing " + fmt(P.rows.length) + ". Prompts stay hidden until you open one." : "";
    const box = $("rec-items"); box.textContent = "";
    if (!P.rows.length) {
      box.append(h("div", { class: "empty", text: S.meta.capture.enabled ? "No records match. New requests appear here as they finish." : "No saved records yet." }));
    }
    for (const it of P.rows) box.append(item(it));
    $("rec-more").hidden = P.rows.length >= P.total;
    updateBulk();
  }
  function item(it) {
    const r = it.row, n = it.note || {};
    const id = r.request_id;
    const cb = h("input", { type: "checkbox", class: "rec-cb", "aria-label": "Select" });
    cb.checked = P.checked.has(id);
    cb.addEventListener("click", (e) => { e.stopPropagation(); if (cb.checked) P.checked.add(id); else P.checked.delete(id); updateBulk(); });
    const title = n.title || (r.captured ? "Untitled prompt" : "Not saved (count only)");
    const row = h("div", { class: "rec-item" + (P.sel === id ? " sel" : "") + (r.captured ? "" : " nofile"), role: "option", tabindex: "0", "data-id": id, "aria-selected": String(P.sel === id) },
      cb,
      h("div", { class: "ri-main" },
        h("div", { class: "ri-top" },
          n.starred ? span("★", "star on") : null,
          h("span", { class: "ri-title", text: title }),
          h("span", { class: "ri-time", text: when(r.timestamp) })),
        h("div", { class: "ri-meta" },
          swatchText(hostName(r.host), hostColorVar(r.host)),
          span(r.model, "ri-model"),
          (n.tags || []).map((t) => span(t, "tag"))),
        h("div", { class: "ri-nums" },
          h("b", { text: fmt(r.input + r.output) }), span(" combined", "label-sm"),
          span(" · " + fmt(r.input) + " in · " + fmt(r.output) + " out", "label-sm"),
          r.tokens_per_sec != null ? span(" · " + r.tokens_per_sec.toFixed(1) + " tok/s", "label-sm") : null,
          r.status >= 400 ? span(String(r.status), "tag err") : null)));
    row.addEventListener("click", () => open(id));
    row.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); open(id); }
      if (e.key === "ArrowDown" || e.key === "ArrowUp") {
        e.preventDefault();
        const sib = e.key === "ArrowDown" ? row.nextElementSibling : row.previousElementSibling;
        if (sib && sib.dataset.id) { sib.focus(); open(sib.dataset.id); }
      }
    });
    return row;
  }
  function updateBulk() {
    const n = P.checked.size;
    $("rec-bulk-delete").disabled = !n;
    $("rec-bulk-delete").textContent = n ? "Delete " + fmt(n) + " selected" : "Delete selected";
  }

  // ================================================================ detail
  async function open(id) {
    P.sel = id;
    if (location.hash !== "#/prompts/" + id) history.replaceState(null, "", "#/prompts/" + id);
    for (const e of document.querySelectorAll(".rec-item")) { const on = e.dataset.id === id; e.classList.toggle("sel", on); e.setAttribute("aria-selected", String(on)); }
    const box = $("rec-detail");
    box.textContent = "";
    box.append(h("div", { class: "empty", text: "Loading…" }));
    let d;
    try { d = await getJSON("/api/records/" + encodeURIComponent(id)); }
    catch (e) { box.textContent = ""; box.append(h("div", { class: "empty", text: e.message })); return; }
    if (P.sel !== id) return;
    renderDetail(d);
  }

  function rankBar(label, m, unit, kind) {
    if (!m) return null;
    const better = m.higher_is_better;
    // "Faster than 72%": share of requests this one beat.
    const beat = (p) => (p == null ? null : better ? p : 100 - p);
    const all = beat(m.pct_all), mod = beat(m.pct_model);
    const word = { speed: "faster than", ttft: "quicker to start than", out: "longer than", in: "more input than", dur: "quicker than" }[kind];
    const val = unit === "ms" ? (m.value >= 10000 ? (m.value / 1000).toFixed(1) + " s" : fmt(m.value) + " ms") : unit === "t/s" ? m.value.toFixed(1) + " tok/s" : fmt(m.value);
    const med = (v) => (v == null ? "—" : unit === "ms" ? (v >= 10000 ? (v / 1000).toFixed(1) + " s" : fmt(v) + " ms") : unit === "t/s" ? v.toFixed(1) : fmt(v));
    return h("div", { class: "rank" },
      h("div", { class: "rank-head" }, span(label, "rank-label"), h("b", { text: val })),
      h("div", { class: "rank-bars" },
        h("div", { class: "rank-row" }, span("All requests", "label-sm"), h("div", { class: "rank-track" }, h("i", { style: `width:${(all ?? 0).toFixed(1)}%` })), span(all == null ? "—" : Math.round(all) + "%", "rank-pct")),
        h("div", { class: "rank-row" }, span("Same model", "label-sm"), h("div", { class: "rank-track model" }, h("i", { style: `width:${(mod ?? 0).toFixed(1)}%` })), span(mod == null ? "—" : Math.round(mod) + "%", "rank-pct"))),
      h("div", { class: "rank-note label-sm", text: (all == null ? "" : word[0].toUpperCase() + word.slice(1) + " " + Math.round(all) + "% of " + fmt(m.n_all) + " requests. ") + "Median " + med(m.median_all) + " overall, " + med(m.median_model) + " for this model." }));
  }

  function renderDetail(d) {
    const box = $("rec-detail");
    box.textContent = "";
    const r = d.row, n = d.note || { title: "", tags: [], notes: "", starred: false };
    const id = r.request_id;

    // ---- header: title, star, tags
    const title = h("input", { class: "rd-title", value: n.title || "", placeholder: r.captured ? "Add a title" : "Not saved (count only)", "aria-label": "Title", disabled: !r.captured });
    const star = h("button", { type: "button", class: "star-btn" + (n.starred ? " on" : ""), title: n.starred ? "Unstar" : "Star", "aria-pressed": String(!!n.starred), disabled: !r.captured, text: n.starred ? "★" : "☆" });
    box.append(h("div", { class: "rd-head" }, star, title));
    const meta = h("div", { class: "rd-meta" },
      span(when(r.timestamp), ""), span(" · ", "label-sm"),
      swatchText(d.host_name, hostColorVar(r.host)), span(" · ", "label-sm"),
      swatchText(r.model, colorVar(r.model)), span(" · ", "label-sm"),
      span(r.endpoint, "label-sm"), span(" · ", "label-sm"),
      span(String(r.status) + (r.status === 499 ? " client left" : ""), "tag" + (r.status >= 400 ? " err" : "")));
    box.append(meta);

    // ---- token counts
    box.append(h("div", { class: "rd-nums" },
      h("div", {}, h("div", { class: "k", text: "Combined" }), h("div", { class: "v", text: fmt(r.input + r.output) })),
      h("div", {}, h("div", { class: "k", text: "Input" }), h("div", { class: "v", text: fmt(r.input) })),
      h("div", {}, h("div", { class: "k", text: "Output" }), h("div", { class: "v", text: fmt(r.output) })),
      h("div", {}, h("div", { class: "k", text: "Speed" }), h("div", { class: "v", text: r.tokens_per_sec != null ? r.tokens_per_sec.toFixed(1) + " tok/s" : "—" })),
      h("div", {}, h("div", { class: "k", text: "First token" }), h("div", { class: "v", text: r.ttft_ms != null ? fmt(r.ttft_ms) + " ms" : "—" })),
      h("div", {}, h("div", { class: "k", text: "Duration" }), h("div", { class: "v", text: (r.duration_ms / 1000).toFixed(2) + " s" }))));

    // ---- how it ranks
    const pf = d.performance || {};
    const ranks = h("div", { class: "rd-ranks" },
      rankBar("Generation speed", pf.tokens_per_sec, "t/s", "speed"),
      rankBar("Time to first token", pf.ttft_ms, "ms", "ttft"),
      rankBar("Output length", pf.output, "", "out"),
      rankBar("Prompt size", pf.input, "", "in"),
      rankBar("Total time", pf.duration_ms, "ms", "dur"));
    box.append(h("h3", { text: "How it ranks" }), ranks);
    if (pf.same_prompt_count > 1) box.append(h("p", { class: "label-sm", text: "This exact prompt was sent " + fmt(pf.same_prompt_count) + " times (same fingerprint)." }));

    // ---- prompt and response, hidden until clicked
    if (r.captured) {
      box.append(reveal(id, "prompt", "Prompt", d), reveal(id, "response", "Response", d));
    } else {
      box.append(h("div", { class: "banner" }, "Only the counts and the prompt's fingerprint were kept for this request, because saving was off when it ran."));
    }

    // ---- notes
    if (r.captured) {
      const tags = h("input", { class: "wide", value: (n.tags || []).join(", "), placeholder: "Tags, separated by commas", "aria-label": "Tags" });
      const notes = h("textarea", { class: "wide", rows: "3", placeholder: "Notes", "aria-label": "Notes" });
      notes.value = n.notes || "";
      const save = async (silent) => {
        const body = { title: title.value.trim(), tags: tags.value.split(",").map((t) => t.trim()).filter(Boolean), notes: notes.value, starred: star.classList.contains("on") };
        try {
          await postJSON("/api/records/" + encodeURIComponent(id) + "/note", body);
          const it = P.rows.find((x) => x.row.request_id === id);
          if (it) { it.note = body; const old = document.querySelector(`.rec-item[data-id="${id}"]`); if (old) old.replaceWith(item(it)); }
          if (!silent) toast("Saved");
        } catch (e) { toast("Couldn't save: " + e.message); }
      };
      title.addEventListener("change", () => save(true));
      title.addEventListener("keydown", (e) => { if (e.key === "Enter") title.blur(); });
      star.addEventListener("click", () => { const on = !star.classList.contains("on"); star.classList.toggle("on", on); star.textContent = on ? "★" : "☆"; star.setAttribute("aria-pressed", String(on)); save(true); });
      box.append(h("h3", { text: "Your notes" }), h("div", { class: "rd-notes" }, tags, notes, h("div", { class: "row-end" }, h("button", { type: "button", class: "small", text: "Save notes", onclick: () => save(false) }))));
    }

    // ---- where the files are, and actions
    const where = h("div", { class: "rd-where label-sm" });
    where.append("Fingerprint ", fpCell(d.fingerprint));
    if (d.files) {
      where.append(" · Files ", h("code", { text: d.files.prompt.replace(/\.prompt$/, ".{prompt,response}") }));
      if (d.availability) where.append(" · ", span(d.availability.local ? "on this Mac" : "not on this Mac", d.availability.local ? "ok-t" : ""), " · ", span(d.availability.s3 ? "in S3" : S.meta.s3.enabled ? "not in S3 yet" : "S3 off", d.availability.s3 ? "ok-t" : ""));
      where.append(" · ", span(d.encryption === "off" ? "gzip, not encrypted" : d.encryption === "per_file" ? "encrypted, own key" : "encrypted with your password", ""));
    }
    box.append(where);
    const actions = h("div", { class: "rd-actions" });
    if (r.captured) actions.append(h("button", { type: "button", class: "small", text: "Continue in chat", onclick: () => continueInChat(id, r) }));
    actions.append(h("button", { type: "button", class: "small danger", text: "Delete…", onclick: () => remove([id]) }));
    box.append(actions);
  }

  /** A hidden section: blurred placeholder until clicked, then the content. */
  function reveal(id, kind, label, d) {
    const wrap = h("section", { class: "reveal", "data-kind": kind });
    const head = h("div", { class: "reveal-head" }, h("h3", { text: label }));
    const tools = h("div", { class: "reveal-tools" });
    head.append(tools);
    const body = h("div", { class: "reveal-body hidden" });
    const shade = h("button", { type: "button", class: "reveal-shade" }, h("span", { class: "eye", text: "◉" }), h("span", { text: "Click to reveal the " + kind }), h("small", { text: d.encryption === "off" ? "Decompressed on this Mac" : "Decrypted on this Mac, shown only here" }));
    // Fake blurred lines so there's something to un-blur.
    const ghost = h("div", { class: "ghost", "aria-hidden": "true" });
    for (let i = 0; i < 5; i++) ghost.append(h("i", { style: `width:${55 + ((i * 37) % 40)}%` }));
    body.append(ghost, shade);
    wrap.append(head, body);
    let loaded = null;
    const hide = () => {
      body.className = "reveal-body hidden"; body.textContent = ""; body.append(ghost, shade); tools.textContent = "";
    };
    shade.addEventListener("click", async () => {
      shade.disabled = true;
      shade.lastChild.textContent = "Opening…";
      try {
        loaded = loaded || (await getJSON("/api/records/" + encodeURIComponent(id) + "/" + kind));
      } catch (e) {
        shade.disabled = false; shade.lastChild.textContent = e.message; return;
      }
      body.className = "reveal-body";
      body.textContent = "";
      body.append(kind === "prompt" ? promptView(loaded.content) : responseView(loaded.content));
      tools.textContent = "";
      tools.append(
        span(fmt(loaded.bytes) + " bytes", "label-sm"),
        h("button", { type: "button", class: "small", text: "Copy", onclick: () => copyText(kind === "prompt" ? loaded.content.messages.map((m) => m.role + ": " + m.text).join("\n\n") : loaded.content.content) }),
        h("a", { class: "btn small", href: "/api/records/" + encodeURIComponent(id) + "/" + kind + "?raw=true", text: "Download original" }),
        h("button", { type: "button", class: "small", text: "Hide", onclick: hide }));
    });
    return wrap;
  }
  function copyText(t) { if (navigator.clipboard) navigator.clipboard.writeText(t).then(() => toast("Copied")); }
  /** Plain text with ``` fenced blocks shown as code. Never parsed as HTML. */
  function richText(text) {
    const box = h("div", { class: "rich" });
    const parts = String(text || "").split(/```/);
    parts.forEach((p, i) => {
      if (i % 2) { const nl = p.indexOf("\n"); box.append(h("pre", {}, h("code", { text: nl >= 0 ? p.slice(nl + 1) : p }))); }
      else if (p) box.append(h("div", { class: "para", text: p.replace(/^\n+|\n+$/g, "") }));
    });
    return box;
  }
  function promptView(c) {
    const box = h("div", { class: "msgs" });
    for (const m of c.messages || []) box.append(h("div", { class: "msg " + m.role }, h("div", { class: "role", text: m.role }), richText(m.text)));
    const params = Object.entries(c.params || {});
    if (params.length) box.append(h("details", { class: "params" }, h("summary", { text: "Request settings (" + params.length + ")" }), h("pre", { text: JSON.stringify(c.params, null, 2) })));
    return box;
  }
  function responseView(c) {
    const box = h("div", { class: "msgs" });
    if (c.reasoning) box.append(h("details", { class: "reasoning" }, h("summary", { text: "Reasoning" }), richText(c.reasoning)));
    box.append(h("div", { class: "msg assistant" }, h("div", { class: "role", text: "assistant" + (c.model ? " · " + c.model : "") }), richText(c.content || "(empty)")));
    return box;
  }

  async function remove(ids) {
    const body = h("div", {},
      h("p", { text: ids.length === 1 ? "Delete this record's saved prompt and response?" : "Delete the saved prompts and responses of " + fmt(ids.length) + " records?" }),
      h("p", { class: "label-sm", text: "The files are removed from this Mac" + (S.meta.s3.enabled ? " and from S3" : "") + (S.meta.capture.encryption === "per_file" ? ", and their keys are destroyed" : "") + ". This can't be undone." }));
    const hist = h("input", { type: "checkbox", id: "del-hist" });
    body.append(h("label", { class: "inline" }, hist, " Also remove ", ids.length === 1 ? "it" : "them", " from the token history (the CSV). Leave this off to keep the counts."));
    const ok = await modal("Delete " + (ids.length === 1 ? "record" : fmt(ids.length) + " records"), body, [{ label: "Cancel", value: false }, { label: "Delete", value: true, cls: "primary danger" }]);
    if (!ok) return;
    try {
      const r = await postJSON("/api/records/delete", { ids, history: hist.checked });
      toast("Deleted " + fmt(r.deleted_files) + " record" + (r.deleted_files === 1 ? "" : "s") + (r.deleted_rows ? " and " + fmt(r.deleted_rows) + " history rows" : "") + (r.errors.length ? ". Some failed: " + r.errors[0] : ""));
      for (const id of ids) P.checked.delete(id);
      if (ids.includes(P.sel)) { P.sel = null; $("rec-detail").textContent = ""; $("rec-detail").append(h("div", { class: "empty", text: "Deleted." })); }
      loadList(false);
    } catch (e) { toast("Couldn't delete: " + e.message); }
  }

  // ================================================================ chat
  function hostsForChat() { return S.meta.hosts.filter((x) => x.enabled && !x.removed); }
  function fillChatHosts() {
    const hs = $("chat-host");
    const keep = hs.value || store.get("chat.host") || "";
    hs.textContent = "";
    for (const x of hostsForChat()) hs.append(h("option", { value: x.id, text: x.name + (x.lmstudio && !x.lmstudio.ok ? " (unreachable)" : "") }));
    if ([...hs.options].some((o) => o.value === keep)) hs.value = keep;
    fillChatModels();
  }
  function fillChatModels() {
    const x = S.meta.hosts.find((y) => y.id === $("chat-host").value);
    const ms = $("chat-model");
    const keep = ms.value || store.get("chat.model") || "";
    ms.textContent = "";
    const models = (x && x.lmstudio && x.lmstudio.models) || [];
    for (const m of models) ms.append(h("option", { value: m, text: m }));
    if (!models.length) ms.append(h("option", { value: "", text: "No models loaded" }));
    if ([...ms.options].some((o) => o.value === keep)) ms.value = keep;
  }
  function renderChat() {
    const log = $("chat-log");
    log.textContent = "";
    if (!P.chat.length) {
      log.append(h("div", { class: "chat-empty" }, h("img", { src: "/pip.svg", alt: "" }), h("p", { text: "Say hi to your model. Pip counts every token, both ways." }), S.meta.capture.enabled ? h("p", { class: "label-sm", text: "This chat is saved as records, like any other request." }) : h("p", { class: "label-sm", text: "Saving is off, so only the counts are kept." })));
    }
    for (const m of P.chat) log.append(bubble(m));
    log.scrollTop = log.scrollHeight;
  }
  function bubble(m) {
    const b = h("div", { class: "bubble " + m.role });
    if (m.reasoning) b.append(h("details", { class: "reasoning" }, h("summary", { text: "Reasoning" }), richText(m.reasoning)));
    b.append(richText(m.content || (m.pending ? "…" : "")));
    if (m.stats) {
      const s = m.stats;
      const foot = h("div", { class: "bubble-foot" }, span(fmt(s.input) + " in · " + fmt(s.output) + " out · " + fmt(s.input + s.output) + " combined", ""),
        s.tps ? span(" · " + s.tps.toFixed(1) + " tok/s", "") : null, s.ttft ? span(" · first token " + fmt(s.ttft) + " ms", "") : null);
      if (s.id) foot.append(" · ", h("a", { href: "#/prompts/" + s.id, text: "Open record" }));
      b.append(foot);
    }
    if (m.error) b.append(h("div", { class: "bubble-err", text: m.error }));
    m.el = b;
    return b;
  }
  async function send(text) {
    if (P.streaming) return;
    const host = $("chat-host").value, model = $("chat-model").value;
    if (!host || !model) return toast("Pick a host and a model first.");
    store.set("chat.host", host); store.set("chat.model", model);
    P.chat.push({ role: "user", content: text });
    const msgs = [];
    const sys = $("chat-system").value.trim();
    if (sys) msgs.push({ role: "system", content: sys });
    for (const m of P.chat) if (!m.error) msgs.push({ role: m.role, content: m.content });
    const reply = { role: "assistant", content: "", reasoning: "", pending: true };
    P.chat.push(reply);
    renderChat();
    P.streaming = true;
    $("chat-send").disabled = true;
    const t0 = performance.now();
    let first = 0, deltas = 0, usage = null;
    const started = new Date();
    try {
      const resp = await fetch("/api/chat", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ host, model, messages: msgs, temperature: num($("chat-temp")) }) });
      if (!resp.ok) throw new Error((await resp.text()) || "HTTP " + resp.status);
      const reader = resp.body.getReader();
      const dec = new TextDecoder();
      let buf = "";
      const paint = () => { const log = $("chat-log"); log.lastChild.replaceWith(bubble(reply)); log.scrollTop = log.scrollHeight; };
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        buf += dec.decode(value, { stream: true });
        let i;
        while ((i = buf.indexOf("\n\n")) >= 0) {
          const ev = buf.slice(0, i); buf = buf.slice(i + 2);
          for (const line of ev.split("\n")) {
            if (!line.startsWith("data:")) continue;
            const data = line.slice(5).trim();
            if (!data || data === "[DONE]") continue;
            let j; try { j = JSON.parse(data); } catch (e) { continue; }
            if (j.error) throw new Error(j.error.message || JSON.stringify(j.error));
            if (j.usage) usage = j.usage;
            for (const c of j.choices || []) {
              const dl = c.delta || {};
              const piece = dl.content || c.text || "";
              const rs = dl.reasoning_content || dl.reasoning || "";
              if (piece || rs) { if (!first) first = performance.now(); deltas++; }
              reply.content += piece; reply.reasoning += rs;
            }
          }
        }
        reply.pending = false;
        paint();
      }
      const out = usage ? usage.completion_tokens || 0 : deltas;
      reply.stats = { input: usage ? usage.prompt_tokens || 0 : 0, output: out, ttft: first ? Math.round(first - t0) : 0, tps: first && out ? out / ((performance.now() - first) / 1000) : 0 };
      reply.pending = false;
      renderChat();
      // Link the bubble to the record Toktail just wrote.
      setTimeout(async () => {
        try {
          const r = await getJSON("/api/records?scope=all&limit=5&hosts=" + encodeURIComponent(host));
          const hit = r.rows.find((x) => x.row.model === model && new Date(x.row.timestamp) >= new Date(started.getTime() - 2000));
          if (hit) {
            reply.stats.id = hit.row.request_id;
            if (hit.row.output) { reply.stats.output = hit.row.output; reply.stats.input = hit.row.input; reply.stats.tps = hit.row.tokens_per_sec || reply.stats.tps; }
            renderChat();
          }
        } catch (e) { /* the link is a nicety */ }
      }, 700);
    } catch (e) {
      reply.pending = false;
      reply.error = "Couldn't get an answer: " + e.message;
      renderChat();
    } finally {
      P.streaming = false;
      $("chat-send").disabled = false;
    }
  }
  async function continueInChat(id, r) {
    try {
      const [p, resp] = await Promise.all([getJSON("/api/records/" + encodeURIComponent(id) + "/prompt"), getJSON("/api/records/" + encodeURIComponent(id) + "/response")]);
      P.chat = [];
      $("chat-system").value = "";
      for (const m of p.content.messages || []) {
        if (m.role === "system") { $("chat-system").value = m.text; $("chat-system").hidden = false; }
        else P.chat.push({ role: m.role === "assistant" ? "assistant" : "user", content: m.text });
      }
      if (resp.content.content) P.chat.push({ role: "assistant", content: resp.content.content });
      setMode("chat");
      if ([...$("chat-host").options].some((o) => o.value === r.host)) { $("chat-host").value = r.host; fillChatModels(); }
      if ([...$("chat-model").options].some((o) => o.value === r.model)) $("chat-model").value = r.model;
      renderChat();
      $("chat-text").focus();
    } catch (e) { toast("Couldn't open it in chat: " + e.message); }
  }

  // ================================================================ page
  function setMode(v) {
    for (const b of $("prompts-mode").children) b.setAttribute("aria-pressed", String(b.dataset.v === v));
    $("records-view").hidden = v !== "records";
    $("chat-view").hidden = v !== "chat";
    const want = v === "chat" ? "#/prompts/chat" : P.sel ? "#/prompts/" + P.sel : "#/prompts";
    if (location.hash !== want) history.replaceState(null, "", want);
    if (v === "chat") { fillChatHosts(); renderChat(); setTimeout(() => $("chat-text").focus(), 50); }
  }
  async function show(sub) {
    if (!S.meta) return;
    renderBanner();
    fillFilters();
    if (sub === "chat") { setMode("chat"); return; }
    setMode("records");
    await loadList(false);
    if (sub) open(sub);
  }
  function refresh() {
    if (S.page !== "prompts" || !S.meta) return;
    renderBanner();
    // New records arrive at the top when sorted newest first and nothing is being typed.
    if ($("records-view").hidden || P.loading || $("rec-sort").value !== "newest" || P.offset > LIMIT) return;
    loadList(false);
  }
  function init() {
    let t;
    $("rec-search").addEventListener("input", () => { clearTimeout(t); t = setTimeout(() => loadList(false), 250); });
    for (const id of ["rec-host", "rec-model", "rec-range", "rec-sort", "rec-starred", "rec-all"]) $(id).addEventListener("change", () => loadList(false));
    $("rec-more").addEventListener("click", () => loadList(true));
    $("rec-select-all").addEventListener("click", () => {
      const ids = P.rows.filter((x) => x.row.captured || $("rec-all").checked).map((x) => x.row.request_id);
      const all = ids.every((id) => P.checked.has(id));
      for (const id of ids) all ? P.checked.delete(id) : P.checked.add(id);
      for (const cb of document.querySelectorAll(".rec-item")) cb.querySelector(".rec-cb").checked = P.checked.has(cb.dataset.id);
      updateBulk();
    });
    $("rec-bulk-delete").addEventListener("click", () => remove([...P.checked]));
    $("prompts-mode").addEventListener("click", (e) => { const b = e.target.closest("button"); if (b) setMode(b.dataset.v); });
    $("chat-host").addEventListener("change", fillChatModels);
    $("chat-system-btn").addEventListener("click", () => { $("chat-system").hidden = !$("chat-system").hidden; });
    $("chat-clear").addEventListener("click", () => { P.chat = []; renderChat(); });
    $("chat-form").addEventListener("submit", (e) => { e.preventDefault(); const t2 = $("chat-text").value.trim(); if (!t2) return; $("chat-text").value = ""; send(t2); });
    $("chat-text").addEventListener("keydown", (e) => { if (e.key === "Enter" && !e.shiftKey && !e.isComposing) { e.preventDefault(); $("chat-form").requestSubmit(); } });
  }
  return { init, show, refresh };
})();
