// SPDX-License-Identifier: Apache-2.0
// Toktail portal: the Dashboard page. Hosts strip, hero, charts, insights,
// the cloud cost comparison and the tables, for one host or all of them.
"use strict";

const Dashboard = (() => {
  // ================================================================ data
  function rangeDates() {
    const today = S.meta ? S.meta.today : isoD(new Date());
    switch (S.range) {
      case "today": return [today, today];
      case "yesterday": { const y = addDays(today, -1); return [y, y]; }
      case "7": return [addDays(today, -6), today];
      case "30": return [addDays(today, -29), today];
      case "90": return [addDays(today, -89), today];
      case "mtd": return [today.slice(0, 8) + "01", today];
      case "all": return [S.meta && S.meta.first_date ? S.meta.first_date : today, today];
      case "custom": {
        const f = $("from").value || addDays(today, -29), t = $("to").value || today;
        return f <= t ? [f, t] : [t, f];
      }
    }
    return [addDays(today, -29), today];
  }
  function rangeName() {
    const [f, t] = rangeDates();
    return { today: "today", yesterday: "yesterday", "7": "in the last 7 days", "30": "in the last 30 days", "90": "in the last 90 days", mtd: "this month", all: "since you started counting" }[S.range]
      || ("from " + dayLabel(f) + " to " + dayLabel(t));
  }
  function selectedModels() {
    if (!S.meta) return null;
    const all = S.meta.models.map((m) => m.model);
    const sel = all.filter((m) => !S.hidden.has(m));
    return sel.length === all.length ? null : sel;
  }
  function qs(extra) {
    const [from, to] = rangeDates();
    const p = new URLSearchParams({ from, to, ...(extra || {}) });
    const m = selectedModels();
    if (m) p.set("models", m.join("|"));
    if (S.hosts && S.hosts.size) p.set("hosts", [...S.hosts].join("|"));
    return p.toString();
  }
  function scopeName() {
    if (!S.hosts || !S.hosts.size) return S.meta && S.meta.hosts.filter((x) => !x.removed).length > 1 ? "Your hosts" : "Your host";
    if (S.hosts.size === 1) return hostName([...S.hosts][0]);
    return [...S.hosts].map(hostName).join(" + ");
  }

  async function load() {
    document.body.classList.add("loading");
    try {
      S.meta = await getJSON("/api/meta");
      assignColors();
      if (S.hosts) for (const id of [...S.hosts]) if (!S.meta.hosts.some((x) => x.id === id)) S.hosts.delete(id);
      const q = qs();
      const key = q + "|" + S.metric + "|" + S.split;
      S.animate = key !== S.dataKey && !reduced();
      S.dataKey = key;
      const [daily, heat, recent, repeats, costs, signals, budgets] = await Promise.all([
        getJSON("/api/daily?" + q),
        getJSON("/api/heatmap?" + q),
        getJSON("/api/recent?" + qs({ limit: 300 })),
        getJSON("/api/repeats?" + qs({ limit: 50 })),
        S.meta.costs_enabled ? getJSON("/api/costs?" + q) : Promise.resolve(null),
        getJSON("/api/signals?" + q),
        getJSON("/api/budgets").catch(() => null),
      ]);
      Object.assign(S, { daily, heat, recent, repeats, costs, signals, budgets });
      render();
    } catch (e) {
      toast("Couldn't load your data: " + e.message);
    } finally {
      document.body.classList.remove("loading");
    }
  }

  /** Groups the daily entries by day and by series (model or host). */
  function derive() {
    const [from, to] = rangeDates();
    const days = [];
    for (let d = from; d <= to; d = addDays(d, 1)) days.push(d);
    const byHost = S.split === "host";
    const keyOf = (e) => (byHost ? e.host : e.model);
    const order = byHost ? S.meta.hosts.map((x) => x.id) : S.meta.models.map((m) => m.model).filter((m) => !S.hidden.has(m));
    const present = new Set(S.daily.map(keyOf));
    let series = order.filter((k) => present.has(k));
    for (const k of present) if (!series.includes(k)) series.push(k);
    const byDay = new Map(days.map((d) => [d, new Map()]));
    const blank = () => ({ input: 0, output: 0, requests: 0, gen_out: 0, gen_ms: 0, errors: 0, estimated: 0, cache_hits: 0, cache_saved: 0, cache_read: 0, cache_write: 0 });
    const tot = blank();
    const perSeries = new Map();
    for (const e of S.daily) {
      const k = keyOf(e);
      for (const f in tot) tot[f] += e[f] || 0;
      const ps = perSeries.get(k) || blank();
      for (const f in ps) ps[f] += e[f] || 0;
      perSeries.set(k, ps);
      if (!byDay.has(e.date)) continue;
      const dm = byDay.get(e.date);
      const cell = dm.get(k) || blank();
      for (const f in cell) cell[f] += e[f] || 0;
      dm.set(k, cell);
    }
    const allSeries = [...series], allByDay = new Map([...byDay].map(([d,m]) => [d,new Map(m)]));
    if (series.length > SLOTS) {
      series.sort((a,b) => mv(perSeries.get(b),S.metric)-mv(perSeries.get(a),S.metric));
      const rest = series.slice(SLOTS - 1), other = blank();
      for (const k of rest) for (const f in other) other[f] += perSeries.get(k)[f] || 0;
      perSeries.set("__other__",other);
      for (const m of byDay.values()) {
        const cell = blank(); for (const k of rest) { const e=m.get(k); if(e) { for(const f in cell) cell[f]+=e[f]||0; m.delete(k); } }
        if (cell.requests) m.set("__other__",cell);
      }
      series=series.slice(0,SLOTS - 1).concat("__other__");
    }
    const dayTotal = (d, key) => { let s = 0; for (const e of byDay.get(d).values()) s += mv(e, key); return s; };
    return { from, to, days, series, byDay, tot, perSeries, dayTotal, byHost, allSeries, allByDay };
  }
  const sColor = (D, k) => (k === "__other__" ? css("--other") : D.byHost ? hostColor(k) : color(k));
  const sVar = (D, k) => (k === "__other__" ? "--other" : D.byHost ? hostColorVar(k) : colorVar(k));
  const sName = (D, k) => (k === "__other__" ? (D.byHost?"Other hosts":"Other models") : D.byHost ? hostName(k) : k);

  // ================================================================ render
  function render() {
    const D = derive();
    S.lastData = D;
    renderHosts();
    renderFilters();
    renderHero(D);
    renderFeed();
    renderBudgets();
    renderDaily(D);
    renderInsights(D);
    renderCosts(D);
    renderShare(D);
    renderSpeed(D);
    renderCumulative(D);
    renderHeat(D);
    renderTable(D);
    renderFoot();
    markMetric();
  }

  // ---------------------------------------------------------------- hosts strip
  const STATE_WORD = { idle: "napping", processing: "munching", stampede: "stampede!" };
  function renderHosts() {
    const box = $("hosts-strip");
    const hosts = S.meta.hosts.filter((x) => !x.removed || x.all_time.requests > 0);
    box.hidden = false;
    box.textContent = "";
    const all = !S.hosts || !S.hosts.size;
    const sum = (k) => hosts.reduce((s, x) => s + mv(x[k], "combined"), 0);
    const tile = (id, name, opts) => {
      const t = h("button", { type: "button", class: "host-tile card" + (opts.selected ? " sel" : "") + (opts.removed ? " removed" : ""), "data-host": id, "aria-pressed": String(!!opts.selected), title: opts.title || "" });
      const head = h("div", { class: "ht-head" },
        opts.dotClass !== undefined ? h("span", { class: "ht-dot " + opts.dotClass }) : null,
        h("span", { class: "ht-name", text: name }),
        opts.swatch ? h("i", { class: "ht-sw", style: "background:var(" + opts.swatch + ")" }) : null);
      const live = h("div", { class: "ht-live" }, h("span", { class: "ht-tps", "data-tps": id, text: "—" }), h("span", { class: "ht-state", "data-state": id, text: "" }));
      const spark = h("div", { class: "ht-spark", "data-spark": id });
      const nums = h("div", { class: "ht-nums" },
        h("div", {}, h("b", { text: compact(opts.today) }), span(" today", "label-sm")),
        h("div", {}, h("b", { text: compact(opts.week) }), span(" in 7 days", "label-sm")));
      t.append(head, live, spark, nums);
      if (opts.note) t.append(h("div", { class: "ht-note", text: opts.note }));
      return t;
    };
    const healthy = S.meta.hosts.filter((x) => x.enabled && x.proxy_ok && x.lmstudio && x.lmstudio.ok).length;
    const enabled = S.meta.hosts.filter((x) => x.enabled).length;
    box.append(tile("", "All hosts", { selected: all, today: sum("today"), week: sum("week"), title: "Combined results from every host", dotClass: healthy === enabled ? "ok" : "warn" }));
    for (const x of hosts) {
      const ok = x.enabled && x.proxy_ok && x.lmstudio && x.lmstudio.ok;
      const note = x.removed ? "removed from Settings" : !x.enabled ? "turned off" : !x.proxy_ok ? x.proxy_message : x.lmstudio && !x.lmstudio.ok ? x.lmstudio.message : "";
      box.append(tile(x.id, x.name, {
        selected: !all && S.hosts.has(x.id), today: mv(x.today, "combined"), week: mv(x.week, "combined"),
        dotClass: x.removed || !x.enabled ? "" : ok ? "ok" : "bad", swatch: hostColorVar(x.id), removed: x.removed, note,
        title: x.removed ? "" : "Apps reach " + x.name + " through " + (x.listen ? "http://localhost:" + x.listen.split(":").pop() + "/v1" : "") + "\nShift-click to add to the selection",
      }));
    }
    updateLive(S.live);
  }
  function selectHost(id, additive) {
    if (!id) S.hosts = null;
    else if (additive) {
      S.hosts = S.hosts || new Set();
      if (S.hosts.has(id)) S.hosts.delete(id); else S.hosts.add(id);
      if (!S.hosts.size) S.hosts = null;
    } else S.hosts = S.hosts && S.hosts.size === 1 && S.hosts.has(id) ? null : new Set([id]);
    store.set("hosts", S.hosts ? [...S.hosts].join("|") : "");
    load();
  }
  /** Live numbers on the host tiles, every second. */
  function updateLive(L) {
    if (!L || !S.meta) return;
    const set = (id, tps, state, per) => {
      const t = document.querySelector(`[data-tps="${id}"]`);
      if (!t) return;
      t.textContent = tps >= 0.5 ? compact(tps) + " tok/s" : "0 tok/s";
      const s = document.querySelector(`[data-state="${id}"]`);
      s.textContent = STATE_WORD[state] || "";
      s.dataset.s = state;
      const sp = document.querySelector(`[data-spark="${id}"]`);
      sp.textContent = "";
      sp.append(sparkline((per || []).slice(-60), 140, 26, id ? hostColor(id) : css("--lav"), id ? hostColor(id) : css("--lav")));
    };
    set("", L.tokens_per_sec, L.state, L.per_second);
    for (const id in L.hosts || {}) { const x = L.hosts[id]; set(id, x.tokens_per_sec, x.state, x.per_second); }
    const v = $("stat-live"), d = $("stat-live-d");
    if (v) {
      const scoped = S.hosts && S.hosts.size ? [...S.hosts].map((id) => (L.hosts || {})[id]).filter(Boolean) : null;
      const tps = scoped ? scoped.reduce((s, x) => s + x.tokens_per_sec, 0) : L.tokens_per_sec;
      const act = scoped ? scoped.reduce((s, x) => s + x.active_requests, 0) : L.active_requests;
      v.textContent = compact(tps) + "/s";
      d.textContent = act ? act + " request" + (act === 1 ? "" : "s") + " in flight" : tps > 0.5 ? "over the last few seconds" : "nothing streaming";
    }
  }

  // ---------------------------------------------------------------- right now: flights and the feed
  /** Takes the activity from a /api/live poll (any page), so nothing is missed. */
  function ingestLive(L) {
    const a = L && L.activity;
    if (!a) return;
    S.flights = a.in_flight || [];
    S.stacks = a.stacks || [];
    const fresh = (a.finished || []).filter((f) => f.seq > (S.liveSeq || 0));
    if (fresh.length) {
      S.feed = fresh.reverse().map((f) => ({ ...f, isNew: S.liveSeq > 0 })).concat((S.feed || []).map((f) => ({ ...f, isNew: false }))).slice(0, 60);
      S.liveSeq = a.seq;
    }
    if (a.seq < (S.liveSeq || 0)) S.liveSeq = a.seq; // Toktail restarted
    if (S.page === "dashboard") renderFeed(!fresh.length);
  }
  const inScope = (x) => !S.hosts || !S.hosts.size || S.hosts.has(x.host);
  // ---- requests in flight you can steer: cancel, reroute, or answer with an
  // identical one already running. Rows are kept and updated in place so a
  // click is never lost to the once-a-second refresh.
  S.flightSel = S.flightSel || new Set();
  let cancelArm = null; // { id, until }: the second click confirms
  const mmss = (s) => Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0");
  const ago = (ms) => ms >= 3600e3 ? (ms / 3600e3).toFixed(1) + " h" : ms >= 60e3 ? mmss(Math.round(ms / 1000)) : (ms / 1000).toFixed(1) + " s";
  async function flightAction(path, body, okText) {
    try {
      const r = await fetch(path, { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body || {}) });
      const v = await r.json().catch(() => ({}));
      if (!r.ok || v.ok === false) throw new Error(v.message || (v.errors && v.errors[0]) || "HTTP " + r.status);
      toast(okText || v.message || "Done");
      return v;
    } catch (e) { toast(e.message); return null; }
  }
  /** First click arms, second confirms. Stays armed while the pointer is on
   *  the button, and for 4 s after it leaves. */
  function cancelFlight(f, btn) {
    if (cancelArm && cancelArm.id === f.id && (cancelArm.over || cancelArm.until > Date.now())) {
      cancelArm = null;
      btn.disabled = true;
      btn.textContent = "Cancelling…";
      flightAction("/api/flights/" + f.id + "/cancel", null, "Cancelled #" + f.id + (f.agent ? " (" + f.agent + ")" : ""));
      return;
    }
    const arm = { id: f.id, until: Date.now() + 4000, over: btn.matches(":hover") };
    cancelArm = arm;
    btn.textContent = "Sure? Cancel";
    btn.classList.add("armed");
    const disarm = () => {
      if (cancelArm !== arm || arm.over || arm.until > Date.now()) return;
      cancelArm = null;
      if (!btn.disabled) { btn.textContent = "Cancel"; btn.classList.remove("armed"); }
    };
    btn.onpointerenter = () => { arm.over = true; };
    btn.onpointerleave = () => { arm.over = false; arm.until = Date.now() + 4000; setTimeout(disarm, 4050); };
    setTimeout(disarm, 4050);
  }
  /** Asks where to send one or more requests, then sends them there. */
  async function rerouteFlights(list) {
    const endpoint = list[0].endpoint;
    let t;
    try { t = await getJSON("/api/flights/targets?endpoint=" + encodeURIComponent(endpoint)); } catch (e) { return toast(e.message); }
    const hosts = t.hosts || [];
    const here = new Set(list.map((f) => f.host));
    const pick = hosts.find((x) => x.available && !here.has(x.id)) || hosts.find((x) => x.available);
    if (!pick) return toast("No other host can take " + endpoint.replace(/^\/v1\//, "") + " right now.");
    const host = h("select", { "aria-label": "Host" });
    for (const x of hosts) host.append(h("option", { value: x.id, text: x.name + (x.public ? " (API)" : "") + (x.available ? "" : " — " + x.reason), disabled: !x.available }));
    host.value = pick.id;
    // The model it runs on there: only one that machine has. A host that
    // doesn't list its models takes a typed name instead.
    const model = h("select", { "aria-label": "Model" });
    const typed = h("input", { type: "text", class: "wide", spellcheck: "false", "aria-label": "Model name", hidden: true });
    const note = h("div", { class: "label-sm reroute-note" });
    const asked = [...new Set(list.map((f) => f.model))];
    const chosen = () => (model.hidden ? typed.value : model.value).trim();
    const fill = () => {
      const x = hosts.find((y) => y.id === host.value) || {};
      const ms = x.models || [];
      model.textContent = "";
      model.hidden = !ms.length;
      typed.hidden = !!ms.length;
      for (const m of ms) model.append(h("option", { value: m, text: m + (asked.includes(m) ? "  (the model it asked for)" : "") }));
      const same = asked.find((m) => ms.includes(m));
      if (ms.length) model.value = same || ms[0];
      else typed.placeholder = "The model's name on " + x.name;
      note.textContent = !ms.length
        ? x.name + " didn't list its models. Type one it has."
        : same ? "" : x.name + " doesn't have " + asked.join(", ") + ", so it will run on " + model.value + " there. Pick another if you like.";
    };
    model.addEventListener("change", () => { note.textContent = asked.includes(model.value) ? "" : "It asked for " + asked.join(", ") + "; it will run on " + model.value + " instead."; });
    fill();
    host.addEventListener("change", fill);
    const body = h("div", { class: "reroute" },
      h("p", { text: list.length === 1
        ? "#" + list[0].id + " (" + list[0].model + ") is still reading its prompt on " + hostName(list[0].host) + ". Send it to another machine and model; the agent gets that answer on the same request, as if nothing happened."
        : "Send these " + list.length + " requests to another machine and model; each agent gets its answer on the same request, as if nothing happened." }),
      h("label", { class: "stack" }, span("Host", "label-sm"), host),
      h("label", { class: "stack" }, span("Model on that host", "label-sm"), model, typed),
      note,
      h("p", { class: "label-sm", text: "The agent's own API key is never sent to another host: the target's stored key is used, or none. The work started on " + [...here].map(hostName).join(", ") + " is stopped." }));
    const go = await modal(list.length === 1 ? "Reroute #" + list[0].id : "Reroute " + list.length + " requests", body, [{ label: "Cancel", value: false }, { label: "Reroute", value: true, cls: "primary", validate: () => { if (!chosen()) { typed.focus(); return false; } return true; } }]);
    if (!go) return;
    const target = chosen();
    if (list.length === 1) await flightAction("/api/flights/" + list[0].id + "/reroute", { host: host.value, model: target }, "Rerouted #" + list[0].id + " to " + target + " on " + hostName(host.value));
    else {
      const r = await flightAction("/api/flights/bulk", { ids: list.map((f) => f.id), action: "reroute", host: host.value, model: target }, "Rerouting…");
      if (r) toast("Rerouted " + r.done + " of " + list.length + (r.errors.length ? ". " + r.errors[0] : ""));
      S.flightSel.clear();
    }
  }
  function flightRow(id) {
    const cb = h("input", { type: "checkbox", class: "fl-cb", "aria-label": "Select #" + id });
    const host = h("b"), model = h("code"), idl = span("#" + id, "fl-id");
    const tags = h("span", { class: "fl-tags" });
    const nums = h("div", { class: "nums" });
    const state = h("div", { class: "state" });
    const acts = h("div", { class: "fl-acts" });
    const row = h("div", { class: "flight", "data-id": String(id) },
      h("div", { class: "who" }, cb, idl, host, model, tags), nums, h("div", { class: "bar" }, h("i")), state, acts);
    cb.addEventListener("change", () => { if (cb.checked) S.flightSel.add(id); else S.flightSel.delete(id); paintBulk(); });
    row._parts = { cb, host, model, tags, nums, state, acts };
    return row;
  }
  function paintFlight(row, f) {
    const p = row._parts;
    const pre = f.output === 0;
    row.classList.toggle("prefill", pre);
    row.classList.toggle("gone", !!f.agent_gone);
    row.classList.toggle("joined", f.joined_to != null);
    row.classList.toggle("sel", S.flightSel.has(f.id));
    row.style.setProperty("--hc", hostColor(f.host));
    p.cb.checked = S.flightSel.has(f.id);
    p.host.textContent = hostName(f.host);
    p.model.textContent = f.model;
    const tags = [];
    if (f.agent) tags.push(span(f.agent + (f.session ? " · " + f.session.slice(-6) : ""), "tag agent", ));
    if (f.duplicate_of != null && f.joined_to == null) tags.push(span("retry of #" + f.duplicate_of, "tag warn"));
    if (f.joined_to != null) tags.push(span(f.joined_finished ? "gets #" + f.joined_to + "'s finished answer" : "gets #" + f.joined_to + "'s answer · not read again", "tag hit"));
    else if (f.queue_len > 1 && f.output === 0) tags.push(span(f.queue_pos === 1 ? "first in line" : "in line: " + f.queue_pos + " of " + f.queue_len, f.queue_pos === 1 ? "tag" : "tag warn"));
    if (f.joiners) tags.push(span("answering " + f.joiners + " more", "tag hit"));
    if (f.rerouted_from) tags.push(span("rerouted from " + f.rerouted_from, "tag"));
    if (f.agent_gone) tags.push(span(f.stops_in_secs != null ? "agent hung up · stops in " + mmss(f.stops_in_secs) + " unless it retries" : "agent hung up", "tag err"));
    p.tags.replaceChildren(...tags);
    if (pre) p.nums.replaceChildren(span(f.joined_to != null ? "waiting on #" + f.joined_to + "…" : "reading the prompt…", "state"));
    else p.nums.replaceChildren(document.createTextNode(fmt(f.output)), h("small", { text: " tokens" + (f.tokens_per_sec ? " · " + f.tokens_per_sec.toFixed(1) + " tok/s" : "") }));
    p.state.textContent = ago(f.elapsed_ms) + (f.output === 0 && f.prompt_tokens_est > 2000 && f.joined_to == null ? " · ~" + compact(f.prompt_tokens_est) + " tokens of prompt to read" : "") + (f.ttft_ms != null ? " · first token after " + fmt(f.ttft_ms) + " ms" : "") + " · " + f.endpoint.replace(/^\/(v1|api\/v\d)\//, "");
    // Actions: rebuilt only when what's possible changes.
    const sig = [f.can_reroute, f.duplicate_of, f.joined_to].join("|");
    if (p.acts.dataset.sig !== sig) {
      p.acts.dataset.sig = sig;
      const list = [];
      if (f.can_reroute && f.duplicate_of != null) list.push(h("button", { type: "button", class: "small", title: "Stop this one and give its agent #" + f.duplicate_of + "'s answer when it comes", text: "Use #" + f.duplicate_of + "'s answer", onclick: () => flightAction("/api/flights/" + f.id + "/join", { to: f.duplicate_of }) }));
      if (f.can_reroute) list.push(h("button", { type: "button", class: "small", text: "Reroute…", onclick: () => { const cur = (S.flights || []).find((x) => x.id === f.id); if (cur) rerouteFlights([cur]); } }));
      const cancel = h("button", { type: "button", class: "small danger", text: "Cancel", title: "Stop it on the model server and tell the agent it was cancelled (agents don't retry this)" });
      cancel.addEventListener("click", () => cancelFlight(f, cancel));
      list.push(cancel);
      p.acts.replaceChildren(...list);
    }
  }
  // ---- the diagnosis: what's piling up, why, and what Toktail is doing
  const seenStacks = new Set();
  function stackNote(st) {
    const host = hostName(st.host);
    const box = h("div", { class: "stack-why" });
    const head = st.line.length > 1
      ? host + " is reading " + st.line.length + " prompts for " + st.model + ", one at a time"
      : host + ": an agent sent the same prompt again";
    box.append(h("b", { text: head }));
    for (const g of st.groups) {
      const who = g.agent || "The agent";
      const copies = g.attempts || g.answered_by_original.length + g.running_separately.length + 1;
      const size = g.prompt_tokens_est > 2000 ? "~" + compact(g.prompt_tokens_est) + "-token " : "";
      const latest = g.answered_by_original.length ? Math.max(...g.answered_by_original) : null;
      const p = h("p", {});
      p.append(h("strong", { text: who + " sent the same " + size + "prompt " + copies + " times. " }));
      if (g.original_finished) p.append("#" + g.original + " finished after " + who + " stopped waiting for it; Toktail kept the answer and is handing it to the retry (#" + latest + "). Nothing is read again.");
      else if (g.original_gone && latest != null) p.append(who + " stopped waiting for #" + g.original + " while it was still being read, and sent it again. #" + g.original + " keeps going, and when it's done its answer goes to the latest attempt (#" + latest + "). The " + (g.answered_by_original.length === 1 ? "retry isn't" : g.answered_by_original.length + " retries aren't") + " read again.");
      else if (latest != null) p.append("Toktail sends #" + g.original + "'s answer to " + (g.answered_by_original.length === 1 ? "the copy" : "all " + g.answered_by_original.length + " copies") + " when it's done; they aren't read again.");
      if (g.running_separately.length) {
        p.append(" " + g.running_separately.map((x) => "#" + x).join(", ") + (g.running_separately.length === 1 ? " is" : " are") + " being read again on " + (g.running_separately.length === 1 ? "its" : "their") + " own. ");
        const b = h("button", { type: "button", class: "small primary", text: "Answer " + (g.running_separately.length === 1 ? "it" : "them") + " with #" + g.original + "'s answer" });
        b.addEventListener("click", () => { b.disabled = true; flightAction("/api/flights/" + g.original + "/collapse"); });
        p.append(b);
      }
      box.append(p);
    }
    const waiting = st.line.slice(1);
    if (waiting.length) {
      const p = h("p", {});
      const behind = (S.flights || []).filter((f) => waiting.includes(f.id)).reduce((a, f) => a + (f.prompt_tokens_est || 0), 0);
      p.append(waiting.length + " other prompt" + (waiting.length === 1 ? " is" : "s are") + " waiting behind #" + st.line[0] + (behind > 2000 ? " (~" + compact(behind) + " more tokens to read)" : "") + "; LM Studio will read " + (waiting.length === 1 ? "it" : "them") + " after it. ");
      const movable = (S.flights || []).filter((f) => waiting.includes(f.id) && f.can_reroute);
      if (movable.length) {
        const b = h("button", { type: "button", class: "small", text: "Send the waiting " + (movable.length === 1 ? "one" : movable.length) + " to another machine…" });
        b.addEventListener("click", () => { const now = (S.flights || []).filter((f) => waiting.includes(f.id) && f.can_reroute); if (now.length) rerouteFlights(now); });
        p.append(b);
      }
      box.append(p);
    }
    box.append(h("p", { class: "label-sm", text: "Why: LM Studio reads one prompt at a time per model on a machine. It can write several answers at once, but coding agents send long prompts for short answers, so parallel requests don't help: each one waits for the reading before it. When an agent times out and sends the same prompt again, the copy joins the back of the line and would be read all over again. Toktail doesn't cause the wait; it keeps the line from growing." }));
    return box;
  }
  function paintWhy() {
    const why = $("flight-why");
    if (!why) return;
    const stacks = (S.stacks || []).filter((st) => inScope({ host: st.host }));
    const sig = JSON.stringify(stacks.map((st) => [st.host, st.model, st.line, st.groups.map((g) => [g.original, g.attempts, g.answered_by_original, g.running_separately, g.original_gone, g.original_finished])]));
    if (why.dataset.sig === sig) return;
    why.dataset.sig = sig;
    why.hidden = !stacks.length;
    why.replaceChildren(...stacks.map(stackNote));
    // Pip says it once per stack, in plain words.
    for (const st of stacks) for (const g of st.groups) {
      if (seenStacks.has(g.original) || !g.answered_by_original.length) continue;
      seenStacks.add(g.original);
      if (typeof Stage !== "undefined") Stage.cheer((g.agent || "Your agent") + " asked the same thing again. I'll read it once and hand over the answer.");
    }
  }
  function paintBulk() {
    const bar = $("flight-bulk");
    if (!bar) return;
    const live = new Set((S.flights || []).map((f) => f.id));
    for (const id of [...S.flightSel]) if (!live.has(id)) S.flightSel.delete(id);
    const n = S.flightSel.size;
    bar.hidden = n === 0;
    if (!n) return;
    const sel = (S.flights || []).filter((f) => S.flightSel.has(f.id));
    const reroutable = sel.filter((f) => f.can_reroute);
    const sameApi = new Set(reroutable.map((f) => f.endpoint)).size <= 1;
    // Rebuilt only when something in it changes, so a click never straddles a rebuild.
    const sig = [...S.flightSel].sort().join(",") + "|" + reroutable.map((f) => f.id).join(",") + "|" + sameApi;
    if (bar.dataset.sig === sig) return;
    bar.dataset.sig = sig;
    bar.replaceChildren(
      span(n + " selected", ""),
      h("button", { type: "button", class: "small", text: "Reroute " + reroutable.length + "…", disabled: !reroutable.length || !sameApi, title: sameApi ? "" : "Pick requests to the same endpoint", onclick: () => rerouteFlights(reroutable) }),
      h("button", { type: "button", class: "small danger", text: "Cancel " + n, onclick: async () => {
        if (!(await confirmBox("Cancel " + n + " requests?", "Each is stopped on its model server and its agent is told it was cancelled.", "Cancel them", true))) return;
        const r = await flightAction("/api/flights/bulk", { ids: [...S.flightSel], action: "cancel" }, "Cancelling…");
        if (r) toast("Cancelled " + r.done + " of " + n + (r.errors.length ? ". " + r.errors[0] : ""));
        S.flightSel.clear(); paintBulk();
      } }),
      h("button", { type: "button", class: "small", text: "Clear", onclick: () => { S.flightSel.clear(); for (const c of document.querySelectorAll(".fl-cb")) c.checked = false; paintBulk(); renderFeed(true); } }));
  }
  /** Redraws the flights; the finished list too unless `flightsOnly`. While
   *  the pointer is over the list nothing moves: finished requests stay put,
   *  greyed out, and new ones join at the bottom. */
  function renderFeed(flightsOnly) {
    const fl = $("flights"), feed = $("feed");
    if (!fl) return;
    if (!fl.dataset.hoverWired) {
      fl.dataset.hoverWired = "1";
      fl.addEventListener("pointerenter", () => { S.flightHold = true; });
      fl.addEventListener("pointerleave", () => { S.flightHold = false; renderFeed(true); });
    }
    // Oldest first, each original followed by the retries it answers.
    const flights = (S.flights || []).filter(inScope).sort((a, b) => ((a.joined_to ?? a.id) - (b.joined_to ?? b.id)) || ((a.joined_to != null) - (b.joined_to != null)) || (a.id - b.id));
    const empty = fl.querySelector(":scope > .empty");
    if (!flights.length) {
      if (!empty) { fl.textContent = ""; fl.append(h("div", { class: "empty" }, h("img", { src: "/pip.svg", alt: "", width: 34, height: 34 }), span("Nothing streaming right now. Pip is listening.", ""))); }
    } else if (empty) empty.remove();
    const rows = new Map([...fl.querySelectorAll(":scope > .flight")].map((r) => [Number(r.dataset.id), r]));
    if (S.flightHold && rows.size) {
      for (const f of flights.slice(0, 40)) {
        let row = rows.get(f.id);
        if (!row) { row = flightRow(f.id); fl.insertBefore(row, fl.querySelector(":scope > .fl-more")); }
        rows.delete(f.id);
        paintFlight(row, f);
      }
      for (const r of rows.values()) if (!r.classList.contains("ended")) {
        r.classList.add("ended");
        r._parts.acts.replaceChildren(span("finished", "label-sm"));
        r._parts.cb.disabled = true;
      }
    } else {
      let prev = null;
      for (const f of flights.slice(0, 40)) {
        let row = rows.get(f.id);
        if (!row) row = flightRow(f.id);
        rows.delete(f.id);
        paintFlight(row, f);
        const want = prev ? prev.nextSibling : fl.firstChild;
        if (row !== want) fl.insertBefore(row, want);
        prev = row;
      }
      for (const r of rows.values()) r.remove();
    }
    let more = fl.querySelector(":scope > .fl-more");
    if (flights.length > 40) { if (!more) { more = h("div", { class: "label-sm fl-more" }); fl.append(more); } more.textContent = "and " + (flights.length - 40) + " more in flight"; }
    else if (more) more.remove();
    paintBulk();
    paintWhy();
    const waiting = flights.filter((f) => f.output === 0).length, dups = flights.filter((f) => f.duplicate_of != null || f.joined_to != null).length;
    $("live-sub").textContent = flights.length ? flights.length + " request" + (flights.length === 1 ? "" : "s") + " in flight" + (waiting ? ", " + waiting + " still reading the prompt" : "") + (dups ? ", " + dups + " retr" + (dups === 1 ? "y" : "ies") : "") + ". Cancel or reroute any of them." : "Requests streaming through your hosts, and the ones that just finished";
    if (!feed || S.feedPaused || (flightsOnly && feed.childElementCount)) return;
    feed.textContent = "";
    const items = (S.feed || []).filter(inScope).slice(0, 30);
    if (!items.length) feed.append(h("li", { class: "label-sm" }, span("", ""), span("Finished requests show up here as they land.", "what"), span("", "")));
    for (const f of items) {
      const t = new Date(f.finished_at);
      const tags = [];
      if (f.status >= 400 && !(f.status === 499 && f.note)) tags.push(span(f.status === 499 ? "client left" : String(f.status), "tag err"));
      if (f.note) tags.push(span(f.note, "tag" + (f.note.startsWith("cancelled") || f.note.startsWith("agent hung up") ? " err" : f.note.startsWith("answered by") ? " hit" : "")));
      else if (f.cache_hit) tags.push(span("cache hit", "tag hit"));
      if (f.agent) tags.push(span(f.agent, "tag agent"));
      if (f.outcome === "refused") tags.push(span("refused", "tag err"));
      else if (["structured", "substantial", "tool_call"].includes(f.signal)) tags.push(span(f.signal.replace("_", " "), "tag"));
      const what = h("span", { class: "what" }, swatchText(hostName(f.host), hostColorVar(f.host)), h("b", { text: f.model }), ...tags);
      const n = h("span", { class: "n" }, document.createTextNode(fmt(f.input + f.output)), h("small", { text: f.cache_hit ? " · not generated" : " · " + fmt(f.output) + " out" + (f.tokens_per_sec ? " · " + f.tokens_per_sec.toFixed(0) + " tok/s" : "") }));
      const li = h("li", { class: (f.isNew ? "new" : "") + (f.cache_hit ? " hit" : ""), title: fmt(f.input) + " in, " + fmt(f.output) + " out, " + (f.duration_ms / 1000).toFixed(2) + " s" + (f.ttft_ms != null ? ", first token " + fmt(f.ttft_ms) + " ms" : "") }, h("time", { text: t.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit" }) }), what, n);
      li.addEventListener("click", () => { location.hash = "#/prompts/" + f.request_id; });
      li.style.cursor = "pointer";
      feed.append(li);
      f.isNew = false;
    }
  }

  // ---------------------------------------------------------------- budgets
  function renderBudgets() {
    const box = $("budgets");
    if (!box) return;
    box.textContent = "";
    const list = (S.budgets && S.budgets.budgets) || [];
    const scoped = list.filter((b) => !b.host || !S.hosts || !S.hosts.size || S.hosts.has(b.host));
    const when = { day: "today", week: "this week", month: "this month" };
    const periods = [...new Set(list.map((b) => when[b.period] || b.period))];
    $("budgets-sub").textContent = list.length ? "Used " + (periods.length > 1 ? periods.slice(0, -1).join(", ") + " and " + periods[periods.length - 1] : periods[0]) + "; the tick is where the pace ends up" : "Token limits per day, week or month";
    if (!scoped.length) {
      box.append(h("div", { class: "empty" },
        h("p", { text: list.length ? "No budgets for this selection." : "No budgets yet. Set a daily, weekly or monthly limit and Pip will tap you on the shoulder at 80% and at 100%." }),
        list.length ? null : h("a", { class: "btn", href: "#/settings/budgets", text: "Set a budget" })));
      return;
    }
    for (const b of scoped) {
      const pct = Math.round(b.ratio * 100), proj = Math.min(1.25, b.projected_ratio);
      const row = h("div", { class: "budget-row " + b.state, title: fmt(b.used) + " of " + fmt(b.limit) + " " + b.metric + " tokens, " + b.period_start + " to " + b.period_end + ". At this pace: " + fmt(b.projected) + "." },
        h("div", { class: "bh" }, h("b", { text: b.name }), span(pct + "%", "")),
        h("div", { class: "budget-track", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(Math.min(100, pct)), "aria-label": b.name },
          h("i", { class: "used", style: "width:" + Math.min(100, b.ratio * 100).toFixed(1) + "%" }),
          b.projected > b.used ? h("i", { class: "proj", style: "left:" + Math.min(100, proj * 100).toFixed(1) + "%" }) : null),
        h("div", { class: "label-sm", text: compact(b.used) + " of " + compact(b.limit) + " " + b.metric + " · " + b.scope + " · pace " + compact(b.projected) + (b.state === "over" ? " · over the limit" : b.projected_ratio >= 1 ? " · on track to go over" : "") }));
      box.append(row);
    }
  }

  // ---------------------------------------------------------------- filters
  function renderFilters() {
    for (const b of $("ranges").children) b.setAttribute("aria-pressed", String(b.dataset.r === S.range));
    $("custom").classList.toggle("on", S.range === "custom");
    for (const b of $("metric").children) b.setAttribute("aria-pressed", String(b.dataset.m === S.metric));
    for (const b of $("split").children) b.setAttribute("aria-pressed", String(b.dataset.s === S.split));
    $("split").hidden = S.meta.hosts.length < 2;
    const chips = $("chips"), wasOpen = chips.querySelector("details")?.open;
    const oldSearch = chips.querySelector("input[type=search]")?.value || "";
    chips.textContent = "";
    const toggle = (model) => {
      if (S.hidden.has(model)) S.hidden.delete(model);
      else if (S.hidden.size < S.meta.models.length - 1) S.hidden.add(model);
      load();
    };
    // The busiest models as chips; the long tail behind "more".
    const SHOWN = S.meta.models.length > 12 ? 9 : 12;
    for (const m of S.meta.models.slice(0, SHOWN)) {
      const c = h("span", { class: "chip", tabindex: "0", role: "button", "aria-pressed": String(!S.hidden.has(m.model)), title: fmt(m.combined) + " combined tokens all time (" + fmt(m.input) + " in, " + fmt(m.output) + " out). Click to hide or show." });
      c.append(h("i", { class: "sw", style: "background:var(" + colorVar(m.model) + ")" }), document.createTextNode(m.model));
      c.onclick = () => toggle(m.model);
      c.onkeydown = (e) => { if (e.key === " " || e.key === "Enter") { e.preventDefault(); toggle(m.model); } };
      chips.append(c);
    }
    if (S.meta.models.length > SHOWN) {
      const rest = S.meta.models.length - SHOWN;
      const chosen = S.meta.models.filter((m) => !S.hidden.has(m.model)).length;
      const details = h("details", { class: "model-picker" });
      const summary = h("summary", { class: "chip more", text: "+" + rest + " more · " + chosen + " of " + S.meta.models.length + " shown" });
      details.open = !!wasOpen;
      const pop = h("div", { class: "model-popover" });
      const search = h("input", { type: "search", placeholder: "Find a model…", "aria-label": "Find a model" });
      search.value = oldSearch;
      const rows = h("div", { class: "model-options" });
      const draw = () => {
        rows.textContent = "";
        const q = search.value.toLowerCase();
        const models = S.meta.models.filter((m) => m.model.toLowerCase().includes(q));
        for (const m of models.slice(0, 120)) {
          const input = h("input", { type: "checkbox", "aria-label": m.model });
          input.checked = !S.hidden.has(m.model);
          input.onchange = () => toggle(m.model);
          rows.append(h("label", { class: "model-option" }, input, h("i", { class: "sw", style: "background:var(" + colorVar(m.model) + ")" }), h("span", { text: m.model }), h("small", { text: compact(m.combined) })));
        }
        if (models.length > 120) rows.append(h("p", { class: "label-sm", text: "Showing 120 of " + models.length + ". Narrow the search." }));
      };
      const all = h("button", { type: "button", class: "small", text: "Show all" });
      const top = h("button", { type: "button", class: "small", text: "Top 5 only" });
      all.onclick = () => { S.hidden.clear(); load(); };
      top.onclick = () => { S.hidden = new Set(S.meta.models.slice(5).map((m) => m.model)); load(); };
      search.oninput = draw; draw();
      pop.append(search, h("div", { class: "actions" }, all, top), rows);
      details.append(summary, pop);
      chips.append(details);
    }
  }

  /** Marks the cards the token toggle changes, so it's clear what it does. */
  function markMetric() {
    for (const card of document.querySelectorAll("#page-dashboard .card")) {
      let b = card.querySelector(":scope > .card-head .metric-badge");
      const aware = card.classList.contains("metric-aware");
      const fixed = card.dataset.fixed;
      if (!aware && !fixed) { if (b) b.remove(); continue; }
      if (!b) {
        b = h("span", { class: "metric-badge" });
        const head = card.querySelector(":scope > .card-head > div");
        if (head) head.append(b);
      }
      b.className = "metric-badge" + (aware ? " on" : "");
      b.textContent = aware ? "◆ " + METRIC_TITLE[S.metric] + " tokens" : fixed;
      b.title = aware ? "Follows the Combined / Output / Input switch above" : "Not affected by the Combined / Output / Input switch";
    }
  }

  // ---------------------------------------------------------------- hero
  /** An exact count from the server (a string, so trillions stay exact), or the chart sum. */
  function exact(field, fallback) {
    const v = S.signals && S.signals[field];
    try { return BigInt(v != null ? v : Math.round(fallback || 0)); } catch (e) { return BigInt(Math.round(fallback || 0)); }
  }
  const big = (b) => b.toLocaleString();
  function renderHero(D) {
    const key = S.metric;
    $("hero-lede").textContent = scopeName() + " rendered, " + rangeName() + ",";
    const numEl = $("hero-num");
    const fit = () => fitText(numEl, Math.min(112, Math.max(56, innerWidth * 0.085)), 30);
    const value = exact(key, mv(D.tot, key));
    if (value <= BigInt(Number.MAX_SAFE_INTEGER)) countUp(numEl, Number(value), 1100, fit);
    else { numEl.dataset.v = "0"; numEl.textContent = big(value); }
    numEl.title = big(value) + " " + METRIC_NAME[key] + " tokens, exactly";
    fit();
    const inp = exact("input", D.tot.input), out = exact("output", D.tot.output), comb = exact("combined", D.tot.input + D.tot.output);
    const u = $("hero-unit"); u.textContent = "";
    const w = words(Number(value));
    if (key === "combined") u.append(bold(w ? w + " " : ""), document.createTextNode("combined tokens: "), bold(big(inp)), document.createTextNode(" read in and "), bold(big(out)), document.createTextNode(" written out, across "), bold(fmt(D.tot.requests)), document.createTextNode(" requests."));
    else if (key === "output") u.append(bold(w ? w + " " : ""), document.createTextNode("output tokens, reading "), bold(big(inp)), document.createTextNode(" input tokens across "), bold(fmt(D.tot.requests)), document.createTextNode(" requests (" + big(comb) + " combined)."));
    else u.append(bold(w ? w + " " : ""), document.createTextNode("input tokens, writing back "), bold(big(out)), document.createTextNode(" output tokens across "), bold(fmt(D.tot.requests)), document.createTextNode(" requests (" + big(comb) + " combined)."));

    // Signals: what's worth a look in this range, and a door to Pip.
    const signals = $("signal-strip"); signals.textContent = "";
    const refused = h("button", { class: "signal-card refused", type: "button", title: "Open the refusals on the Prompts page" }, h("b", { text: fmt(S.signals?.refused || 0) }), h("span", { text: "Refusals" }), h("small", { text: "Answers a model declined to give" }));
    const noteworthy = h("button", { class: "signal-card noteworthy", type: "button", title: "Open code, structured and long answers on the Prompts page" }, h("b", { text: fmt(S.signals?.noteworthy || 0) }), h("span", { text: "Worth a closer look" }), h("small", { text: "Code, structured or substantial answers" }));
    const ask = h("button", { class: "signal-card ask-pip", type: "button" }, h("img", { src: "/pip.svg", alt: "" }), h("span", {}, "Ask Pip about your trail", h("kbd", { text: "/" })), h("small", { text: "Your own model, with exact numbers from your CSV" }));
    refused.onclick = () => Prompts.filter("refused");
    noteworthy.onclick = () => Prompts.filter("noteworthy");
    ask.onclick = () => PipChat.open();
    signals.append(refused, noteworthy, ask);

    const n = D.days.length, today = S.meta.today;
    const todayIn = D.byDay.has(today);
    let prev = 0, cnt = 0;
    for (let i = 1; i <= 7; i++) { const d = addDays(today, -i); if (D.byDay.has(d)) { prev += D.dayTotal(d, key); cnt++; } }
    const tps = D.tot.gen_ms > 0 ? D.tot.gen_out / (D.tot.gen_ms / 1000) : 0;
    const ratio = D.tot.output > 0 ? D.tot.input / D.tot.output : 0;
    const stats = [
      ["Today so far", todayIn ? compact(D.dayTotal(today, key)) : "—", todayIn && cnt ? METRIC_NAME[key] + "; a typical day is " + compact(prev / cnt) : "today is outside this range"],
      ["Per day", compact(mv(D.tot, key) / Math.max(1, n)), METRIC_NAME[key] + " tokens on average"],
      ["Right now", S.live ? compact(S.live.tokens_per_sec) + "/s" : "—", "", "live"],
      ["Speed", tps ? tps.toFixed(1) + " tok/s" : "—", "output per request, while generating"],
      ["Input per output", ratio ? ratio.toFixed(1) + " to 1" : "—", "prompt tokens per token generated"],
      ["Errors", fmt(D.tot.errors), D.tot.errors ? "or clients that hung up early" : "every request finished"],
      ["Cache hits", fmt(D.tot.cache_hits || 0), D.tot.cache_hits ? compact(D.tot.cache_saved || 0) + " tokens answered without a model" : "repeated prompts answered from Toktail's cache"],
    ];
    const box = $("stats"); box.textContent = "";
    for (const [k, v, d, id] of stats) {
      box.append(h("div", { class: "stat" }, h("div", { class: "k", text: k }), h("div", { class: "v", id: id ? "stat-" + id : null, text: v }), h("div", { class: "d", id: id ? "stat-" + id + "-d" : null, text: d })));
    }
    updateLive(S.live);
  }

  // ---------------------------------------------------------------- daily stacked columns
  let dailyGeom = null; // kept for the trail replay
  function chartData(D) {
    if(D.days.length<=90)return D;
    const monthly=D.days.length>180, byDay=new Map(), blank=()=>({input:0,output:0,requests:0,gen_out:0,gen_ms:0,errors:0,estimated:0});
    for(const d of D.days) {
      const date=new Date(d+"T12:00:00Z");const key=monthly?d.slice(0,8)+"01":addDays(d,-((date.getUTCDay()+6)%7));
      if(!byDay.has(key))byDay.set(key,new Map());const bin=byDay.get(key);
      for(const [k,e]of D.byDay.get(d)){const cell=bin.get(k)||blank();for(const f in cell)cell[f]+=e[f]||0;bin.set(k,cell);}
    }
    return {...D,days:[...byDay.keys()],byDay,period:monthly?"month":"week",dayTotal:(d,key)=>[...byDay.get(d).values()].reduce((n,e)=>n+mv(e,key),0)};
  }
  function renderDaily(D) {
    D=chartData(D);
    const box = $("chart-daily");
    const key = S.metric;
    $("daily-title").textContent = METRIC_TITLE[key] + " tokens per "+(D.period||"day")+", by " + (D.byHost ? "host" : "model");
    $("daily-sub").textContent = fmt(mv(D.tot, key)) + " " + METRIC_NAME[key] + " tokens " + rangeName() + ". The line is the moving average.";
    dailyGeom = null;
    if (!D.series.length) { legend($("legend-daily"), []); return emptyState(box, "Nothing counted in this range yet. Send a request through one of your hosts' Toktail ports and Pip will add it here."); }
    const W = Math.max(320, box.clientWidth), H = W > 700 ? 360 : 270;
    const m = { l: 56, r: 12, t: 16, b: 30 };
    const svg = newSvg(box, W, H, $("daily-title").textContent);
    const n = D.days.length;
    const totals = D.days.map((d) => D.dayTotal(d, key));
    const ma = totals.map((_, i) => { let s = 0, c = 0; for (let j = Math.max(0, i - 6); j <= i; j++) { s += totals[j]; c++; } return s / c; });
    const { max, step } = niceMax(Math.max(...totals, ...ma, 1), 4);
    const iw = W - m.l - m.r, ih = H - m.t - m.b;
    const y = (v) => m.t + ih - (v / max) * ih;
    yAxis(svg, m.l, W - m.r, y, max, step);
    const band = iw / n;
    const bw = Math.max(1, Math.min(24, band * 0.72));
    const xc = (i) => m.l + band * i + band / 2;
    const gap = bw >= 4 ? 2 : 0;
    const colG = el("g", {}, svg);
    D.days.forEach((d, i) => {
      const per = D.byDay.get(d);
      const g = el("g", { "data-day": i }, colG);
      if (S.animate) { g.classList.add("anim-bar"); g.style.animationDelay = Math.min(700, i * (700 / n)) + "ms"; }
      const segs = D.series.map((k) => ({ k, v: per.has(k) ? mv(per.get(k), key) : 0 })).filter((s) => s.v > 0);
      let acc = 0;
      segs.forEach((s, si) => {
        const y0 = y(acc), y1 = y(acc + s.v);
        acc += s.v;
        const top = si === segs.length - 1;
        const hh = y0 - y1;
        const h2 = top ? hh : Math.max(0, hh - gap);
        if (h2 <= 0) return;
        const yy = top ? y1 : y1 + gap;
        if (top) el("path", { d: topRounded(xc(i) - bw / 2, yy, bw, h2, 4), fill: sColor(D, s.k) }, g);
        else el("rect", { x: xc(i) - bw / 2, y: yy, width: bw, height: h2, fill: sColor(D, s.k) }, g);
      });
    });
    if (n >= 7) {
      const dpath = ma.map((v, i) => (i ? "L" : "M") + xc(i).toFixed(1) + "," + y(v).toFixed(1)).join("");
      const p = el("path", { d: dpath, fill: "none", stroke: css("--ink"), "stroke-width": 2, "stroke-linejoin": "round", "stroke-linecap": "round", opacity: 0.8, class: "ma" }, svg);
      drawIn(p, 400);
    }
    xDateTicks(svg, D.days, xc, H - 9, Math.floor(iw / 72));
    const pi = totals.indexOf(Math.max(...totals));
    if (totals[pi] > 0) {
      const t = txt(svg, xc(pi), y(totals[pi]) - 7, compact(totals[pi]), { "text-anchor": "middle", fill: css("--ink-2"), "font-weight": 700, class: "peak" });
      if (S.animate) { t.classList.add("anim-fade"); t.style.animationDelay = "800ms"; }
    }
    const hit = el("g", { "data-ui": "1" }, svg);
    const hl = el("rect", { x: 0, y: m.t, width: band, height: ih, fill: css("--lav"), opacity: 0, "pointer-events": "none", rx: 4 }, hit);
    D.days.forEach((d, i) => {
      const r = el("rect", { x: m.l + band * i, y: m.t, width: band, height: ih, fill: "transparent" }, hit);
      r.addEventListener("pointermove", (e) => {
        hl.setAttribute("x", m.l + band * i); hl.setAttribute("opacity", 0.08);
        const per = D.byDay.get(d);
        const rows = D.series.filter((k) => per.has(k)).map((k) => ({ color: sColor(D, k), label: sName(D, k), value: fmt(mv(per.get(k), key)), v: mv(per.get(k), key) })).sort((a, b) => b.v - a.v);
        showTip(e, longDay(d), rows.length ? rows : [{ label: "No requests", value: "" }], { label: "Total " + METRIC_NAME[key] + (n >= 7 ? ", 7-"+(D.period||"day")+" avg " + compact(ma[i]) : ""), value: fmt(totals[i]) });
      });
      r.addEventListener("pointerleave", () => { hl.setAttribute("opacity", 0); hideTip(); });
    });
    const items = D.series.map((k) => ({ label: sName(D, k), color: sColor(D, k) }));
    if (n >= 7) items.push({ label: "7-"+(D.period||"day")+" average", color: css("--ink"), line: true });
    legend($("legend-daily"), items.length >= 2 ? items : []);
    dailyGeom = { svg, W, H, xc, y, base: y(0), totals, days: D.days };
  }

  // ---------------------------------------------------------------- share
  function renderShare(D) {
    const box = $("chart-models");
    const key = S.metric;
    $("models-title").textContent = METRIC_TITLE[key] + " tokens by " + (D.byHost ? "host" : "model");
    const rows = D.series.map((k) => ({ k, v: mv(D.perSeries.get(k), key), p: D.perSeries.get(k) })).sort((a, b) => b.v - a.v);
    const total = rows.reduce((s, r) => s + r.v, 0);
    $("models-sub").textContent = rows.length ? "Each " + (D.byHost ? "host" : "model") + "'s share of " + fmt(total) + " " + METRIC_NAME[key] + " tokens" : "";
    if (!rows.length || total === 0) return emptyState(box, "No " + METRIC_NAME[key] + " tokens in this range.");
    hbars(box, $("models-title").textContent, rows.map((r) => ({
      label: sName(D, r.k), v: r.v, color: sColor(D, r.k), value: compact(r.v), right: Math.round((r.v / total) * 1000) / 10 + "%",
      tip: () => [sName(D, r.k), [
        { color: sColor(D, r.k), label: "Combined", value: fmt(r.p.input + r.p.output) },
        { label: "Output", value: fmt(r.p.output) },
        { label: "Input", value: fmt(r.p.input) },
        { label: "Requests", value: fmt(r.p.requests) },
        { label: "Share of " + METRIC_NAME[key], value: ((r.v / total) * 100).toFixed(1) + "%" },
      ]],
    })));
  }

  function renderSpeed(D) {
    const box = $("chart-speed");
    $("card-speed").dataset.fixed = "Always output speed";
    $("speed-sub").textContent = "Output tokens per second while generating, per " + (D.byHost ? "host" : "model") + ", per day";
    const series = D.series.map((k) => ({
      label: sName(D, k), color: sColor(D, k),
      values: D.days.map((d) => { const e = D.byDay.get(d).get(k); return e && e.gen_ms > 0 ? e.gen_out / (e.gen_ms / 1000) : null; }),
    })).filter((s) => s.values.some((v) => v != null));
    legend($("legend-speed"), series.map((s) => ({ label: s.label, color: s.color, line: true })));
    if (!series.length) return emptyState(box, "No timed generations in this range.");
    lineChart(box, D.days, series, { title: "Generation speed (tokens per second)", endLabels: true, valueFmt: (v) => v.toFixed(1) + " t/s", tickFmt: (v) => String(Math.round(v)) });
  }

  function renderCumulative(D) {
    const box = $("chart-cumulative");
    $("card-cumulative").dataset.fixed = "Shows all three";
    let ci = 0, co = 0;
    const inp = [], out = [], comb = [];
    for (const d of D.days) { ci += D.dayTotal(d, "input"); co += D.dayTotal(d, "output"); inp.push(ci); out.push(co); comb.push(ci + co); }
    const series = [
      { label: "Combined", color: css("--lav"), values: comb, area: true, width: 2.5 },
      { label: "Input", color: css("--muted"), values: inp },
      { label: "Output", color: css("--ink"), values: out },
    ];
    legend($("legend-cum"), series.map((s) => ({ label: s.label, color: s.color, line: true })));
    if (ci + co === 0) return emptyState(box, "No tokens in this range.");
    lineChart(box, D.days, series, { title: "Running total of tokens", endLabels: true });
  }

  // ---------------------------------------------------------------- heatmap
  function renderHeat() {
    const box = $("chart-heat");
    const key = S.metric;
    const sc = scopeName();
    $("heat-title").textContent = sc.startsWith("Your") ? "When " + sc.toLowerCase() + " work" + (sc === "Your host" ? "s" : "") : "When " + sc + " works";
    const vals = S.heat.map((c) => mv(c, key));
    const max = Math.max(...vals, 0);
    if (max === 0) return emptyState(box, "No activity in this range.");
    const W = Math.max(300, box.clientWidth);
    const m = { l: 36, r: 6, t: 4, b: 50 };
    const cw = (W - m.l - m.r) / 24, ch = Math.min(28, Math.max(14, cw));
    const H = m.t + ch * 7 + m.b;
    const svg = newSvg(box, W, H, "When your hosts work, " + METRIC_NAME[key] + " tokens by weekday and hour");
    const steps = [1, 2, 3, 4, 5, 6, 7].map((i) => css("--seq-" + i));
    const zero = css("--seq-0");
    const bin = (v) => (v <= 0 ? -1 : Math.min(6, Math.floor(Math.sqrt(v / max) * 7)));
    for (let wd = 0; wd < 7; wd++) txt(svg, m.l - 6, m.t + wd * ch + ch / 2 + 4, DAYS[wd], { "text-anchor": "end" });
    for (let hr = 0; hr < 24; hr += 3) txt(svg, m.l + hr * cw + cw / 2, m.t + ch * 7 + 15, hr === 0 ? "12a" : hr < 12 ? hr + "a" : hr === 12 ? "12p" : (hr - 12) + "p", { "text-anchor": "middle" });
    for (const c of S.heat) {
      const b = bin(mv(c, key));
      const r = el("rect", { x: m.l + c.hour * cw + 1, y: m.t + c.weekday * ch + 1, width: Math.max(1, cw - 2), height: Math.max(1, ch - 2), rx: 4, fill: b < 0 ? zero : steps[b] }, svg);
      if (S.animate) { r.classList.add("anim-fade"); r.style.animationDelay = (c.hour * 18 + c.weekday * 30) + "ms"; }
      r.addEventListener("pointermove", (e) => {
        r.setAttribute("stroke", css("--ink")); r.setAttribute("stroke-width", 1.5);
        const hh = String(c.hour).padStart(2, "0");
        showTip(e, DAYS_LONG[c.weekday] + "s, " + hh + ":00 to " + hh + ":59", [
          { label: "Combined tokens", value: fmt(c.input + c.output) }, { label: "Output tokens", value: fmt(c.output) }, { label: "Input tokens", value: fmt(c.input) }, { label: "Requests", value: fmt(c.requests) },
        ]);
      });
      r.addEventListener("pointerleave", () => { r.removeAttribute("stroke"); hideTip(); });
    }
    const ly = m.t + ch * 7 + 28, lw = Math.min(180, W - m.l - 150);
    txt(svg, m.l, ly + 9, "Less");
    const sx = m.l + 34;
    steps.forEach((c, i) => el("rect", { x: sx + i * (lw / 7), y: ly, width: lw / 7 - 2, height: 10, rx: 2, fill: c }, svg));
    txt(svg, sx + lw + 6, ly + 9, "More (busiest hour " + compact(max) + " " + METRIC_NAME[key] + ")");
  }

  // ---------------------------------------------------------------- insights
  function renderInsights(D) {
    const ul = $("insights"); ul.textContent = "";
    const add = (icon, parts) => {
      const t = h("span");
      for (const p of parts) t.append(typeof p === "string" ? document.createTextNode(p) : p);
      ul.append(h("li", {}, h("span", { class: "ic", "aria-hidden": "true", text: icon }), t));
    };
    if (!D.tot.requests) { add("z", ["Nothing counted in this range. Pip is napping."]); return; }
    const key = S.metric, today = S.meta.today;
    const totals = D.days.map((d) => D.dayTotal(d, key));
    const word = METRIC_NAME[key];
    if (D.days.length >= 14) {
      const a = totals.slice(-7).reduce((s, v) => s + v, 0), b = totals.slice(-14, -7).reduce((s, v) => s + v, 0);
      if (b > 0) add(a >= b ? "↗" : "↘", ["The last 7 days ran ", bold(pct(a / b - 1)), " against the 7 before (" + compact(a) + " vs " + compact(b) + " " + word + ")."]);
    }
    const pi = totals.indexOf(Math.max(...totals));
    if (totals[pi] > 0 && D.days.length > 1) add("▲", ["Busiest day was ", bold(longDay(D.days[pi])), ", with " + fmt(totals[pi]) + " " + word + " tokens."]);
    const active = totals.filter((v) => v > 0).sort((x, y) => x - y);
    if (active.length >= 7) {
      const med = active[Math.floor(active.length / 2)];
      const mad = active.map((v) => Math.abs(v - med)).sort((x, y) => x - y)[Math.floor(active.length / 2)] || 1;
      const odd = D.days.map((d, i) => ({ d, v: totals[i], z: (totals[i] - med) / (1.4826 * mad) })).filter((o) => o.v > 0 && Math.abs(o.z) > 3.5).sort((a, b) => Math.abs(b.z) - Math.abs(a.z)).slice(0, 3);
      if (odd.length) add("!", ["Out of the ordinary: ", ...odd.flatMap((o, k) => [k ? ", " : "", bold(dayLabel(o.d)), " (" + (o.v > med ? (o.v / med).toFixed(1) + "× a typical day" : "unusually quiet") + ")"]), "."]);
      else add("✓", ["Steady going. A typical day is " + compact(med) + " " + word + " tokens and nothing stood out."]);
    }
    const ps = [...D.perSeries.entries()].filter(([k])=>k!=="__other__").sort((a, b) => mv(b[1], key) - mv(a[1], key));
    if (ps.length > 1 && mv(D.tot, key) > 0) add("◆", [bold(sName(D, ps[0][0])), " did " + Math.round((mv(ps[0][1], key) / mv(D.tot, key)) * 100) + "% of the work across " + ps.length + (D.byHost ? " hosts." : " models.")]);
    const speeds = ps.filter(([, v]) => v.gen_ms > 0 && v.gen_out > 50).map(([k, v]) => [k, v.gen_out / (v.gen_ms / 1000)]).sort((a, b) => b[1] - a[1]);
    if (speeds.length > 1) add("⚡", ["Fastest is ", bold(sName(D, speeds[0][0])), " at " + speeds[0][1].toFixed(1) + " tok/s. Slowest is ", bold(sName(D, speeds[speeds.length - 1][0])), " at " + speeds[speeds.length - 1][1].toFixed(1) + " tok/s."]);
    const month = today.slice(0, 7);
    if (D.byDay.has(today) && D.from <= month + "-01") {
      let mtd = 0, last14 = 0, c14 = 0;
      for (const d of D.days) if (d.startsWith(month)) mtd += D.dayTotal(d, key);
      for (let i = 1; i <= 14; i++) { const d = addDays(today, -i); if (D.byDay.has(d)) { last14 += D.dayTotal(d, key); c14++; } }
      const dim = new Date(Date.UTC(+month.slice(0, 4), +month.slice(5, 7), 0)).getUTCDate();
      const left = dim - +today.slice(8, 10);
      if (c14 >= 3 && left > 0) add("→", ["On pace for about ", bold(compact(mtd + (last14 / c14) * left)), " " + word + " tokens this month, with " + compact(mtd) + " so far."]);
    }
    const ph = [...S.heat].sort((a, b) => mv(b, key) - mv(a, key))[0];
    if (ph && mv(ph, key) > 0) add("◷", ["Busiest hour is ", bold(DAYS_LONG[ph.weekday] + "s at " + String(ph.hour).padStart(2, "0") + ":00"), "."]);
    if (S.repeats.repeated_requests > 0) {
      const share = S.repeats.repeated_requests / D.tot.requests;
      add("↺", [bold(fmt(S.repeats.repeated_requests)), " requests (" + (share * 100).toFixed(1) + "%) repeated a prompt Pip had already seen. A response cache could answer those instantly."]);
    }
    if (D.tot.estimated) add("≈", [fmt(D.tot.estimated) + " requests were counted from the stream because LM Studio didn't report usage, so their input shows as 0."]);
  }

  // ---------------------------------------------------------------- costs
  function renderCosts(D) {
    const card = $("card-costs");
    const R = S.costs;
    card.hidden = !(S.meta.costs_enabled && R);
    if (card.hidden) return;
    card.dataset.fixed = "Prices both input and output";
    const compared = R.models.filter((m) => m.compared).sort((a, b) => b.total - a.total);
    const base = R.models.find((m) => m.id === R.baseline) || compared[0];
    const sel = $("baseline"); sel.textContent = "";
    for (const m of R.models) {
      const o = h("option", { value: m.id, text: m.provider + " " + m.name.replace(m.provider + " ", "") });
      if (m.id === base.id) o.selected = true;
      sel.append(o);
    }
    $("costs-sub").textContent = (R.market ? "Market prices as of " + R.as_of + " via " + R.market.replace(/ \(.*\)$/, "") : "List prices as of " + R.as_of) + ", for " + scopeName().toLowerCase() + "' requests " + rangeName() + (R.cache_aware ? ". Repeated prompt prefixes priced at the cached-input rate." : "");
    const local = R.local;
    const hw = R.hardware || { amortized: 0, machines: [], has_fleet: false };
    const yours = local.cost + hw.amortized;
    const saved = base.total - yours;
    const lead = $("costs-lead"); lead.textContent = "";
    const why = $("costs-why"); why.textContent = "";
    if (!R.requests) {
      lead.textContent = "Nothing to price in this range yet.";
    } else {
      lead.append(document.createTextNode(saved >= 0 ? "Running locally saved about " : "Running locally cost about "), span(usd(Math.abs(saved)), saved >= 0 ? "save" : "loss"),
        document.createTextNode(saved >= 0 ? " compared with " + base.name + "." : " more than " + base.name + " would have."));
      why.textContent = base.name + " would have charged " + usd(base.total) + " for " + fmt(R.input_tokens) + " input and " + fmt(R.output_tokens) +
        " output tokens (" + fmt(R.input_tokens + R.output_tokens) + " combined). Your hosts spent " + local.busy_hours.toFixed(1) + " hours generating: about " + usd(local.cost) + " of electricity at " + usd(local.usd_per_kwh) + " per kWh" +
        (hw.has_fleet ? ", plus " + usd(hw.amortized) + " of hardware wear (each machine's price spread over its lifecycle, for these " + hw.days + " days)." : ".");
      if (!hw.has_fleet) {
        const a = h("a", { href: "#/savings", text: "Add your hardware" });
        why.append(" ", a, " to count what the machines cost, too.");
      } else {
        why.append(" ", h("a", { href: "#/savings", text: "See the monthly and yearly breakdown →" }));
      }
    }
    // What your host cost, side by side with the baseline.
    const vs = $("cost-vs"); vs.textContent = "";
    if (R.requests) {
      const max = Math.max(base.total, yours, 1e-9);
      const bar = (label, value, parts, sub) => {
        const track = h("div", { class: "vs-track" });
        for (const p of parts) if (p.v > 0) track.append(h("i", { style: `width:${(p.v / max) * 100}%;background:${p.color}`, title: p.label + ": " + usd(p.v) }));
        return h("div", { class: "vs-row" }, h("div", { class: "vs-label" }, h("b", { text: label }), sub ? span(sub, "label-sm") : null), track, h("div", { class: "vs-val", text: usd(value) }));
      };
      vs.append(
        bar(base.name, base.total, [{ v: base.total, color: css("--lav"), label: base.name }], "at list price"),
        bar("What your hosts cost", yours, [{ v: hw.amortized, color: css("--orange"), label: "Hardware share" }, { v: local.cost, color: css("--mint"), label: "Electricity" }],
          hw.has_fleet ? usd(hw.amortized) + " hardware + " + usd(local.cost) + " electricity" : "electricity only"),
      );
    }
    const rows = compared.map((m) => ({
      label: m.provider + " " + m.name.replace(m.provider + " ", ""), v: m.total, strong: m.id === base.id,
      color: m.id === base.id ? css("--lav") : css("--neutral-bar"), value: usd(m.total),
      right: yours > 0 && m.total > 0 ? (m.total / yours).toFixed(m.total / yours < 10 ? 1 : 0) + "× your cost" : "",
      tip: () => [m.provider + " " + m.name, [
        { label: "Input at $" + m.input_rate + "/M", value: usd(m.input_cost) },
        { label: "Output at $" + m.output_rate + "/M", value: usd(m.output_cost) },
        ...(m.request_cost ? [{ label: "Request fees", value: usd(m.request_cost) }] : []),
        { label: "Saved by running locally", value: usd(m.total - yours) },
      ]],
    }));
    rows.push({ label: "Your hosts (hardware share + electricity)", v: yours, strong: true, value: usd(yours), right: "",
      parts: [{ v: hw.amortized, color: css("--orange") }, { v: local.cost, color: css("--mint") }],
      tip: () => ["Your hosts", [{ label: "Hardware share", value: usd(hw.amortized) }, { label: "Hours generating", value: local.busy_hours.toFixed(2) }, { label: "Energy", value: local.kwh.toFixed(2) + " kWh" }, { label: "Electricity", value: usd(local.cost) }]] });
    const box = $("chart-costbars");
    legend($("legend-costbars"), [{ label: base.name + " (comparing with)", color: css("--lav") }, { label: "Other services", color: css("--neutral-bar") }, { label: "Hardware share", color: css("--orange") }, { label: "Electricity", color: css("--mint") }]);
    if (!R.requests) emptyState(box, "Prices appear once there are requests in this range.");
    else hbars(box, "What this range would cost on each service", rows);

    // Running total: the baseline's price against your hosts' cost.
    const days = D.days;
    const byDate = new Map(R.daily.map((d) => [d.date, d]));
    const hwPerDay = hw.days > 0 ? hw.amortized / hw.days : 0;
    let cb = 0, cl = 0;
    const vb = [], vl = [];
    for (const d of days) { const e = byDate.get(d); cb += e ? e.models[base.id] || 0 : 0; cl += (e ? e.local : 0) + hwPerDay; vb.push(cb); vl.push(cl); }
    const cumBox = $("chart-costcum");
    $("costcum-title").textContent = "Running total against " + base.name;
    const series = [{ label: base.name, color: css("--lav"), values: vb }, { label: "Your hosts", color: css("--mint"), values: vl }];
    legend($("legend-costcum"), series.map((s) => ({ label: s.label, color: s.color, line: true })).concat([{ label: "Saved", color: "color-mix(in srgb, " + css("--mint") + " 30%, transparent)" }]));
    if (cb + cl === 0) emptyState(cumBox, "No priced requests in this range.");
    else lineChart(cumBox, days, series, { title: "Running total cost", endLabels: true, valueFmt: usd, tickFmt: usdTick, between: [0, 1], betweenColor: css("--mint"),
      tipTotal: (i) => ({ label: "Saved so far", value: usd(vb[i] - vl[i]) }) });

    const tb = $("costs-table"); tb.textContent = "";
    const cols = [
      { h: "Service", l: true, v: (m) => m.provider },
      { h: "Model", l: true, v: (m) => m.name },
      { h: "Input $/1M", v: (m) => "$" + m.input_rate },
      { h: "Output $/1M", v: (m) => "$" + m.output_rate },
      { h: "Per request", v: (m) => (m.per_request ? "$" + m.per_request : "—") },
      { h: "This range", v: (m) => usd(m.total) },
      { h: "You saved", v: (m) => usd(m.total - yours), title: "After hardware share and electricity" },
    ];
    const t = table(cols, compared);
    [...t.tBodies[0].rows].forEach((tr, i) => { if (compared[i].id === base.id) tr.className = "hl"; });
    tb.append(t);

    const notes = $("costs-notes"); notes.textContent = "";
    const lines = [...R.notes];
    if (R.excluded_embedding_requests) lines.push(fmt(R.excluded_embedding_requests) + " embedding requests are left out, since chat prices don't apply to them.");
    for (const m of compared) if (m.note) lines.push(m.name + ": " + m.note);
    for (const l of lines) notes.append(h("div", { text: l }));
    const src = h("div", {}, "Prices from " + (R.origin === "built-in" ? "Toktail's built-in list" : R.origin) + (R.market ? ", updated from the market (" + R.market + ")" : "") + ": ");
    const seen = new Set();
    compared.forEach((m) => {
      if (!m.source || seen.has(m.provider)) return;
      seen.add(m.provider);
      if (seen.size > 1) src.append(", ");
      src.append(h("a", { href: m.source, target: "_blank", rel: "noopener noreferrer", text: m.provider }));
    });
    src.append(".");
    notes.append(src);
  }

  // ---------------------------------------------------------------- tables
  let currentTable = { cols: [], rows: [], name: "table" };
  function renderTable(D) {
    $("card-table").dataset.fixed = S.tab === "daily" ? "" : "Shows input and output";
    for (const b of $("tabs").children) b.setAttribute("aria-selected", String(b.dataset.t === S.tab));
    $("card-table").classList.toggle("metric-aware", S.tab === "daily");
    const box = $("table"); box.textContent = "";
    let cols, rows, name;
    if (S.tab === "daily") {
      const key = S.metric;
      rows = [...D.days].reverse().map((d) => ({ d, per: D.byDay.get(d) })).filter((r) => r.per.size > 0);
      cols = [{ h: "Date", l: true, v: (r) => r.d, raw: (r) => r.d }];
      for (const k of D.series) cols.push({ h: sName(D, k) + " (" + METRIC_NAME[key] + ")", v: (r) => fmt(r.per.has(k) ? mv(r.per.get(k), key) : 0), raw: (r) => (r.per.has(k) ? mv(r.per.get(k), key) : 0) });
      cols.push({ h: "Combined", v: (r) => fmt(D.dayTotal(r.d, "combined")), raw: (r) => D.dayTotal(r.d, "combined") });
      cols.push({ h: "Output", v: (r) => fmt(D.dayTotal(r.d, "output")), raw: (r) => D.dayTotal(r.d, "output") });
      cols.push({ h: "Input", v: (r) => fmt(D.dayTotal(r.d, "input")), raw: (r) => D.dayTotal(r.d, "input") });
      cols.push({ h: "Requests", v: (r) => fmt(D.dayTotal(r.d, "requests")), raw: (r) => D.dayTotal(r.d, "requests") });
      name = "daily-by-" + (D.byHost ? "host" : "model");
    } else if (S.tab === "recent") {
      rows = S.recent;
      cols = [
        { h: "Time", l: true, v: (r) => r.timestamp.replace("T", " ").slice(0, 19), raw: (r) => r.timestamp },
        { h: "Host", l: true, v: (r) => swatchText(hostName(r.host), hostColorVar(r.host)), raw: (r) => r.host },
        { h: "Model", l: true, v: (r) => swatchText(r.model, colorVar(r.model)), raw: (r) => r.model },
        { h: "Combined", v: (r) => fmt(r.input + r.output), raw: (r) => r.input + r.output },
        { h: "Input", v: (r) => fmt(r.input), raw: (r) => r.input },
        { h: "Output", v: (r) => fmt(r.output), raw: (r) => r.output },
        { h: "tok/s", v: (r) => (r.tokens_per_sec == null ? "—" : r.tokens_per_sec.toFixed(1)), raw: (r) => r.tokens_per_sec ?? "" },
        { h: "First token", v: (r) => (r.ttft_ms == null ? "—" : r.ttft_ms + " ms"), raw: (r) => r.ttft_ms ?? "" },
        { h: "Duration", v: (r) => (r.duration_ms / 1000).toFixed(2) + " s", raw: (r) => r.duration_ms },
        { h: "Status", v: (r) => span(String(r.status) + (r.status === 499 ? " client left" : ""), "tag" + (r.status >= 400 ? " err" : "")), raw: (r) => r.status },
        { h: "Saved", v: (r) => (r.captured ? h("a", { href: "#/prompts/" + r.request_id, text: "Open" }) : "—"), raw: (r) => (r.captured ? "yes" : "") },
        { h: "Prompt fingerprint", l: true, v: (r) => fpCell(r.prompt_sha256), raw: (r) => r.prompt_sha256 },
      ];
      name = "recent-requests";
    } else {
      rows = S.repeats.top;
      cols = [
        { h: "Prompt fingerprint", l: true, v: (r) => fpCell(r.prompt_sha256), raw: (r) => r.prompt_sha256 },
        { h: "Times sent", v: (r) => fmt(r.count), raw: (r) => r.count },
        { h: "Input tokens", v: (r) => fmt(r.input_tokens), raw: (r) => r.input_tokens },
        { h: "Models", l: true, v: (r) => r.models.join(", "), raw: (r) => r.models.join(" ") },
        { h: "First seen", l: true, v: (r) => r.first_seen.replace("T", " ").slice(0, 16), raw: (r) => r.first_seen },
        { h: "Last seen", l: true, v: (r) => r.last_seen.replace("T", " ").slice(0, 16), raw: (r) => r.last_seen },
      ];
      name = "repeated-prompts";
    }
    currentTable = { cols, rows, name };
    markMetric();
    if (!rows.length) return emptyState(box, S.tab === "repeats" ? "No prompt was sent twice in this range." : "No rows in this range.");
    let page=0;
    const draw=()=>{
      box.textContent="";box.append(table(cols,rows.slice(page*50,(page+1)*50)));
      if(rows.length>50){const prev=h("button",{class:"small",type:"button",text:"Previous"}),next=h("button",{class:"small",type:"button",text:"Next"});prev.disabled=page===0;next.disabled=(page+1)*50>=rows.length;prev.onclick=()=>{page--;draw();};next.onclick=()=>{page++;draw();};box.append(h("div",{class:"table-pagination"},prev,h("span",{class:"label-sm",text:fmt(page*50+1)+"–"+fmt(Math.min(rows.length,(page+1)*50))+" of "+fmt(rows.length)}),next));}
    };draw();
  }
  function exportTableCsv() {
    const { cols, rows, name } = currentTable;
    const lines = [cols.map((c) => csvEscape(c.h)).join(",")].concat(rows.map((r) => cols.map((c) => csvEscape(c.raw(r))).join(",")));
    const [f, t] = rangeDates();
    download(`toktail-${name}-${f}-to-${t}.csv`, new Blob([lines.join("\n") + "\n"], { type: "text/csv" }));
  }

  function renderFoot() {
    const f = $("foot"); f.textContent = "";
    const p = (parts) => f.append(h("div", {}, ...parts));
    const code = (s) => h("code", { text: s });
    p(["Your CSV is the source of truth: ", code(S.meta.data_dir), ", " + S.meta.files.length + " file" + (S.meta.files.length === 1 ? "" : "s") + ", " + fmt(S.meta.rows) + " requests from " + S.meta.hosts.length + " host" + (S.meta.hosts.length === 1 ? "" : "s") + ". The CSV holds counts and SHA-256 fingerprints of prompts, never the prompts themselves."]);
    if (S.meta.capture.enabled) p(["Prompts and responses are saved separately in ", code(S.meta.capture.dir), S.meta.capture.encryption !== "off" ? ", encrypted." : ", compressed but not encrypted."]);
    p(["Days run midnight to midnight local time (UTC" + (S.meta.utc_offset_minutes >= 0 ? "+" : "−") + Math.abs(S.meta.utc_offset_minutes / 60) + "). This page refreshes every 30 seconds."]);
    p(["Toktail " + S.meta.version + " is open source under the Apache 2.0 license: ", h("a", { href: S.meta.repository, target: "_blank", rel: "noopener noreferrer", text: "github.com/playandprosper/toktail" }), "."]);
    if (!S.meta.costs_enabled) p([h("a", { href: "#/settings/costs", text: "Turn on the cloud cost comparison" }), " to see what these tokens would cost on Claude, GPT, Grok, Gemini and others, against what your hosts cost."]);
  }

  // ---------------------------------------------------------------- export
  const EXPORTS = {
    daily: ["#chart-daily", "#legend-daily"], models: ["#chart-models", null],
    speed: ["#chart-speed", "#legend-speed"], cumulative: ["#chart-cumulative", "#legend-cum"],
    heat: ["#chart-heat", null], costbars: ["#chart-costbars", "#legend-costbars"],
    costcum: ["#chart-costcum", "#legend-costcum"],
  };
  function exportSvg(which) {
    const [chart, leg] = EXPORTS[which];
    const [f, t] = rangeDates();
    exportChart(chart, leg, null, scopeName() + ", " + dayLabel(f) + ", " + f.slice(0, 4) + " to " + dayLabel(t) + ", " + t.slice(0, 4), `toktail-${which}-${f}-to-${t}.svg`);
  }

  // ---------------------------------------------------------------- wire up
  function init() {
    const saved = store.get("hosts");
    S.hosts = saved ? new Set(saved.split("|").filter(Boolean)) : null;
    $("ranges").addEventListener("click", (e) => {
      const b = e.target.closest("button"); if (!b) return;
      S.range = b.dataset.r; store.set("range", S.range);
      if (S.range === "custom" && !$("from").value) { $("from").value = addDays(S.meta.today, -29); $("to").value = S.meta.today; }
      load();
    });
    $("from").addEventListener("change", load);
    $("to").addEventListener("change", load);
    $("metric").addEventListener("click", (e) => {
      const b = e.target.closest("button"); if (!b) return;
      S.metric = b.dataset.m; store.set("metric2", S.metric);
      S.dataKey = ""; load();
    });
    // Hovering the switch lights up the charts it changes.
    $("metric-ctl").addEventListener("pointerenter", () => document.body.classList.add("metric-hint"));
    $("metric-ctl").addEventListener("pointerleave", () => document.body.classList.remove("metric-hint"));
    $("split").addEventListener("click", (e) => {
      const b = e.target.closest("button"); if (!b) return;
      S.split = b.dataset.s; store.set("split", S.split);
      S.dataKey = ""; load();
    });
    $("hosts-strip").addEventListener("click", (e) => {
      const t = e.target.closest("[data-host]"); if (!t) return;
      selectHost(t.dataset.host, e.shiftKey || e.metaKey || e.ctrlKey);
    });
    $("tabs").addEventListener("click", (e) => { const b = e.target.closest("button"); if (!b) return; S.tab = b.dataset.t; renderTable(S.lastData); });
    document.addEventListener("click", (e) => { const b = e.target.closest("#page-dashboard [data-export]"); if (b) exportSvg(b.dataset.export); });
    $("btn-table-csv").addEventListener("click", exportTableCsv);
    $("btn-raw-csv").addEventListener("click", () => { location.href = "/api/export.csv?" + qs(); });
    $("btn-replay").addEventListener("click", () => Stage.replay(dailyGeom));
    $("btn-live-pause").addEventListener("click", (e) => {
      S.feedPaused = !S.feedPaused;
      e.currentTarget.setAttribute("aria-pressed", String(S.feedPaused));
      e.currentTarget.textContent = S.feedPaused ? "Resume feed" : "Pause feed";
      if (!S.feedPaused) renderFeed();
    });
    $("baseline").addEventListener("change", async (e) => {
      try { await postJSON("/api/settings", { costs: { baseline: e.target.value } }); S.dataKey = ""; load(); }
      catch (err) { toast("Couldn't change the comparison: " + err.message); }
    });
  }

  return { derive, chartData, init, load, render, updateLive, ingestLive, renderFeed, rangeName, scopeName, get geom() { return dailyGeom; } };
})();


