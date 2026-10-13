// SPDX-License-Identifier: Apache-2.0
// Toktail portal: the Models page. Every model that ran through Toktail,
// side by side: how fast it starts, how fast it writes, how often it fails
// or refuses, how much work it did, and what that work would have cost.
"use strict";

const Models = (() => {
  const V = {
    data: null, sort: store.get("models.sort") || "combined", dir: -1,
    split: store.get("models.split") === "host" ? "host" : "model", sel: null,
  };
  const shortName = (m) => m.split("/").pop();
  const keyOf = (r) => r.model + (r.host ? "@" + r.host : "");

  function params() {
    const p = new URLSearchParams();
    const r = $("mdl-range").value;
    if (r !== "all" && S.meta) { p.set("from", addDays(S.meta.today, -(+r - 1))); p.set("to", S.meta.today); }
    if ($("mdl-host").value) p.set("hosts", $("mdl-host").value);
    return p;
  }

  async function show(sub) {
    if (!S.meta) return;
    const hs = $("mdl-host"), keep = hs.value;
    hs.textContent = "";
    hs.append(h("option", { value: "", text: "All hosts" }));
    for (const x of S.meta.hosts) hs.append(h("option", { value: x.id, text: x.name }));
    hs.value = keep;
    for (const b of $("mdl-split").children) b.setAttribute("aria-pressed", String(b.dataset.v === V.split));
    if (sub) V.sel = decodeURIComponent(sub);
    try {
      V.data = await getJSON("/api/models/stats?" + params());
      render();
    } catch (e) { toast("Couldn't load model stats: " + e.message); }
  }

  const rows = () => (V.data ? (V.split === "host" ? V.data.by_host : V.data.models) : []);
  const COLS = [
    { k: "model", h: "Model", l: true, v: (r) => r.model, cell: (r) => h("span", {}, swatchText(r.model, colorVar(r.model)), r.host ? span(" on " + hostName(r.host), "label-sm") : null) },
    { k: "hosts", h: "Hosts", l: true, v: (r) => (r.host ? hostName(r.host) : r.hosts.map(hostName).join(", ")), hideSplit: "host" },
    { k: "requests", h: "Requests", v: (r) => r.requests, cell: (r) => fmt(r.requests) },
    { k: "combined", h: "Combined", v: (r) => r.combined, cell: (r) => compact(r.combined), title: "Input + output tokens" },
    { k: "output", h: "Output", v: (r) => r.output, cell: (r) => compact(r.output) },
    { k: "tps", h: "Speed", v: (r) => r.weighted_tps ?? -1, cell: (r) => (r.weighted_tps == null ? "—" : r.weighted_tps.toFixed(1) + " tok/s"), title: "Output tokens per second while generating (output-weighted)" },
    { k: "tps95", h: "Speed p95", v: (r) => r.tokens_per_sec.p95 ?? -1, cell: (r) => (r.tokens_per_sec.p95 == null ? "—" : r.tokens_per_sec.p95.toFixed(1)), title: "95% of requests were slower than this" },
    { k: "ttft", h: "First token", v: (r) => -(r.ttft_ms.p50 ?? 1e12), cell: (r) => ms(r.ttft_ms.p50), title: "Median time to the first token (lower is better)" },
    { k: "ttft95", h: "First token p95", v: (r) => -(r.ttft_ms.p95 ?? 1e12), cell: (r) => ms(r.ttft_ms.p95), title: "The slowest 5% waited at least this long" },
    { k: "errors", h: "Errors", v: (r) => -r.error_rate, cell: (r) => h("span", { class: r.error_rate > 0.05 ? "neg" : "" }, (r.error_rate * 100).toFixed(1) + "%"), title: "Share of requests that ended in an error (the client hanging up isn't counted)" },
    { k: "refused", h: "Refused", v: (r) => r.refused, cell: (r) => (r.refused ? fmt(r.refused) : "—") },
    { k: "cloud", h: "Cloud value", v: (r) => r.cloud_value, cell: (r) => usd(r.cloud_value), title: () => "What its tokens would have cost at " + (V.data.baseline_name || "the baseline") + " list prices" },
    { k: "trend", h: "Daily tokens", v: (r) => r.combined, cell: (r) => sparkline(r.daily_tokens.slice(-60), 90, 20, color(r.model), color(r.model)), nosort: true },
    { k: "last", h: "Last used", v: (r) => r.last_used_ms, cell: (r) => (r.last_used_ms ? ago(new Date(r.last_used_ms).toISOString()) : "—") },
  ];
  function ms(v) { return v == null ? "—" : v >= 10000 ? (v / 1000).toFixed(1) + " s" : fmt(v) + " ms"; }

  function render() {
    const D = V.data;
    const list = rows();
    const range = $("mdl-range").selectedOptions[0].textContent.toLowerCase();
    $("models-page-sub").textContent = fmt(D.models.length) + " model" + (D.models.length === 1 ? "" : "s") + " ran " + (range === "all time" ? "since you started counting" : "in the last " + range) + ($("mdl-host").value ? " on " + hostName($("mdl-host").value) : "") + ". How fast each one starts, how fast it writes, how often it stumbles, and what its tokens would have cost.";
    renderPodium(D.models);
    renderScatter(list);
    renderTable(list);
    const pick = list.find((r) => keyOf(r) === V.sel) || list[0];
    if (pick) select(pick, true); else { $("md-title").textContent = "No models in this range"; $("md-sub").textContent = ""; $("model-detail").textContent = ""; }
    if (typeof Stage !== "undefined") Stage.platformsChanged();
  }

  function renderPodium(models) {
    const box = $("podium"); box.textContent = "";
    const busy = models.filter((m) => m.requests >= 5);
    const best = (arr, f) => arr.filter((m) => f(m) != null).sort((a, b) => f(b) - f(a))[0];
    const medals = [
      ["⚡", "Fastest writer", best(busy, (m) => m.weighted_tps), (m) => m.weighted_tps.toFixed(1) + " tok/s"],
      ["🏁", "Quickest to start", best(busy, (m) => (m.ttft_ms.p50 == null ? null : -m.ttft_ms.p50)), (m) => ms(m.ttft_ms.p50)],
      ["🦊", "Did the most work", best(models, (m) => m.combined), (m) => compact(m.combined) + " tokens"],
      ["💸", "Most cloud value", best(models, (m) => m.cloud_value || null), (m) => usd0(m.cloud_value)],
    ];
    for (const [icon, label, m, val] of medals) {
      if (!m) continue;
      const b = h("button", { type: "button", class: "medal" }, h("span", { class: "k" }, h("i", { text: icon, "aria-hidden": "true" }), label), h("span", { class: "v", text: val(m) }), h("span", { class: "m", text: m.model }));
      b.onclick = () => { if (V.split === "host") { V.split = "model"; store.set("models.split", "model"); show(); } select(m); };
      box.append(b);
    }
  }

  function renderScatter(list) {
    const box = $("chart-scatter");
    const pts = list.filter((r) => r.weighted_tps != null && r.ttft_ms.p50 != null && r.requests > 0);
    if (!pts.length) return emptyState(box, "No timed generations in this range yet.");
    const W = Math.max(320, box.clientWidth), H = 320, m = { l: 62, r: 20, t: 16, b: 40 };
    const svg = newSvg(box, W, H, "Speed against time to first token, per model");
    const xs = niceMax(Math.max(...pts.map((p) => p.weighted_tps)) * 1.08, 5);
    const ys = niceMax(Math.max(...pts.map((p) => p.ttft_ms.p50)) * 1.1, 4);
    const iw = W - m.l - m.r, ih = H - m.t - m.b;
    const x = (v) => m.l + (v / xs.max) * iw, y = (v) => m.t + (v / ys.max) * ih; // lower is better: 0 at the top
    for (let v = 0; v <= ys.max + 1e-9; v += ys.step) {
      el("line", { x1: m.l, x2: W - m.r, y1: y(v), y2: y(v), stroke: css(v ? "--grid" : "--axis"), "shape-rendering": "crispEdges" }, svg);
      txt(svg, m.l - 8, y(v) + 4, v >= 1000 ? (v / 1000).toFixed(v % 1000 ? 1 : 0) + " s" : v + " ms", { "text-anchor": "end" });
    }
    for (let v = 0; v <= xs.max + 1e-9; v += xs.step) txt(svg, x(v), H - 22, String(Math.round(v)), { "text-anchor": "middle" });
    txt(svg, m.l + iw / 2, H - 5, "output tokens per second →", { "text-anchor": "middle", fill: css("--ink-2") });
    txt(svg, 4, H - 5, "↑ starts sooner", { fill: css("--ink-2") });
    const maxC = Math.max(...pts.map((p) => p.combined), 1);
    const placed = [];
    pts.sort((a, b) => b.combined - a.combined).forEach((p, i) => {
      const cx = x(p.weighted_tps), cy = y(p.ttft_ms.p50), r = 6 + 22 * Math.sqrt(p.combined / maxC);
      const g = el("g", { style: "cursor:pointer" }, svg);
      const c = el("circle", { cx, cy, r, fill: color(p.model), "fill-opacity": 0.32, stroke: color(p.model), "stroke-width": keyOf(p) === V.sel ? 3 : 1.6 }, g);
      if (S.animate !== false && !reduced()) { c.classList.add("anim-fade"); c.style.animationDelay = i * 40 + "ms"; }
      el("circle", { cx, cy, r: 2.5, fill: color(p.model) }, g);
      // Label when it doesn't collide with one already drawn.
      const lx = cx + r + 4, ly = cy + 4, label = shortName(p.model) + (p.host ? " · " + hostName(p.host) : "");
      const wLabel = label.length * 6.4;
      if (lx + wLabel < W && !placed.some((q) => Math.abs(q.y - ly) < 13 && lx < q.x + q.w && q.x < lx + wLabel)) {
        txt(g, lx, ly, label, { fill: css("--ink-2"), "font-size": 11.5 });
        placed.push({ x: lx, y: ly, w: wLabel });
      }
      g.addEventListener("pointermove", (e) => showTip(e, p.model + (p.host ? " on " + hostName(p.host) : ""), [
        { color: color(p.model), label: "Speed", value: p.weighted_tps.toFixed(1) + " tok/s" },
        { label: "First token (median)", value: ms(p.ttft_ms.p50) },
        { label: "Combined tokens", value: fmt(p.combined) },
        { label: "Requests", value: fmt(p.requests) },
      ]));
      g.addEventListener("pointerleave", hideTip);
      g.addEventListener("click", () => select(p));
    });
  }

  function renderTable(list) {
    const box = $("leaderboard"); box.textContent = "";
    if (!list.length) return emptyState(box, "No requests in this range.");
    const cols = COLS.filter((c) => c.hideSplit !== V.split);
    const col = cols.find((c) => c.k === V.sort) || cols.find((c) => c.k === "combined");
    const sorted = list.slice().sort((a, b) => {
      const va = col.v(a), vb = col.v(b);
      return (typeof va === "string" ? va.localeCompare(vb) : va - vb) * V.dir;
    });
    const t = table(cols.map((c) => ({ h: c.h, l: c.l, title: typeof c.title === "function" ? c.title() : c.title, v: c.cell || c.v })), sorted);
    [...t.tHead.rows[0].cells].forEach((th, i) => {
      const c = cols[i];
      if (c.nosort) return;
      if (c.k === col.k) th.setAttribute("aria-sort", V.dir < 0 ? "descending" : "ascending");
      th.tabIndex = 0;
      const sortBy = () => { if (V.sort === c.k) V.dir = -V.dir; else { V.sort = c.k; V.dir = c.l ? 1 : -1; } store.set("models.sort", V.sort); renderTable(rows()); };
      th.addEventListener("click", sortBy);
      th.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); sortBy(); } });
    });
    [...t.tBodies[0].rows].forEach((tr, i) => {
      const r = sorted[i];
      if (keyOf(r) === V.sel) tr.className = "sel";
      tr.addEventListener("click", () => select(r));
    });
    box.append(t);
    V.table = { cols, rows: sorted };
  }

  function select(r, quiet) {
    V.sel = keyOf(r);
    for (const tr of document.querySelectorAll("#leaderboard tbody tr")) tr.classList.remove("sel");
    const idx = V.table ? V.table.rows.indexOf(r) : -1;
    const tr = idx >= 0 ? document.querySelectorAll("#leaderboard tbody tr")[idx] : null;
    if (tr) tr.classList.add("sel");
    if (!quiet && location.hash !== "#/models/" + encodeURIComponent(V.sel)) history.replaceState(null, "", "#/models/" + encodeURIComponent(V.sel));
    $("md-title").textContent = r.model;
    $("md-sub").textContent = (r.host ? "on " + hostName(r.host) + " · " : "") + (r.first_date ? "first seen " + longDay(r.first_date) + " · " : "") + "ran on " + r.hosts.map(hostName).join(", ");
    const box = $("model-detail"); box.textContent = "";
    const stat = (k, v, d) => h("div", {}, h("div", { class: "k", text: k }), h("div", { class: "v", text: v }), d ? h("div", { class: "d", text: d }) : null);
    box.append(h("div", { class: "md-grid" },
      stat("Speed", r.weighted_tps == null ? "—" : r.weighted_tps.toFixed(1) + " tok/s", r.tokens_per_sec.p50 == null ? "" : "median request " + r.tokens_per_sec.p50.toFixed(1) + " · fastest 5% " + r.tokens_per_sec.p95.toFixed(1)),
      stat("First token", ms(r.ttft_ms.p50), "median · slowest 5% " + ms(r.ttft_ms.p95)),
      stat("Whole request", ms(r.duration_ms.p50), "median · slowest 5% " + ms(r.duration_ms.p95)),
      stat("Requests", fmt(r.requests), (r.errors ? fmt(r.errors) + " errors · " : "") + (r.cancelled ? fmt(r.cancelled) + " cancelled · " : "") + (r.estimated ? fmt(r.estimated) + " estimated" : "all reported")),
      stat("Per request", compact(r.avg_input || 0) + " → " + compact(r.avg_output || 0), "average input → output tokens"),
      stat("Cloud value", usd(r.cloud_value), "at " + (V.data.baseline_name || "the baseline")),
    ));
    const chart = h("div", { class: "chart" });
    box.append(chart);
    if (r.daily_tokens.some((v) => v > 0)) lineChart(chart, V.data.days.map(String), [{ label: "Combined tokens", color: color(r.model), values: r.daily_tokens, area: true, width: 2 }], { title: r.model + " combined tokens per day", height: 170 });
    const asks = h("div", { class: "md-actions" },
      h("button", { type: "button", class: "small", text: "Ask Pip about it", onclick: () => PipChat.open("How is " + r.model + " doing" + (r.host ? " on " + hostName(r.host) : "") + " compared with my other models?") }),
      h("button", { type: "button", class: "small", text: "Only this model on the dashboard", onclick: () => { S.hidden = new Set(S.meta.models.map((m) => m.model).filter((m) => m !== r.model)); S.dataKey = ""; location.hash = "#/dashboard"; } }),
      h("a", { class: "btn", href: "#/prompts", text: "Its saved prompts", onclick: () => setTimeout(() => { const s = $("rec-model"); if (s) { s.value = r.model; s.dispatchEvent(new Event("change")); } }, 300) }));
    box.append(asks);
  }

  function exportCsv() {
    if (!V.table) return;
    const cols = V.table.cols.filter((c) => !c.nosort);
    const lines = [cols.map((c) => csvEscape(c.h)).join(",")].concat(V.table.rows.map((r) => cols.map((c) => { const v = c.v(r); return csvEscape(typeof v === "number" ? (v === -1 || Math.abs(v) >= 1e12 ? "" : Math.abs(v)) : v); }).join(",")));
    download("toktail-models.csv", new Blob([lines.join("\n") + "\n"], { type: "text/csv" }));
  }

  function init() {
    $("mdl-range").value = store.get("models.range") || "30";
    $("mdl-range").addEventListener("change", () => { store.set("models.range", $("mdl-range").value); show(); });
    $("mdl-host").addEventListener("change", () => show());
    $("mdl-split").addEventListener("click", (e) => { const b = e.target.closest("button"); if (!b) return; V.split = b.dataset.v; store.set("models.split", V.split); show(); });
    $("btn-lb-csv").addEventListener("click", exportCsv);
    document.addEventListener("click", (e) => { if (e.target.closest("#page-models [data-export]")) exportChart("#chart-scatter", null, "Speed against wait", "Each bubble is a model", "toktail-models-scatter.svg"); });
  }
  return { init, show, rerender: () => V.data && render() };
})();
