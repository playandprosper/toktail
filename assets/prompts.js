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
    const signal=$("rec-outcome").value;if(signal==="refused")p.set("outcome","refused");else if(signal==="noteworthy")p.set("signal","noteworthy");
    const r = $("rec-range").value;
    if (r !== "all" && S.meta) {
      const today = S.meta.today;
      const y = addDays(today, -1);
      p.set("from", r === "today" ? today : r === "yesterday" ? y : addDays(today, -(+r - 1)));
      p.set("to", r === "yesterday" ? y : today);
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
    $("rec-count-sub").textContent = P.total ? "Showing " + fmt(P.rows.length) + ". Open a record to inspect the full exchange." : "";
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
    const title = n.title || it.summary || it.preview || (r.captured ? "Saved prompt" : "Not saved (count only)");
    const row = h("div", { class: "rec-item" + (P.sel === id ? " sel" : "") + (r.captured ? "" : " nofile"), role: "option", tabindex: "0", "data-id": id, "aria-selected": String(P.sel === id) },
      cb,
      h("div", { class: "ri-main" },
        h("div", { class: "ri-top" },
          n.starred ? span("★", "star on") : null,
          h("span", { class: "ri-title", text: title }),
          h("span", { class: "ri-time", text: when(r.timestamp) })),
        it.summary && n.title ? h("div", { class: "ri-summary", title: "Pip's summary", text: "🦊 " + it.summary }) : null,
        h("div", { class: "ri-meta" },
          swatchText(hostName(r.host), hostColorVar(r.host)),
          span(r.model, "ri-model"),
          r.outcome==="refused"?span(r.signal==="likely_refusal"?"Likely refusal":"Refused", "tag err"):null,
          ["structured","substantial","tool_call"].includes(r.signal)?span(r.signal.replace("_"," "),"tag"):null,
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
    if (d.summary) box.append(h("div", { class: "rd-summary", title: "Pip's one-sentence summary, written by " + d.summary.model + " " + when(d.summary.at) }, span("🦊", "rd-summary-fox"), span(d.summary.text, "")));

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

    // ---- where this prompt sits in its session chain
    if (d.chain) {
      const c = d.chain;
      const pct = c.size ? Math.round(100 * c.prefix_bytes / c.size) : 0;
      const line = h("div", { class: "rd-chain" },
        h("div", { class: "k", text: "Session chain" }),
        h("div", {},
          c.missing ? span("This link was erased; its descendants were refolded.", "tag err") : null,
          span(c.snapshot ? (c.depth === 0 ? "Root of the chain (full copy)" : "Snapshot (full copy) at link " + (c.depth + 1)) : "Link " + (c.depth + 1) + ": only " + fmt(Math.max(0, c.size - c.prefix_bytes)) + " of " + fmt(c.size) + " bytes stored, " + pct + "% was a repeat of the link before", ""),
          c.children > 1 ? span(" · " + c.children + " branches continue from here", "label-sm") : null,
          h("div", { class: "label-sm" }, "Session ", h("a", { href: "#/prompts/session/" + encodeURIComponent(c.session), text: c.session }), " · " + fmt(c.session_nodes) + " prompts, " + c.session_heads + " branch tip" + (c.session_heads === 1 ? "" : "s") + " · " + human(c.session_saved_bytes) + " saved of " + human(c.session_full_bytes) + (c.agent ? " · " + c.agent : "") + (c.project ? " · " + c.project : ""))));
      box.append(line);
    }

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
    // Struck out while the chain is patched and verified; gone only once it is.
    const rows = ids.map((id) => document.querySelector(`.rec-item[data-id="${id}"]`)).filter(Boolean);
    for (const el of rows) { el.classList.add("deleting"); el.setAttribute("aria-busy", "true"); }
    try {
      const r = await postJSON("/api/records/delete", { ids, history: hist.checked });
      toast("Deleted " + fmt(r.deleted_files) + " record" + (r.deleted_files === 1 ? "" : "s") + (r.deleted_rows ? " and " + fmt(r.deleted_rows) + " history rows" : "") + (r.released_prompts ? ", chain patched" : "") + (r.errors.length ? ". Some failed: " + r.errors[0] : ""));
      for (const id of ids) P.checked.delete(id);
      if (ids.includes(P.sel)) { P.sel = null; $("rec-detail").textContent = ""; $("rec-detail").append(h("div", { class: "empty", text: "Deleted." })); }
      const keep = !hist.checked && $("rec-all").checked;
      if (keep) {
        for (const el of rows) el.classList.remove("deleting");
        loadList(false);
      } else {
        await poof(rows);
        P.rows = P.rows.filter((x) => !ids.includes(x.row.request_id));
        P.total = Math.max(0, P.total - ids.length);
        $("rec-count").textContent = fmt(P.total) + ($("rec-all").checked ? " requests" : " saved records");
        updateBulk();
        if (!P.rows.length) loadList(false);
      }
    } catch (e) {
      for (const el of rows) { el.classList.remove("deleting"); el.removeAttribute("aria-busy"); }
      toast("Couldn't delete: " + e.message);
    }
  }
  /** Each row turns to a puff of smoke, then the rows below slide up. */
  function poof(rows) {
    if (!rows.length) return Promise.resolve();
    if (reduced()) { for (const el of rows) el.remove(); return Promise.resolve(); }
    return new Promise((done) => {
      for (const el of rows) {
        el.classList.remove("deleting");
        const r = el.getBoundingClientRect();
        const smoke = h("div", { class: "smoke", "aria-hidden": "true" });
        for (let i = 0; i < 9; i++) {
          const p = h("i");
          p.style.setProperty("--dx", ((i % 3) - 1) * (18 + Math.random() * 22) + "px");
          p.style.setProperty("--dy", -(10 + Math.random() * 34) + "px");
          p.style.setProperty("--d", (Math.random() * 120) + "ms");
          p.style.left = (10 + (i / 9) * 80) + "%";
          smoke.append(p);
        }
        el.style.height = r.height + "px";
        el.append(smoke);
        el.classList.add("poof");
        requestAnimationFrame(() => { el.classList.add("gone"); });
      }
      setTimeout(() => { for (const el of rows) el.remove(); done(); }, 760);
    });
  }

  // ================================================================ sessions
  function human(b) { return b < 1024 ? b + " B" : b < 1048576 ? (b / 1024).toFixed(1) + " KB" : b < 1073741824 ? (b / 1048576).toFixed(1) + " MB" : (b / 1073741824).toFixed(2) + " GB"; }
  async function showSessions(id) {
    const list = $("sessions-list"), det = $("session-detail");
    if (id) { list.hidden = true; det.hidden = false; return showSession(id); }
    list.hidden = false; det.hidden = true; list.textContent = "";
    list.append(h("div", { class: "empty", text: "Loading…" }));
    let r, st;
    try { [r, st] = await Promise.all([getJSON("/api/sessions"), getJSON("/api/chains/status")]); }
    catch (e) { list.textContent = ""; list.append(h("div", { class: "empty", text: e.message })); return; }
    list.textContent = "";
    const m = st.migration || {};
    if (m.running) list.append(h("div", { class: "banner" }, h("b", { text: "Folding earlier prompts into chains… " }), fmt(m.folded + m.skipped) + " of " + fmt(m.total) + " done, " + human(m.saved_bytes) + " saved so far. Originals stay in .pre-v0.0.2 until you free the space."));
    else if (st.pre_v2_bytes > 0) {
      const free = h("button", { type: "button", class: "small", text: "Free " + human(st.pre_v2_bytes) });
      free.addEventListener("click", async () => { free.disabled = true; try { const x = await postJSON("/api/chains/free-space", {}); toast("Freed " + human(x.freed_bytes)); showSessions(); } catch (e) { toast(e.message); free.disabled = false; } });
      list.append(h("div", { class: "banner" }, h("b", { text: "Migration done. " }), "The pre-0.0.3 originals of " + fmt(m.folded) + " prompts are still on disk, verified against the new chains. ", free));
    }
    const total = r.sessions.reduce((a, s) => a + s.saved_bytes, 0), full = r.sessions.reduce((a, s) => a + s.full_bytes, 0);
    $("sessions-sub").textContent = r.sessions.length ? fmt(r.sessions.length) + " sessions · " + human(total) + " saved of " + human(full) + " (" + (full ? Math.round(100 * total / full) : 0) + "%) by storing only what changed. Every " + st.snapshot_every + "nd link is a full snapshot." : "No sessions yet. Chains form as agents send prompts to a host with saving on.";
    if (!r.sessions.length) list.append(h("div", { class: "empty", text: "No sessions yet." }));
    for (const s of r.sessions) {
      const row = h("div", { class: "sess-row", role: "button", tabindex: "0" },
        h("div", { class: "sess-main" },
          h("div", { class: "ri-top" }, h("b", { text: (s.agent || "session") + (s.project ? " · " + s.project : "") }), span(when(s.latest || s.created), "ri-time")),
          h("div", { class: "ri-meta" }, s.host ? swatchText(hostName(s.host), hostColorVar(s.host)) : null, span(s.id, "label-sm mono"), s.missing ? span(s.missing + " erased", "tag err") : null, s.heads > 1 ? span(s.heads + " branches", "tag") : null)),
        h("div", { class: "sess-nums" }, h("b", { text: fmt(s.nodes) + " prompts" }), span(human(s.saved_bytes) + " saved of " + human(s.full_bytes), "label-sm")));
      row.addEventListener("click", () => { location.hash = "#/prompts/session/" + encodeURIComponent(s.id); });
      row.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); row.click(); } });
      list.append(row);
    }
  }
  async function showSession(id) {
    const det = $("session-detail"); det.textContent = "";
    det.append(h("div", { class: "empty", text: "Loading…" }));
    let s;
    try { s = await getJSON("/api/sessions/" + encodeURIComponent(id)); }
    catch (e) { det.textContent = ""; det.append(h("div", { class: "empty", text: e.message })); return; }
    det.textContent = "";
    det.append(h("div", { class: "row-end", style: "justify-content:flex-start" }, h("a", { href: "#/prompts/sessions", class: "btn small", text: "← All sessions" }), h("b", { text: (s.agent || "session") + (s.project ? " · " + s.project : "") }), span(s.id, "label-sm mono")));
    det.append(h("p", { class: "label-sm", text: fmt(s.nodes.length) + " prompts · " + s.heads.length + " branch tip" + (s.heads.length === 1 ? "" : "s") + " · " + human(s.saved_bytes) + " saved of " + human(s.full_bytes) + ". Each node's id is the hash of its parent's id and its own content, so a tip commits to its whole branch." }));
    det.append(chainView(s));
  }

  // A session chain as a flat timeline. The main line (root to the newest
  // tip) comes first, then each branch from where it forks. Every row shows
  // its link number instead of being indented by depth, and long straight
  // runs fold into one row you can open, a page at a time. Chains of
  // thousands of links stay readable and quick.
  const RUN_FOLD = 6, PAGE = 200;
  function chainView(s) {
    const byId = new Map(s.nodes.map((n) => [n.node, n]));
    const kids = new Map();
    for (const n of s.nodes) { const k = n.parent || ""; if (!kids.has(k)) kids.set(k, []); kids.get(k).push(n); }
    const pathTo = (id, stop) => { const out = []; let cur = byId.get(id); while (cur && !(stop && stop.has(cur.node))) { out.push(cur); cur = cur.parent ? byId.get(cur.parent) : null; } return out.reverse(); };
    const newest = s.heads[0] || (s.nodes.length ? s.nodes.reduce((a, b) => (a.created > b.created ? a : b)).node : null);
    const placed = new Set();
    const segments = [];
    if (newest) { const main = pathTo(newest); main.forEach((n) => placed.add(n.node)); segments.push({ title: "Main line", fork: null, nodes: main }); }
    s.heads.slice(1).forEach((hd, i) => {
      const seg = pathTo(hd, placed);
      if (!seg.length) return;
      seg.forEach((n) => placed.add(n.node));
      segments.push({ title: "Branch " + (i + 2), fork: seg[0].parent ? byId.get(seg[0].parent) : null, nodes: seg });
    });
    const rest = s.nodes.filter((n) => !placed.has(n.node)).sort((a, b) => (a.created < b.created ? -1 : 1));
    if (rest.length) segments.push({ title: "Erased ends", fork: null, nodes: rest });

    const wrap = h("div", { class: "chain" });
    for (const seg of segments) {
      const head = h("div", { class: "chain-seg-head" }, h("b", { text: seg.title }), span(fmt(seg.nodes.length) + " link" + (seg.nodes.length === 1 ? "" : "s"), "label-sm"),
        seg.fork ? span("forks from link " + fmt(seg.fork.depth + 1), "tag") : null);
      wrap.append(head);
      const list = h("div", { class: "chain-list" + (seg.fork ? " branch" : "") });
      renderRuns(list, seg.nodes, s, kids);
      wrap.append(list);
    }
    return wrap;
  }
  function plain(n, s, kids) { return !n.missing && !s.heads.includes(n.node) && (kids.get(n.node) || []).length <= 1; }
  function renderRuns(list, nodes, s, kids) {
    let i = 0;
    while (i < nodes.length) {
      let j = i;
      while (j < nodes.length && plain(nodes[j], s, kids)) j++;
      const run = nodes.slice(i, j);
      if (run.length >= RUN_FOLD) {
        // Keep the ends of a long straight run visible; fold the middle.
        list.append(linkRow(run[0], s, kids));
        list.append(foldRow(run.slice(1, -1), s, kids));
        list.append(linkRow(run[run.length - 1], s, kids));
      } else run.forEach((n) => list.append(linkRow(n, s, kids)));
      if (j < nodes.length) list.append(linkRow(nodes[j], s, kids));
      i = j + 1;
    }
  }
  function foldRow(run, s, kids) {
    const fresh = run.reduce((a, n) => a + Math.max(0, n.size - n.prefix), 0), full = run.reduce((a, n) => a + n.size, 0);
    const first = run[0], last = run[run.length - 1];
    const row = h("button", { type: "button", class: "chain-fold" },
      span("⋮", "glyph"),
      h("span", {}, h("b", { text: "Links " + fmt(first.depth + 1) + "–" + fmt(last.depth + 1) }), span(" · " + fmt(run.length) + " in a straight line · " + human(fresh) + " new of " + human(full) + " · " + when(first.created) + " to " + when(last.created), "label-sm")),
      span("Show", "chain-fold-cta"));
    row.addEventListener("click", () => {
      const frag = document.createDocumentFragment();
      const shown = run.slice(0, PAGE);
      shown.forEach((n) => frag.append(linkRow(n, s, kids)));
      if (run.length > PAGE) frag.append(foldRow(run.slice(PAGE), s, kids));
      row.replaceWith(frag);
    });
    return row;
  }
  function linkRow(n, s, kids) {
    const k = (kids.get(n.node) || []).length;
    const head = s.heads.includes(n.node);
    const name = n.stem ? n.stem.split("/").pop() : n.content.slice(0, 12);
    return h("div", { class: "chain-link" + (n.missing ? " missing" : "") + (n.snapshot ? " snap" : "") + (head ? " head" : "") },
      span(fmt(n.depth + 1), "chain-num"),
      span(n.missing ? "✕" : n.snapshot ? "◆" : "●", "glyph"),
      h("div", { class: "sn-main" },
        h("div", {}, n.request_id ? h("a", { href: "#/prompts/" + n.request_id, text: name }) : span(name, ""), n.missing ? span(" erased", "tag err") : null, k > 1 ? span(" " + k + " branches", "tag") : null, head ? span(" tip", "tag") : null),
        span(when(n.created) + " · " + (n.snapshot ? "full copy, " + human(n.size) : human(Math.max(0, n.size - n.prefix)) + " new of " + human(n.size) + " (" + (n.size ? Math.round(100 * n.prefix / n.size) : 0) + "% repeated)") + " · " + n.node.slice(0, 10), "label-sm")));
  }

  // ================================================================ chat
  function continueInChat(id,row) { PipChat.continueRecord(id,row); }

  // ================================================================ page
  function setMode(v, sessionId) {
    for (const b of $("prompts-mode").children) b.setAttribute("aria-pressed", String(b.dataset.v === v));
    $("records-view").hidden = v !== "records";
    $("sessions-view").hidden = v !== "sessions";
    $("chat-view").hidden = v !== "chat";
    const want = v === "chat" ? "#/prompts/chat" : v === "sessions" ? (sessionId ? "#/prompts/session/" + encodeURIComponent(sessionId) : "#/prompts/sessions") : P.sel ? "#/prompts/" + P.sel : "#/prompts";
    if (location.hash !== want) history.replaceState(null, "", want);
    if (v === "chat") PipChat.showPage();
    if (v === "sessions") showSessions(sessionId);
  }
  async function show(sub) {
    if (!S.meta) return;
    renderBanner();
    fillFilters();
    if (sub === "chat") { setMode("chat"); return; }
    if (sub === "sessions") { setMode("sessions"); return; }
    if (sub && sub.startsWith("session/")) { setMode("sessions", decodeURIComponent(sub.slice(8))); return; }
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
    for (const id of ["rec-host", "rec-model", "rec-range", "rec-sort", "rec-outcome", "rec-starred", "rec-all"]) $(id).addEventListener("change", () => loadList(false));
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

  }
  function filter(signal){$("rec-outcome").value=signal;$("rec-all").checked=true;location.hash="#/prompts";setMode("records");loadList(false);}
  return { init, show, refresh, filter };
})();


