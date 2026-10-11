// SPDX-License-Identifier: Apache-2.0
// Toktail portal: charts, insights, the cloud-cost comparison, settings,
// and the stage where Pip runs, naps and replays your history.
"use strict";

// ================================================================== state
const S = {
  meta: null, daily: [], heat: [], recent: [], repeats: { repeated_requests: 0, top: [] }, costs: null,
  settings: null, live: null,
  range: "30", metric: "output", hidden: new Set(), tab: "daily",
  colorOf: new Map(), lastData: null, animate: true, dataKey: "",
};
const SLOTS = 8;
const $ = (id) => document.getElementById(id);
const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const store = {
  get(k) { try { return localStorage.getItem("toktail." + k); } catch (e) { return null; } },
  set(k, v) { try { localStorage.setItem("toktail." + k, v); } catch (e) {} },
};
S.range = store.get("range") || S.range;
S.metric = store.get("metric") || S.metric;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const reduced = () => Pip.reduceMotion.matches;

// ================================================================== format
const nf = new Intl.NumberFormat();
const fmt = (n) => nf.format(Math.round(n));
function compact(n) {
  const a = Math.abs(n);
  const f = (v, u) => (v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)).replace(/\.0+$|(\.\d*?)0+$/, "$1") + u;
  if (a >= 1e9) return f(n / 1e9, "B");
  if (a >= 1e6) return f(n / 1e6, "M");
  if (a >= 1e3) return f(n / 1e3, "K");
  return String(Math.round(n));
}
const tickFmt = (n) => (n === 0 ? "0" : compact(n));
const usdFmt = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const usdFmt0 = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 0 });
function usd(v) {
  if (v === 0) return "$0";
  if (Math.abs(v) < 0.01) return v < 0 ? "−<$0.01" : "<$0.01";
  return Math.abs(v) >= 1000 ? usdFmt0.format(v) : usdFmt.format(v);
}
const usdTick = (v) => (v === 0 ? "$0" : v < 1 ? "$" + v.toFixed(2).replace(/0$/, "") : "$" + compact(v));
const MONTHS = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
const DAYS = ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"];
const DAYS_LONG = ["Monday","Tuesday","Wednesday","Thursday","Friday","Saturday","Sunday"];
function parseD(s) { const [y, m, d] = s.split("-").map(Number); return new Date(Date.UTC(y, m - 1, d)); }
function isoD(d) { return d.toISOString().slice(0, 10); }
function addDays(s, n) { const d = parseD(s); d.setUTCDate(d.getUTCDate() + n); return isoD(d); }
function dayLabel(s) { const d = parseD(s); return MONTHS[d.getUTCMonth()] + " " + d.getUTCDate(); }
function longDay(s) { const d = parseD(s); return DAYS[(d.getUTCDay() + 6) % 7] + ", " + MONTHS[d.getUTCMonth()] + " " + d.getUTCDate() + ", " + d.getUTCFullYear(); }
function pct(x) { return (x >= 0 ? "+" : "−") + Math.abs(x * 100).toFixed(Math.abs(x) < 0.1 ? 1 : 0) + "%"; }

// ================================================================== data
function rangeDates() {
  const today = S.meta ? S.meta.today : isoD(new Date());
  switch (S.range) {
    case "today": return [today, today];
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
  return { today: "today", "7": "in the last 7 days", "30": "in the last 30 days", "90": "in the last 90 days", mtd: "this month", all: "since you started counting" }[S.range]
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
  return p.toString();
}
async function getJSON(url, opts) {
  const r = await fetch(url, { cache: "no-store", ...(opts || {}) });
  if (!r.ok) throw new Error(url + " answered " + r.status);
  return r.json();
}
async function load() {
  document.body.classList.add("loading");
  try {
    S.meta = await getJSON("/api/meta");
    assignColors();
    const q = qs();
    const key = q + "|" + S.metric;
    S.animate = key !== S.dataKey && !reduced();
    S.dataKey = key;
    const [daily, heat, recent, repeats, costs] = await Promise.all([
      getJSON("/api/daily?" + q),
      getJSON("/api/heatmap?" + q),
      getJSON("/api/recent?" + qs({ limit: 300 })),
      getJSON("/api/repeats?" + qs({ limit: 50 })),
      S.meta.costs_enabled ? getJSON("/api/costs?" + q) : Promise.resolve(null),
    ]);
    Object.assign(S, { daily, heat, recent, repeats, costs });
    render();
  } catch (e) {
    toast("Couldn't load your data: " + e.message);
  } finally {
    document.body.classList.remove("loading");
  }
}
// Colors follow the model (ranked by all-time output), never the filter.
function assignColors() {
  S.colorOf = new Map();
  S.meta.models.forEach((m, i) => S.colorOf.set(m.model, i < SLOTS ? "--s" + (i + 1) : "--other"));
}
const colorVar = (model) => S.colorOf.get(model) || "--other";
const color = (model) => css(colorVar(model));

function derive() {
  const [from, to] = rangeDates();
  const days = [];
  for (let d = from; d <= to; d = addDays(d, 1)) days.push(d);
  const models = S.meta.models.map((m) => m.model).filter((m) => !S.hidden.has(m));
  const present = new Set(S.daily.map((e) => e.model));
  const shown = models.filter((m) => present.has(m));
  const byDay = new Map(days.map((d) => [d, new Map()]));
  for (const e of S.daily) if (byDay.has(e.date)) byDay.get(e.date).set(e.model, e);
  const tot = { input: 0, output: 0, requests: 0, gen_out: 0, gen_ms: 0, errors: 0, estimated: 0 };
  const perModel = new Map();
  for (const e of S.daily) {
    for (const k in tot) tot[k] += e[k] || 0;
    const pm = perModel.get(e.model) || { input: 0, output: 0, requests: 0, gen_out: 0, gen_ms: 0 };
    for (const k in pm) pm[k] += e[k] || 0;
    perModel.set(e.model, pm);
  }
  const dayTotal = (d, key) => { let s = 0; for (const e of byDay.get(d).values()) s += e[key]; return s; };
  return { from, to, days, models: shown, byDay, tot, perModel, dayTotal };
}

// ================================================================== render
function render() {
  const D = derive();
  S.lastData = D;
  renderStatus();
  renderFilters();
  renderHero(D);
  renderDaily(D);
  renderInsights(D);
  renderCosts(D);
  renderModels(D);
  renderSpeed(D);
  renderCumulative(D);
  renderHeat(D);
  renderTable(D);
  renderFoot();
}

function renderStatus() {
  const m = S.meta;
  const pp = $("pill-proxy");
  pp.className = "pill " + (m.proxy.ok ? "ok" : "bad");
  pp.lastChild.textContent = m.proxy.ok ? "Counting on " + m.proxy.listen.replace(/^127\.0\.0\.1:/, "port ") : "Proxy not running";
  pp.title = m.proxy.message || "";
  const ps = $("pill-s3");
  if (!m.s3.enabled) { ps.className = "pill"; ps.lastChild.textContent = "S3 off"; ps.title = "Turn on [s3] in config.toml to mirror your CSV"; }
  else if (m.s3.last_error) { ps.className = "pill bad"; ps.lastChild.textContent = "S3 error"; ps.title = m.s3.last_error; }
  else if (m.s3.last_ok) { ps.className = "pill ok"; ps.lastChild.textContent = "S3 synced " + ago(m.s3.last_ok); ps.title = m.s3.target; }
  else { ps.className = "pill"; ps.lastChild.textContent = m.s3.in_progress ? "S3 syncing" : "S3 waiting"; ps.title = m.s3.target; }
  $("btn-sync").hidden = !m.s3.enabled;
  $("btn-theme").textContent = isDark() ? "Light mode" : "Dark mode";
}
function ago(iso) {
  const s = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return Math.floor(s / 60) + " min ago";
  if (s < 86400) return Math.floor(s / 3600) + " h ago";
  return Math.floor(s / 86400) + " d ago";
}
function isDark() {
  const t = document.documentElement.dataset.theme;
  return t ? t === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
}

function renderFilters() {
  for (const b of $("ranges").children) b.setAttribute("aria-pressed", String(b.dataset.r === S.range));
  $("custom").classList.toggle("on", S.range === "custom");
  for (const b of $("metric").children) b.setAttribute("aria-pressed", String(b.dataset.m === S.metric));
  const chips = $("chips");
  chips.textContent = "";
  for (const m of S.meta.models) {
    const c = document.createElement("span");
    c.className = "chip"; c.tabIndex = 0; c.setAttribute("role", "button");
    c.setAttribute("aria-pressed", String(!S.hidden.has(m.model)));
    c.title = fmt(m.output) + " output and " + fmt(m.input) + " input tokens all time";
    const sw = document.createElement("i"); sw.className = "sw"; sw.style.background = "var(" + colorVar(m.model) + ")";
    c.append(sw, document.createTextNode(m.model));
    const toggle = () => {
      if (S.hidden.has(m.model)) S.hidden.delete(m.model);
      else if (S.hidden.size < S.meta.models.length - 1) S.hidden.add(m.model);
      load();
    };
    c.onclick = toggle;
    c.onkeydown = (e) => { if (e.key === " " || e.key === "Enter") { e.preventDefault(); toggle(); } };
    chips.append(c);
  }
}

function span(text, cls) { const s = document.createElement("span"); if (cls) s.className = cls; s.textContent = text; return s; }
function bold(text) { const b = document.createElement("b"); b.textContent = text; return b; }

// Counts a number up to its value. One orchestrated moment, not decoration.
function countUp(el, to, ms = 1100) {
  const from = Number(el.dataset.v || 0);
  el.dataset.v = to;
  if (!S.animate || from === to) { el.textContent = fmt(to); return; }
  const t0 = performance.now();
  const tick = (now) => {
    const t = Math.min(1, (now - t0) / ms);
    const e = 1 - Math.pow(1 - t, 3);
    el.textContent = fmt(from + (to - from) * e);
    if (t < 1) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

function renderHero(D) {
  const key = S.metric;
  $("hero-lede").textContent = "Your host rendered, " + rangeName() + ",";
  countUp($("hero-num"), D.tot[key]);
  const u = $("hero-unit"); u.textContent = "";
  if (key === "output") u.append(document.createTextNode("output tokens, reading "), bold(fmt(D.tot.input)), document.createTextNode(" input tokens across "), bold(fmt(D.tot.requests)), document.createTextNode(" requests."));
  else u.append(document.createTextNode("input tokens, writing back "), bold(fmt(D.tot.output)), document.createTextNode(" output tokens across "), bold(fmt(D.tot.requests)), document.createTextNode(" requests."));

  const n = D.days.length, today = S.meta.today;
  const todayIn = D.byDay.has(today);
  let prev = 0, cnt = 0;
  for (let i = 1; i <= 7; i++) { const d = addDays(today, -i); if (D.byDay.has(d)) { prev += D.dayTotal(d, key); cnt++; } }
  const tps = D.tot.gen_ms > 0 ? D.tot.gen_out / (D.tot.gen_ms / 1000) : 0;
  const ratio = D.tot.output > 0 ? D.tot.input / D.tot.output : 0;
  const stats = [
    ["Today so far", todayIn ? compact(D.dayTotal(today, key)) : "—", todayIn && cnt ? "a typical day is " + compact(prev / cnt) : "today is outside this range"],
    ["Per day", compact(D.tot[key] / Math.max(1, n)), key + " tokens on average"],
    ["Right now", S.live ? compact(S.live.tokens_per_sec) + "/s" : "—", S.live ? (S.live.active_requests ? S.live.active_requests + " request" + (S.live.active_requests === 1 ? "" : "s") + " in flight" : "nothing streaming") : "", "live"],
    ["Speed", tps ? tps.toFixed(1) + " tok/s" : "—", "per request, all models"],
    ["Input per output", ratio ? ratio.toFixed(1) + " to 1" : "—", "prompt tokens per token generated"],
    ["Errors", fmt(D.tot.errors), D.tot.errors ? "or clients that hung up early" : "every request finished"],
  ];
  const box = $("stats"); box.textContent = "";
  for (const [k, v, d, id] of stats) {
    const s = document.createElement("div"); s.className = "stat";
    const kk = document.createElement("div"); kk.className = "k"; kk.textContent = k;
    const vv = document.createElement("div"); vv.className = "v"; vv.textContent = v; if (id) vv.id = "stat-" + id;
    const dd = document.createElement("div"); dd.className = "d"; dd.textContent = d; if (id) dd.id = "stat-" + id + "-d";
    s.append(kk, vv, dd); box.append(s);
  }
}

// ================================================================== svg helpers
const NS = "http://www.w3.org/2000/svg";
function el(tag, attrs, parent) {
  const e = document.createElementNS(NS, tag);
  for (const k in attrs) if (attrs[k] !== undefined && attrs[k] !== null) e.setAttribute(k, attrs[k]);
  if (parent) parent.append(e);
  return e;
}
function txt(parent, x, y, s, attrs) {
  const t = el("text", { x, y, fill: css("--muted"), "font-size": 11.5, ...(attrs || {}) }, parent);
  t.textContent = s;
  return t;
}
function niceMax(v, count) {
  if (!(v > 0)) return { max: 1, step: 1 };
  const raw = v / count, p = Math.pow(10, Math.floor(Math.log10(raw)));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * p).find((s) => s >= raw);
  return { max: Math.ceil(v / step - 1e-9) * step, step };
}
function newSvg(container, w, h, title) {
  container.textContent = "";
  const svg = el("svg", {
    viewBox: `0 0 ${w} ${h}`, width: w, height: h, role: "img", "aria-label": title,
    "font-family": 'system-ui, -apple-system, "Segoe UI", sans-serif',
  }, container);
  svg.dataset.title = title;
  return svg;
}
function yAxis(svg, x0, x1, y, max, step, fmtFn) {
  const g = el("g", {}, svg);
  for (let v = 0; v <= max + 1e-9; v += step) {
    const yy = y(v);
    el("line", { x1: x0, x2: x1, y1: yy, y2: yy, stroke: v === 0 ? css("--axis") : css("--grid"), "stroke-width": 1, "shape-rendering": "crispEdges" }, g);
    txt(g, x0 - 8, yy + 4, (fmtFn || tickFmt)(v), { "text-anchor": "end" });
  }
}
function xDateTicks(svg, days, xc, y, maxTicks) {
  const n = days.length, every = Math.max(1, Math.ceil(n / Math.max(1, maxTicks)));
  for (let i = 0; i < n; i++) if ((n - 1 - i) % every === 0) txt(svg, xc(i), y, dayLabel(days[i]), { "text-anchor": "middle" });
}
function topRounded(x, y, w, h, r) {
  r = Math.min(r, w / 2, h);
  return `M${x},${y + h}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h}Z`;
}
function rightRounded(x, y, w, h, r) {
  r = Math.min(r, h / 2, w);
  return `M${x},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + h - r}Q${x + w},${y + h} ${x + w - r},${y + h}H${x}Z`;
}
function legend(container, items) {
  container.textContent = "";
  if (items.length < 2) return;
  for (const it of items) {
    const s = document.createElement("span");
    const i = document.createElement("i");
    if (it.line) i.className = "line";
    i.style.background = it.color;
    s.append(i, document.createTextNode(it.label));
    container.append(s);
  }
}
function emptyState(container, msg) {
  container.textContent = "";
  const d = document.createElement("div"); d.className = "empty"; d.textContent = msg; container.append(d);
}
/** Lines draw themselves on first render of a range. */
function drawIn(path, delay = 0) {
  if (!S.animate) return;
  const len = Math.ceil(path.getTotalLength ? path.getTotalLength() : 1000);
  path.style.setProperty("--len", len);
  path.style.animationDelay = delay + "ms";
  path.classList.add("anim-line");
}

// ================================================================== tooltip
const tip = $("tip");
function showTip(evt, head, rows, total) {
  tip.textContent = "";
  const h = document.createElement("div"); h.className = "h"; h.textContent = head; tip.append(h);
  for (const r of rows) {
    const row = document.createElement("div"); row.className = "r";
    const i = document.createElement("i"); i.style.background = r.color || "transparent";
    const n = document.createElement("span"); n.className = "n"; n.textContent = r.label;
    const b = document.createElement("b"); b.textContent = r.value;
    row.append(i, n, b); tip.append(row);
  }
  if (total) {
    const row = document.createElement("div"); row.className = "r tot";
    const n = document.createElement("span"); n.className = "n"; n.textContent = total.label;
    const b = document.createElement("b"); b.textContent = total.value;
    row.append(document.createElement("i"), n, b); tip.append(row);
  }
  tip.style.display = "block";
  const w = tip.offsetWidth, hgt = tip.offsetHeight;
  let x = evt.clientX + 14, y = evt.clientY + 14;
  if (x + w > innerWidth - 8) x = evt.clientX - w - 14;
  if (y + hgt > innerHeight - 8) y = evt.clientY - hgt - 14;
  tip.style.left = Math.max(8, x) + "px"; tip.style.top = Math.max(8, y) + "px";
}
function hideTip() { tip.style.display = "none"; }

// ================================================================== chart: daily stacked columns
let dailyGeom = null; // kept for the trail replay
function renderDaily(D) {
  const box = $("chart-daily");
  const key = S.metric;
  $("daily-title").textContent = (key === "output" ? "Output" : "Input") + " tokens per day by model";
  $("daily-sub").textContent = fmt(D.tot[key]) + " " + key + " tokens " + rangeName() + ". The line is the 7-day average.";
  dailyGeom = null;
  if (!D.models.length) { legend($("legend-daily"), []); return emptyState(box, "Nothing counted in this range yet. Send a request through port 1234 and Pip will add it here."); }
  const W = Math.max(320, box.clientWidth), H = W > 700 ? 360 : 270;
  const m = { l: 50, r: 12, t: 16, b: 30 };
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
    const segs = D.models.map((mo) => ({ mo, v: per.has(mo) ? per.get(mo)[key] : 0 })).filter((s) => s.v > 0);
    let acc = 0;
    segs.forEach((s, si) => {
      const y0 = y(acc), y1 = y(acc + s.v);
      acc += s.v;
      const top = si === segs.length - 1;
      const h = y0 - y1;
      const hh = top ? h : Math.max(0, h - gap);
      if (hh <= 0) return;
      const yy = top ? y1 : y1 + gap;
      if (top) el("path", { d: topRounded(xc(i) - bw / 2, yy, bw, hh, 4), fill: color(s.mo) }, g);
      else el("rect", { x: xc(i) - bw / 2, y: yy, width: bw, height: hh, fill: color(s.mo) }, g);
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
      const rows = D.models.filter((mo) => per.has(mo)).map((mo) => ({ color: color(mo), label: mo, value: fmt(per.get(mo)[key]), v: per.get(mo)[key] })).sort((a, b) => b.v - a.v);
      showTip(e, longDay(d), rows.length ? rows : [{ label: "No requests", value: "" }], { label: "Total " + key + (n >= 7 ? ", 7-day avg " + compact(ma[i]) : ""), value: fmt(totals[i]) });
    });
    r.addEventListener("pointerleave", () => { hl.setAttribute("opacity", 0); hideTip(); });
  });
  const items = D.models.map((mo) => ({ label: mo, color: color(mo) }));
  if (n >= 7) items.push({ label: "7-day average, all models", color: css("--ink"), line: true });
  legend($("legend-daily"), items.length >= 2 ? items : []);
  dailyGeom = { svg, W, H, xc, y, base: y(0), totals, days: D.days };
}

// ================================================================== chart: share by model
function renderModels(D) {
  const box = $("chart-models");
  const key = S.metric;
  $("models-title").textContent = (key === "output" ? "Output" : "Input") + " tokens by model";
  const rows = D.models.map((mo) => ({ mo, v: D.perModel.get(mo)[key], other: D.perModel.get(mo)[key === "output" ? "input" : "output"], req: D.perModel.get(mo).requests })).sort((a, b) => b.v - a.v);
  const total = rows.reduce((s, r) => s + r.v, 0);
  $("models-sub").textContent = rows.length ? "Each model's share of " + fmt(total) + " " + key + " tokens" : "";
  if (!rows.length || total === 0) return emptyState(box, "No " + key + " tokens in this range.");
  hbars(box, $("models-title").textContent, rows.map((r) => ({
    label: r.mo, v: r.v, color: color(r.mo), value: compact(r.v), right: Math.round((r.v / total) * 1000) / 10 + "%",
    tip: () => [r.mo, [
      { color: color(r.mo), label: "Output", value: fmt(key === "output" ? r.v : r.other) },
      { color: color(r.mo), label: "Input", value: fmt(key === "input" ? r.v : r.other) },
      { label: "Requests", value: fmt(r.req) },
      { label: "Share of " + key, value: ((r.v / total) * 100).toFixed(1) + "%" },
    ]],
  })));
}

/** Horizontal bars with a label above each bar and the value at its tip. */
function hbars(box, title, rows) {
  const W = Math.max(300, box.clientWidth), rowH = 46, m = { l: 4, r: 78, t: 4, b: 4 };
  const H = m.t + m.b + rows.length * rowH;
  const svg = newSvg(box, W, H, title);
  const iw = W - m.l - m.r;
  const max = Math.max(...rows.map((r) => r.v), 1e-9);
  rows.forEach((r, i) => {
    const y0 = m.t + i * rowH;
    txt(svg, m.l, y0 + 15, r.label, { fill: css("--ink"), "font-size": 13, "font-weight": r.strong ? 700 : 400 });
    if (r.right) txt(svg, W - 4, y0 + 15, r.right, { "text-anchor": "end", fill: css("--ink-2"), "font-size": 12 });
    el("rect", { x: m.l, y: y0 + 23, width: iw, height: 12, fill: css("--surface-2"), rx: 4 }, svg);
    const bw = Math.max(r.v > 0 ? 3 : 0, (r.v / max) * iw);
    const bar = el("path", { d: rightRounded(m.l, y0 + 23, bw, 12, 4), fill: r.color }, svg);
    if (S.animate) { bar.classList.add("anim-hbar"); bar.style.animationDelay = i * 70 + "ms"; }
    txt(svg, W - 4, y0 + 34, r.value, { "text-anchor": "end", fill: css("--ink"), "font-weight": 700, "font-size": 12.5 });
    const hit = el("rect", { x: 0, y: y0, width: W, height: rowH, fill: "transparent", "data-ui": "1" }, svg);
    if (r.tip) {
      hit.addEventListener("pointermove", (e) => { const [h, rr] = r.tip(); showTip(e, h, rr); });
      hit.addEventListener("pointerleave", hideTip);
    }
  });
  return svg;
}

// ================================================================== line charts
function lineChart(box, days, series, opts) {
  const W = Math.max(300, box.clientWidth), H = opts.height || 250;
  const m = { l: 52, r: opts.endLabels ? 74 : 14, t: 12, b: 30 };
  const svg = newSvg(box, W, H, opts.title);
  const n = days.length;
  const vals = series.flatMap((s) => s.values.filter((v) => v != null));
  const { max, step } = niceMax(Math.max(...vals, 1e-9), 4);
  const iw = W - m.l - m.r, ih = H - m.t - m.b;
  const x = (i) => m.l + (n === 1 ? iw / 2 : (i / (n - 1)) * iw);
  const y = (v) => m.t + ih - (v / max) * ih;
  yAxis(svg, m.l, W - m.r, y, max, step, opts.tickFmt);
  xDateTicks(svg, days, x, H - 9, Math.floor(iw / 72));
  const surface = css("--surface");
  if (opts.between) {
    // Fill between two series (e.g. savings between cloud price and electricity).
    const [a, b] = opts.between.map((k) => series[k].values);
    let d = "";
    a.forEach((v, i) => { d += (i ? "L" : "M") + x(i).toFixed(1) + "," + y(v).toFixed(1); });
    for (let i = n - 1; i >= 0; i--) d += "L" + x(i).toFixed(1) + "," + y(b[i]).toFixed(1);
    const p = el("path", { d: d + "Z", fill: opts.betweenColor, opacity: 0.16 }, svg);
    if (S.animate) { p.classList.add("anim-fade"); p.style.animationDelay = "500ms"; }
  }
  series.forEach((s, si) => {
    let d = "", pen = false;
    s.values.forEach((v, i) => {
      if (v == null) { pen = false; return; }
      d += (pen ? "L" : "M") + x(i).toFixed(1) + "," + y(v).toFixed(1); pen = true;
    });
    if (s.area) {
      const a = el("path", { d: d + `L${x(n - 1)},${y(0)}L${x(0)},${y(0)}Z`, fill: s.color, opacity: 0.1, stroke: "none" }, svg);
      if (S.animate) a.classList.add("anim-fade");
    }
    const p = el("path", { d, fill: "none", stroke: s.color, "stroke-width": 2, "stroke-linejoin": "round", "stroke-linecap": "round" }, svg);
    drawIn(p, si * 120);
    s.values.forEach((v, i) => {
      if (v == null) return;
      const lone = (i === 0 || s.values[i - 1] == null) && (i === n - 1 || s.values[i + 1] == null);
      if (lone) el("circle", { cx: x(i), cy: y(v), r: 4, fill: s.color, stroke: surface, "stroke-width": 2 }, svg);
    });
  });
  if (opts.endLabels && series.length <= 4) {
    const ends = series.map((s) => { let i = s.values.length - 1; while (i >= 0 && s.values[i] == null) i--; return i >= 0 ? { s, i, yy: y(s.values[i]) } : null; }).filter(Boolean).sort((a, b) => a.yy - b.yy);
    const clear = ends.every((e, k) => k === 0 || e.yy - ends[k - 1].yy >= 14);
    if (clear) for (const e of ends) {
      el("circle", { cx: x(e.i), cy: e.yy, r: 4, fill: e.s.color, stroke: surface, "stroke-width": 2 }, svg);
      const t = txt(svg, W - m.r + 8, e.yy + 4, (opts.valueFmt || compact)(e.s.values[e.i]), { fill: css("--ink-2"), "font-weight": 700 });
      if (S.animate) { t.classList.add("anim-fade"); t.style.animationDelay = "900ms"; }
    }
  }
  const ui = el("g", { "data-ui": "1" }, svg);
  const cross = el("line", { x1: 0, x2: 0, y1: m.t, y2: m.t + ih, stroke: css("--muted"), "stroke-width": 1, opacity: 0 }, ui);
  const dots = series.map((s) => el("circle", { r: 4, fill: s.color, stroke: surface, "stroke-width": 2, opacity: 0 }, ui));
  const ov = el("rect", { x: m.l, y: m.t, width: iw, height: ih, fill: "transparent" }, ui);
  ov.addEventListener("pointermove", (e) => {
    const r = svg.getBoundingClientRect();
    const px = (e.clientX - r.left) * (W / r.width);
    const i = Math.max(0, Math.min(n - 1, Math.round(n === 1 ? 0 : ((px - m.l) / iw) * (n - 1))));
    cross.setAttribute("x1", x(i)); cross.setAttribute("x2", x(i)); cross.setAttribute("opacity", 1);
    series.forEach((s, k) => {
      const v = s.values[i];
      if (v == null) dots[k].setAttribute("opacity", 0);
      else { dots[k].setAttribute("cx", x(i)); dots[k].setAttribute("cy", y(v)); dots[k].setAttribute("opacity", 1); }
    });
    const rows = series.map((s) => ({ color: s.color, label: s.label, value: s.values[i] == null ? "—" : (opts.valueFmt || fmt)(s.values[i]), v: s.values[i] ?? -1 })).sort((a, b) => b.v - a.v);
    showTip(e, longDay(days[i]), rows, opts.tipTotal ? opts.tipTotal(i) : null);
  });
  ov.addEventListener("pointerleave", () => { cross.setAttribute("opacity", 0); dots.forEach((d) => d.setAttribute("opacity", 0)); hideTip(); });
  return svg;
}

function renderSpeed(D) {
  const box = $("chart-speed");
  const series = D.models.map((mo) => ({
    label: mo, color: color(mo),
    values: D.days.map((d) => { const e = D.byDay.get(d).get(mo); return e && e.gen_ms > 0 ? e.gen_out / (e.gen_ms / 1000) : null; }),
  })).filter((s) => s.values.some((v) => v != null));
  legend($("legend-speed"), series.map((s) => ({ label: s.label, color: s.color, line: true })));
  if (!series.length) return emptyState(box, "No timed generations in this range.");
  lineChart(box, D.days, series, { title: "Generation speed (tokens per second)", endLabels: true, valueFmt: (v) => v.toFixed(1) + " t/s", tickFmt: (v) => String(Math.round(v)) });
}

function renderCumulative(D) {
  const box = $("chart-cumulative");
  let ci = 0, co = 0;
  const inp = [], out = [];
  for (const d of D.days) { ci += D.dayTotal(d, "input"); co += D.dayTotal(d, "output"); inp.push(ci); out.push(co); }
  // Neutral inks: input and output are measures, not models.
  const series = [
    { label: "Input tokens", color: css("--muted"), values: inp, area: true },
    { label: "Output tokens", color: css("--ink"), values: out, area: true },
  ];
  legend($("legend-cum"), series.map((s) => ({ label: s.label, color: s.color, line: true })));
  if (ci + co === 0) return emptyState(box, "No tokens in this range.");
  lineChart(box, D.days, series, { title: "Running total of tokens", endLabels: true });
}

// ================================================================== chart: heatmap
function renderHeat(D) {
  const box = $("chart-heat");
  const key = S.metric;
  $("heat-title").textContent = "When your host works";
  const vals = S.heat.map((c) => c[key]);
  const max = Math.max(...vals, 0);
  if (max === 0) return emptyState(box, "No activity in this range.");
  const W = Math.max(300, box.clientWidth);
  const m = { l: 36, r: 6, t: 4, b: 50 };
  const cw = (W - m.l - m.r) / 24, ch = Math.min(28, Math.max(14, cw));
  const H = m.t + ch * 7 + m.b;
  const svg = newSvg(box, W, H, "When your host works, " + key + " tokens by weekday and hour");
  const steps = [1, 2, 3, 4, 5, 6, 7].map((i) => css("--seq-" + i));
  const zero = css("--seq-0");
  const bin = (v) => (v <= 0 ? -1 : Math.min(6, Math.floor(Math.sqrt(v / max) * 7)));
  for (let wd = 0; wd < 7; wd++) txt(svg, m.l - 6, m.t + wd * ch + ch / 2 + 4, DAYS[wd], { "text-anchor": "end" });
  for (let h = 0; h < 24; h += 3) txt(svg, m.l + h * cw + cw / 2, m.t + ch * 7 + 15, h === 0 ? "12a" : h < 12 ? h + "a" : h === 12 ? "12p" : (h - 12) + "p", { "text-anchor": "middle" });
  for (const c of S.heat) {
    const b = bin(c[key]);
    const r = el("rect", { x: m.l + c.hour * cw + 1, y: m.t + c.weekday * ch + 1, width: Math.max(1, cw - 2), height: Math.max(1, ch - 2), rx: 4, fill: b < 0 ? zero : steps[b] }, svg);
    if (S.animate) { r.classList.add("anim-fade"); r.style.animationDelay = (c.hour * 18 + c.weekday * 30) + "ms"; }
    r.addEventListener("pointermove", (e) => {
      r.setAttribute("stroke", css("--ink")); r.setAttribute("stroke-width", 1.5);
      const hh = String(c.hour).padStart(2, "0");
      showTip(e, DAYS_LONG[c.weekday] + "s, " + hh + ":00 to " + hh + ":59", [
        { label: "Output tokens", value: fmt(c.output) }, { label: "Input tokens", value: fmt(c.input) }, { label: "Requests", value: fmt(c.requests) },
      ]);
    });
    r.addEventListener("pointerleave", () => { r.removeAttribute("stroke"); hideTip(); });
  }
  const ly = m.t + ch * 7 + 28, lw = Math.min(180, W - m.l - 150);
  txt(svg, m.l, ly + 9, "Less");
  const sx = m.l + 34;
  steps.forEach((c, i) => el("rect", { x: sx + i * (lw / 7), y: ly, width: lw / 7 - 2, height: 10, rx: 2, fill: c }, svg));
  txt(svg, sx + lw + 6, ly + 9, "More (busiest hour " + compact(max) + " " + key + ")");
}

// ================================================================== insights
function renderInsights(D) {
  const ul = $("insights"); ul.textContent = "";
  const add = (icon, parts) => {
    const li = document.createElement("li");
    const ic = document.createElement("span"); ic.className = "ic"; ic.textContent = icon; ic.setAttribute("aria-hidden", "true");
    const t = document.createElement("span");
    for (const p of parts) t.append(typeof p === "string" ? document.createTextNode(p) : p);
    li.append(ic, t); ul.append(li);
  };
  if (!D.tot.requests) { add("z", ["Nothing counted in this range. Pip is napping."]); return; }
  const key = S.metric, today = S.meta.today;
  const totals = D.days.map((d) => D.dayTotal(d, key));
  if (D.days.length >= 14) {
    const a = totals.slice(-7).reduce((s, v) => s + v, 0), b = totals.slice(-14, -7).reduce((s, v) => s + v, 0);
    if (b > 0) add(a >= b ? "↗" : "↘", ["The last 7 days ran ", bold(pct(a / b - 1)), " against the 7 before (" + compact(a) + " vs " + compact(b) + ")."]);
  }
  const pi = totals.indexOf(Math.max(...totals));
  if (totals[pi] > 0 && D.days.length > 1) add("▲", ["Busiest day was ", bold(longDay(D.days[pi])), ", with " + fmt(totals[pi]) + " " + key + " tokens."]);
  const active = totals.filter((v) => v > 0).sort((x, y) => x - y);
  if (active.length >= 7) {
    const med = active[Math.floor(active.length / 2)];
    const mad = active.map((v) => Math.abs(v - med)).sort((x, y) => x - y)[Math.floor(active.length / 2)] || 1;
    const odd = D.days.map((d, i) => ({ d, v: totals[i], z: (totals[i] - med) / (1.4826 * mad) })).filter((o) => o.v > 0 && Math.abs(o.z) > 3.5).sort((a, b) => Math.abs(b.z) - Math.abs(a.z)).slice(0, 3);
    if (odd.length) add("!", ["Out of the ordinary: ", ...odd.flatMap((o, k) => [k ? ", " : "", bold(dayLabel(o.d)), " (" + (o.v > med ? (o.v / med).toFixed(1) + "× a typical day" : "unusually quiet") + ")"]), "."]);
    else add("✓", ["Steady going. A typical day is " + compact(med) + " " + key + " tokens and nothing stood out."]);
  }
  const pm = [...D.perModel.entries()].sort((a, b) => b[1][key] - a[1][key]);
  if (pm.length > 1 && D.tot[key] > 0) add("◆", [bold(pm[0][0]), " did " + Math.round((pm[0][1][key] / D.tot[key]) * 100) + "% of the work across " + pm.length + " models."]);
  const speeds = pm.filter(([, v]) => v.gen_ms > 0 && v.gen_out > 50).map(([k, v]) => [k, v.gen_out / (v.gen_ms / 1000)]).sort((a, b) => b[1] - a[1]);
  if (speeds.length > 1) add("⚡", ["Fastest is ", bold(speeds[0][0]), " at " + speeds[0][1].toFixed(1) + " tok/s. Slowest is ", bold(speeds[speeds.length - 1][0]), " at " + speeds[speeds.length - 1][1].toFixed(1) + " tok/s."]);
  const month = today.slice(0, 7);
  if (D.byDay.has(today) && D.from <= month + "-01") {
    let mtd = 0, last14 = 0, c14 = 0;
    for (const d of D.days) if (d.startsWith(month)) mtd += D.dayTotal(d, key);
    for (let i = 1; i <= 14; i++) { const d = addDays(today, -i); if (D.byDay.has(d)) { last14 += D.dayTotal(d, key); c14++; } }
    const dim = new Date(Date.UTC(+month.slice(0, 4), +month.slice(5, 7), 0)).getUTCDate();
    const left = dim - +today.slice(8, 10);
    if (c14 >= 3 && left > 0) add("→", ["On pace for about ", bold(compact(mtd + (last14 / c14) * left)), " " + key + " tokens this month, with " + compact(mtd) + " so far."]);
  }
  const ph = [...S.heat].sort((a, b) => b[key] - a[key])[0];
  if (ph && ph[key] > 0) add("◷", ["Busiest hour is ", bold(DAYS_LONG[ph.weekday] + "s at " + String(ph.hour).padStart(2, "0") + ":00"), "."]);
  if (S.repeats.repeated_requests > 0) {
    const share = S.repeats.repeated_requests / D.tot.requests;
    add("↺", [bold(fmt(S.repeats.repeated_requests)), " requests (" + (share * 100).toFixed(1) + "%) repeated a prompt Pip had already seen. A response cache could answer those instantly."]);
  }
  if (D.tot.estimated) add("≈", [fmt(D.tot.estimated) + " requests were counted from the stream because LM Studio didn't report usage, so their input shows as 0."]);
}

// ================================================================== costs
function renderCosts(D) {
  const card = $("card-costs");
  const R = S.costs;
  card.hidden = !(S.meta.costs_enabled && R);
  if (card.hidden) return;
  const compared = R.models.filter((m) => m.compared).sort((a, b) => b.total - a.total);
  const base = R.models.find((m) => m.id === R.baseline) || compared[0];
  const sel = $("baseline"); sel.textContent = "";
  for (const m of R.models) {
    const o = document.createElement("option"); o.value = m.id; o.textContent = m.provider + " " + m.name.replace(m.provider + " ", "");
    if (m.id === base.id) o.selected = true;
    sel.append(o);
  }
  $("costs-sub").textContent = "List prices as of " + R.as_of + ", for the requests " + rangeName();
  const local = R.local;
  const saved = base.total - local.cost;
  const lead = $("costs-lead"); lead.textContent = "";
  const why = $("costs-why"); why.textContent = "";
  if (!R.requests) {
    lead.textContent = "Nothing to price in this range yet.";
  } else {
    lead.append(document.createTextNode(saved >= 0 ? "Running locally saved about " : "Running locally cost about "), span(usd(Math.abs(saved)), "save"),
      document.createTextNode(saved >= 0 ? " compared with " + base.name + "." : " more than " + base.name + " would have."));
    why.textContent = base.name + " would have charged " + usd(base.total) + " for " + fmt(R.input_tokens) + " input and " + fmt(R.output_tokens) +
      " output tokens. Your host spent " + local.busy_hours.toFixed(1) + " hours generating, which at " + Math.round(local.watts) + " W and " +
      usd(local.usd_per_kwh) + " per kWh comes to about " + usd(local.cost) + " of electricity.";
    if (R.hardware_cost_usd > 0 && D.days.length) {
      const perDay = saved / D.days.length;
      const p = document.createElement("span");
      p.textContent = perDay > 0
        ? " At this pace the host pays for itself in about " + Math.max(1, Math.round(R.hardware_cost_usd / (perDay * 30.44))) + " months against " + base.name + "."
        : " At this pace the savings don't cover the host's " + usd0(R.hardware_cost_usd) + " price.";
      why.append(p);
    }
  }
  // Bars: every compared service, the baseline highlighted, your electricity in mint.
  const rows = compared.map((m) => ({
    label: m.provider + " " + m.name.replace(m.provider + " ", ""), v: m.total, strong: m.id === base.id,
    color: m.id === base.id ? css("--lav") : css("--neutral-bar"), value: usd(m.total),
    right: local.cost > 0 && m.total > 0 ? Math.round(m.total / local.cost) + "× your electricity" : "",
    tip: () => [m.provider + " " + m.name, [
      { label: "Input at $" + m.input_rate + "/M", value: usd(m.input_cost) },
      { label: "Output at $" + m.output_rate + "/M", value: usd(m.output_cost) },
      ...(m.request_cost ? [{ label: "Request fees", value: usd(m.request_cost) }] : []),
      { label: "Saved by running locally", value: usd(m.total - local.cost) },
    ]],
  }));
  rows.push({ label: "Your host (electricity)", v: local.cost, strong: true, color: css("--mint"), value: usd(local.cost), right: "",
    tip: () => ["Your host", [{ label: "Hours generating", value: local.busy_hours.toFixed(2) }, { label: "Energy", value: local.kwh.toFixed(2) + " kWh" }, { label: "At " + usd(local.usd_per_kwh) + "/kWh", value: usd(local.cost) }]] });
  const box = $("chart-costbars");
  legend($("legend-costbars"), [{ label: base.name + " (comparing with)", color: css("--lav") }, { label: "Other services", color: css("--neutral-bar") }, { label: "Your electricity", color: css("--mint") }]);
  if (!R.requests) emptyState(box, "Prices appear once there are requests in this range.");
  else hbars(box, "What this range would cost on each service", rows);

  // Running total: the baseline's price against your electricity.
  const days = D.days;
  const byDate = new Map(R.daily.map((d) => [d.date, d]));
  let cb = 0, cl = 0;
  const vb = [], vl = [];
  for (const d of days) { const e = byDate.get(d); cb += e ? e.models[base.id] || 0 : 0; cl += e ? e.local : 0; vb.push(cb); vl.push(cl); }
  const cumBox = $("chart-costcum");
  $("costcum-title").textContent = "Running total against " + base.name;
  const series = [{ label: base.name, color: css("--lav"), values: vb }, { label: "Your electricity", color: css("--mint"), values: vl }];
  legend($("legend-costcum"), series.map((s) => ({ label: s.label, color: s.color, line: true })).concat([{ label: "Saved", color: "color-mix(in srgb, " + css("--mint") + " 30%, transparent)" }]));
  if (cb + cl === 0) emptyState(cumBox, "No priced requests in this range.");
  else lineChart(cumBox, days, series, { title: "Running total cost", endLabels: true, valueFmt: usd, tickFmt: usdTick, between: [0, 1], betweenColor: css("--mint"),
    tipTotal: (i) => ({ label: "Saved so far", value: usd(vb[i] - vl[i]) }) });

  // Table of every compared service.
  const tb = $("costs-table"); tb.textContent = "";
  const cols = [
    { h: "Service", l: true, v: (m) => m.provider },
    { h: "Model", l: true, v: (m) => m.name },
    { h: "Input $/1M", v: (m) => "$" + m.input_rate },
    { h: "Output $/1M", v: (m) => "$" + m.output_rate },
    { h: "Per request", v: (m) => (m.per_request ? "$" + m.per_request : "—") },
    { h: "This range", v: (m) => usd(m.total) },
    { h: "You saved", v: (m) => usd(m.total - local.cost) },
  ];
  const t = table(cols, compared);
  [...t.tBodies[0].rows].forEach((tr, i) => { if (compared[i].id === base.id) tr.className = "hl"; });
  tb.append(t);

  const notes = $("costs-notes"); notes.textContent = "";
  const lines = [...R.notes];
  if (R.excluded_embedding_requests) lines.push(fmt(R.excluded_embedding_requests) + " embedding requests are left out, since chat prices don't apply to them.");
  for (const m of compared) if (m.note) lines.push(m.name + ": " + m.note);
  for (const l of lines) { const d = document.createElement("div"); d.textContent = l; notes.append(d); }
  const src = document.createElement("div");
  src.append(document.createTextNode("Prices from " + (R.origin === "built-in" ? "Toktail's built-in list" : R.origin) + ": "));
  const seen = new Set();
  compared.forEach((m) => {
    if (!m.source || seen.has(m.provider)) return;
    seen.add(m.provider);
    if (seen.size > 1) src.append(document.createTextNode(", "));
    const a = document.createElement("a"); a.href = m.source; a.target = "_blank"; a.rel = "noopener noreferrer"; a.textContent = m.provider;
    src.append(a);
  });
  src.append(document.createTextNode("."));
  notes.append(src);
}
const usd0 = (v) => usdFmt0.format(v);

// ================================================================== tables
function table(cols, rows) {
  const t = document.createElement("table");
  const thead = t.createTHead().insertRow();
  for (const c of cols) { const th = document.createElement("th"); th.textContent = c.h; if (c.l) th.className = "l"; thead.append(th); }
  const tb = t.createTBody();
  for (const r of rows) {
    const tr = tb.insertRow();
    for (const c of cols) {
      const td = tr.insertCell();
      if (c.l) td.className = "l";
      const v = c.v(r);
      if (v instanceof Node) td.append(v); else td.textContent = v;
    }
  }
  return t;
}
function fpCell(h) {
  if (!h) return document.createTextNode("—");
  const c = document.createElement("code"); c.className = "fp"; c.textContent = h.slice(0, 12) + "…"; c.title = h + " (click to copy)";
  c.onclick = () => { navigator.clipboard && navigator.clipboard.writeText(h).then(() => toast("Fingerprint copied")); };
  return c;
}
function swatchText(model) {
  const s = document.createElement("span"); s.style.cssText = "display:inline-flex;align-items:center;gap:6px";
  const i = document.createElement("i"); i.style.cssText = "width:8px;height:8px;border-radius:2px;display:inline-block;background:var(" + colorVar(model) + ")";
  s.append(i, document.createTextNode(model)); return s;
}
let currentTable = { cols: [], rows: [], name: "table" };
function renderTable(D) {
  for (const b of $("tabs").children) b.setAttribute("aria-selected", String(b.dataset.t === S.tab));
  const box = $("table"); box.textContent = "";
  let cols, rows, name;
  if (S.tab === "daily") {
    const key = S.metric;
    rows = [...D.days].reverse().map((d) => ({ d, per: D.byDay.get(d) })).filter((r) => r.per.size > 0);
    cols = [{ h: "Date", l: true, v: (r) => r.d, raw: (r) => r.d }];
    for (const mo of D.models) cols.push({ h: mo + " (" + key + ")", v: (r) => fmt(r.per.has(mo) ? r.per.get(mo)[key] : 0), raw: (r) => (r.per.has(mo) ? r.per.get(mo)[key] : 0) });
    cols.push({ h: "Total output", v: (r) => fmt(D.dayTotal(r.d, "output")), raw: (r) => D.dayTotal(r.d, "output") });
    cols.push({ h: "Total input", v: (r) => fmt(D.dayTotal(r.d, "input")), raw: (r) => D.dayTotal(r.d, "input") });
    cols.push({ h: "Requests", v: (r) => fmt(D.dayTotal(r.d, "requests")), raw: (r) => D.dayTotal(r.d, "requests") });
    name = "daily-by-model";
  } else if (S.tab === "recent") {
    rows = S.recent;
    cols = [
      { h: "Time", l: true, v: (r) => r.timestamp.replace("T", " ").slice(0, 19), raw: (r) => r.timestamp },
      { h: "Model", l: true, v: (r) => swatchText(r.model), raw: (r) => r.model },
      { h: "Endpoint", l: true, v: (r) => r.endpoint, raw: (r) => r.endpoint },
      { h: "Input", v: (r) => fmt(r.input), raw: (r) => r.input },
      { h: "Output", v: (r) => fmt(r.output), raw: (r) => r.output },
      { h: "tok/s", v: (r) => (r.tokens_per_sec == null ? "—" : r.tokens_per_sec.toFixed(1)), raw: (r) => r.tokens_per_sec ?? "" },
      { h: "First token", v: (r) => (r.ttft_ms == null ? "—" : r.ttft_ms + " ms"), raw: (r) => r.ttft_ms ?? "" },
      { h: "Duration", v: (r) => (r.duration_ms / 1000).toFixed(2) + " s", raw: (r) => r.duration_ms },
      { h: "Status", v: (r) => span(String(r.status) + (r.status === 499 ? " client left" : ""), "tag" + (r.status >= 400 ? " err" : "")), raw: (r) => r.status },
      { h: "Counted", v: (r) => r.source, raw: (r) => r.source },
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
  if (!rows.length) return emptyState(box, S.tab === "repeats" ? "No prompt was sent twice in this range." : "No rows in this range.");
  box.append(table(cols, rows));
}

function renderFoot() {
  const f = $("foot"); f.textContent = "";
  const p = (parts) => { const d = document.createElement("div"); for (const x of parts) d.append(typeof x === "string" ? document.createTextNode(x) : x); f.append(d); };
  const code = (s) => { const c = document.createElement("code"); c.textContent = s; return c; };
  p(["Your CSV is the source of truth: ", code(S.meta.data_dir), ", " + S.meta.files.length + " file" + (S.meta.files.length === 1 ? "" : "s") + ", " + fmt(S.meta.rows) + " requests. Prompts are never stored, only their SHA-256 fingerprints."]);
  p(["Days run midnight to midnight local time (UTC" + (S.meta.utc_offset_minutes >= 0 ? "+" : "−") + Math.abs(S.meta.utc_offset_minutes / 60) + "). This page refreshes every 30 seconds."]);
  const a = document.createElement("a"); a.href = S.meta.repository; a.target = "_blank"; a.rel = "noopener noreferrer"; a.textContent = "github.com/playandprosper/toktail";
  p(["Toktail " + S.meta.version + " is open source under the Apache 2.0 license: ", a, "."]);
  if (!S.meta.costs_enabled) {
    const b = document.createElement("a"); b.href = "#"; b.textContent = "Turn on the cloud cost comparison";
    b.onclick = (e) => { e.preventDefault(); openSettings(); };
    p([b, " to see what these tokens would cost on Claude, GPT, Grok, Gemini and others."]);
  }
}

// ================================================================== export
function download(name, blob) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob); a.download = name;
  document.body.append(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 2000);
}
const EXPORTS = {
  daily: ["card-daily", "#chart-daily", "#legend-daily"], models: ["card-models", "#chart-models", null],
  speed: ["card-speed", "#chart-speed", "#legend-speed"], cumulative: ["card-cumulative", "#chart-cumulative", "#legend-cum"],
  heat: ["card-heat", "#chart-heat", null], costbars: ["card-costs", "#chart-costbars", "#legend-costbars"],
  costcum: ["card-costs", "#chart-costcum", "#legend-costcum"],
};
function exportSvg(which) {
  const [card, chartSel, legendSel] = EXPORTS[which];
  const svg = document.querySelector(chartSel + " svg");
  if (!svg) return toast("Nothing to export in this range.");
  const W = +svg.getAttribute("width"), H = +svg.getAttribute("height");
  const legendItems = legendSel ? [...document.querySelectorAll(legendSel + " span")].map((s) => ({ label: s.textContent, color: s.querySelector("i").style.background, line: s.querySelector("i").classList.contains("line") })) : [];
  const perRow = Math.max(1, Math.floor(W / 220));
  const pad = 22, headH = 50, legH = legendItems.length ? 22 * Math.ceil(legendItems.length / perRow) : 0, footH = 26;
  const OW = W + pad * 2, OH = H + headH + legH + pad * 2 + footH;
  const out = document.createElementNS(NS, "svg");
  out.setAttribute("xmlns", NS);
  out.setAttribute("width", OW); out.setAttribute("height", OH); out.setAttribute("viewBox", `0 0 ${OW} ${OH}`);
  out.setAttribute("font-family", 'system-ui, -apple-system, "Segoe UI", sans-serif');
  el("rect", { x: 0, y: 0, width: OW, height: OH, fill: css("--surface"), rx: 18 }, out);
  const title = svg.dataset.title || document.querySelector("#" + card + " h2").textContent;
  txt(out, pad, pad + 16, title, { "font-size": 17, "font-weight": 800, fill: css("--ink"), "font-family": 'ui-rounded, "SF Pro Rounded", system-ui, sans-serif' });
  const [f, t] = rangeDates();
  txt(out, pad, pad + 36, dayLabel(f) + ", " + f.slice(0, 4) + " to " + dayLabel(t) + ", " + t.slice(0, 4), { "font-size": 12, fill: css("--ink-2") });
  legendItems.forEach((it, i) => {
    const lx = pad + (i % perRow) * 220, ly = pad + headH + Math.floor(i / perRow) * 22;
    if (it.line) el("rect", { x: lx, y: ly + 4, width: 14, height: 3, rx: 1.5, fill: it.color }, out);
    else el("rect", { x: lx, y: ly, width: 10, height: 10, rx: 2, fill: it.color }, out);
    txt(out, lx + 20, ly + 9.5, it.label, { "font-size": 12, fill: css("--ink-2") });
  });
  const g = el("g", { transform: `translate(${pad},${pad + headH + legH})` }, out);
  const clone = svg.cloneNode(true);
  clone.querySelectorAll("[data-ui]").forEach((n) => n.remove());
  clone.querySelectorAll("[class]").forEach((n) => { n.removeAttribute("class"); n.removeAttribute("style"); });
  for (const c of [...clone.childNodes]) g.append(c);
  txt(out, OW - pad, OH - pad + 4, "Counted by Toktail. Every token leaves a trail.", { "text-anchor": "end", "font-size": 11, fill: css("--muted") });
  const xml = '<?xml version="1.0" encoding="UTF-8"?>\n' + new XMLSerializer().serializeToString(out);
  download(`toktail-${which}-${f}-to-${t}.svg`, new Blob([xml], { type: "image/svg+xml" }));
  Stage.cheer("Exported! That chart is all yours.");
}
function csvEscape(v) { const s = String(v ?? ""); return /[",\n]/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s; }
function exportTableCsv() {
  const { cols, rows, name } = currentTable;
  const lines = [cols.map((c) => csvEscape(c.h)).join(",")].concat(rows.map((r) => cols.map((c) => csvEscape(c.raw(r))).join(",")));
  const [f, t] = rangeDates();
  download(`toktail-${name}-${f}-to-${t}.csv`, new Blob([lines.join("\n") + "\n"], { type: "text/csv" }));
}

// ================================================================== settings
async function openSettings() {
  let st;
  try { st = await getJSON("/api/settings"); } catch (e) { return toast("Couldn't load settings: " + e.message); }
  S.settings = st;
  const body = $("settings-body"); body.textContent = "";
  const fs = (title, hint) => {
    const f = document.createElement("fieldset");
    const l = document.createElement("legend"); l.textContent = title; f.append(l);
    if (hint) { const p = document.createElement("div"); p.className = "label-sm"; p.textContent = hint; p.style.marginBottom = "6px"; f.append(p); }
    body.append(f); return f;
  };
  const toggle = (parent, id, label, hint, checked) => {
    const r = document.createElement("label"); r.className = "row"; r.htmlFor = id;
    const t = document.createElement("span"); t.append(document.createTextNode(label));
    if (hint) { const h = document.createElement("div"); h.className = "hint"; h.textContent = hint; t.append(h); }
    const i = document.createElement("input"); i.type = "checkbox"; i.className = "switch"; i.id = id; i.checked = checked;
    r.append(t, i); parent.append(r); return i;
  };
  const number = (parent, id, label, hint, value, step, unit) => {
    const r = document.createElement("label"); r.className = "row"; r.htmlFor = id;
    const t = document.createElement("span"); t.append(document.createTextNode(label));
    if (hint) { const h = document.createElement("div"); h.className = "hint"; h.textContent = hint; t.append(h); }
    const w = document.createElement("span"); w.style.cssText = "display:inline-flex;gap:6px;align-items:center";
    const i = document.createElement("input"); i.type = "number"; i.id = id; i.min = "0"; i.step = step; i.value = value;
    w.append(i, span(unit, "label-sm"));
    r.append(t, w); parent.append(r); return i;
  };
  const pipF = fs("Pip");
  toggle(pipF, "set-intro", "Run out from the middle when the portal opens", "The intro plays once each time you open the portal.", st.portal.intro_animation);
  toggle(pipF, "set-roams", "Hop along the trail while tokens stream", "Off keeps Pip in one spot, tapping its feet.", st.portal.pip_roams);
  number(pipF, "set-stampede", "Stampede at", "0 means automatic: three times your usual speed.", st.app.stampede_tokens_per_sec, "10", "tokens/sec");

  const costF = fs("Cloud cost comparison", "Prices what you ran locally at public API list prices (" + st.prices.as_of + "), next to the electricity your host used.");
  toggle(costF, "set-costs", "Show the cost comparison", null, st.costs.show_comparison);
  const r = document.createElement("label"); r.className = "row"; r.htmlFor = "set-baseline";
  const sel = document.createElement("select"); sel.id = "set-baseline";
  for (const m of st.prices.models) { const o = document.createElement("option"); o.value = m.id; o.textContent = m.provider + " " + m.name.replace(m.provider + " ", ""); o.selected = m.id === st.costs.baseline; sel.append(o); }
  r.append(span("Measure savings against"), sel); costF.append(r);
  number(costF, "set-kwh", "Electricity price", null, st.costs.electricity_usd_per_kwh, "0.01", "$ per kWh");
  number(costF, "set-watts", "Power while generating", "A Mac Studio typically draws 150 to 300 W under load.", st.costs.watts_while_generating, "10", "W");
  number(costF, "set-hw", "What your host cost", "Optional. Shows how long it takes to pay for itself.", st.costs.hardware_cost_usd, "100", "$");
  const pick = document.createElement("div"); pick.className = "models";
  for (const m of st.prices.models) {
    const l = document.createElement("label");
    const i = document.createElement("input"); i.type = "checkbox"; i.value = m.id; i.className = "set-compare"; i.checked = st.costs.compare.includes(m.id);
    const t = document.createElement("span"); t.textContent = m.provider + " " + m.name.replace(m.provider + " ", "");
    const s = document.createElement("small"); s.textContent = "$" + m.input + " in, $" + m.output + " out per 1M" + (m.per_request ? ", plus $" + m.per_request + " a request" : "");
    t.append(s); l.append(i, t); pick.append(l);
  }
  const ph = document.createElement("div"); ph.className = "label-sm"; ph.style.marginTop = "10px"; ph.textContent = "Compare against:";
  costF.append(ph, pick);
  const note = document.createElement("div"); note.className = "label-sm"; note.style.marginTop = "10px";
  note.textContent = "Changes are saved to " + st.config_file + ". To update prices, put your own pricing.json next to it.";
  costF.append(note);
  $("settings").showModal();
}
async function saveSettings() {
  const num = (id) => { const v = parseFloat($(id).value); return Number.isFinite(v) ? v : undefined; };
  const body = {
    portal: { intro_animation: $("set-intro").checked, pip_roams: $("set-roams").checked },
    app: { stampede_tokens_per_sec: num("set-stampede") },
    costs: {
      show_comparison: $("set-costs").checked, baseline: $("set-baseline").value,
      electricity_usd_per_kwh: num("set-kwh"), watts_while_generating: num("set-watts"), hardware_cost_usd: num("set-hw"),
      compare: [...document.querySelectorAll(".set-compare:checked")].map((i) => i.value),
    },
  };
  try {
    await getJSON("/api/settings", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });
    $("settings").close();
    toast("Settings saved");
    S.dataKey = "";
    await load();
    Stage.applySettings();
    if (body.costs.show_comparison) Stage.cheer("Ooh, let's see what we saved!");
  } catch (e) {
    toast("Couldn't save settings: " + e.message);
  }
}

// ================================================================== misc
let toastTimer;
function toast(msg) { const t = $("toast"); t.textContent = msg; t.style.display = "block"; clearTimeout(toastTimer); toastTimer = setTimeout(() => (t.style.display = "none"), 2800); }

// ================================================================== Pip's stage
const Stage = (() => {
  let pip = null, phase = "loading", state = "idle", liveTimer = 0, lastSparkle = 0;
  const groundY = () => innerHeight - 22;
  const trailSize = () => (innerWidth < 600 ? 84 : 108);
  const lightScale = (size) => Math.max(0.55, size / 110) * 0.85;
  const LINES = {
    idle: () => "Nothing to count. Nap time!",
    processing: (L) => "Nom nom nom… " + fmt(L.tokens_per_sec) + " tokens/sec!",
    stampede: () => "Holy tokens! We've got a stampede!",
  };

  function land(p, scale) {
    Pip.step(p.x, p.y, scale);
  }

  async function intro() {
    const overlay = $("intro");
    overlay.hidden = false;
    let skipped = false;
    const skip = () => { skipped = true; pip.cancel && pip.cancel(); };
    $("intro-skip").onclick = skip;
    const onKey = (e) => { if (e.key === "Escape" || e.key === " " || e.key === "Enter") skip(); };
    addEventListener("keydown", onKey);
    overlay.addEventListener("click", skip, { once: true });

    const big = Math.min(innerHeight * 0.36, 320, innerWidth * 0.62);
    const cx = innerWidth / 2, cy = innerHeight / 2 + big * 0.36;
    Pip.place(pip, cx, cy, big);
    pip.setState("processing");
    pip.setTail(Array.from({ length: 40 }, (_, i) => 30 + 25 * Math.sin(i / 3) + i * 1.5), 0);
    pip.el.animate([{ opacity: 0, scale: 0.6 }, { opacity: 1, scale: 1 }], { duration: 450, easing: "cubic-bezier(.2,.9,.3,1.4)" });
    typeTagline();
    await sleep(500);
    // Two happy taps in the middle, red then green.
    if (!skipped) await Pip.hopPath(pip, pip.pos, [{ x: cx, y: cy, height: 46, ms: 330 }, { x: cx, y: cy, height: 30, ms: 280, pause: 40 }], (p) => land(p, lightScale(big) * 1.25));
    // Then a run out from the center that shrinks onto the trail.
    const hops = [];
    const N = 6, Rx = innerWidth * 0.34, Ry = innerHeight * 0.3;
    for (let k = 1; k <= N; k++) {
      const a = -0.5 + k * 1.02;
      const r = k / N;
      hops.push({
        x: Math.max(80, Math.min(innerWidth - 80, cx + Math.cos(a) * Rx * r)),
        y: Math.max(big * 0.7, Math.min(innerHeight - 40, cy + Math.sin(a) * Ry * r)),
        size: big + (trailSize() - big) * (k / (N + 1)), ms: 380, height: 70,
      });
    }
    hops.push({ x: innerWidth * 0.5, y: groundY(), size: trailSize(), ms: 520, height: 110 });
    if (!skipped) {
      let leaving = false;
      await Pip.hopPath(pip, pip.pos, hops, (p, i) => {
        land(p, lightScale(p.size));
        if (i >= N - 2 && !leaving) { leaving = true; overlay.classList.add("leaving"); }
      });
    }
    removeEventListener("keydown", onKey);
    overlay.classList.add("leaving");
    if (skipped) Pip.place(pip, innerWidth * 0.5, groundY(), trailSize());
    setTimeout(() => (overlay.hidden = true), 950);
    pip.say("Hi, I'm Pip! I count every token.", 3200);
  }

  function typeTagline() {
    const el = $("intro-tag"), text = "Every token leaves a trail.";
    let i = 0; el.textContent = "";
    const caret = document.createElement("span"); caret.className = "caret";
    const t = document.createTextNode(""); el.append(t, caret);
    const step = () => { t.textContent = text.slice(0, ++i); if (i < text.length) setTimeout(step, 38); };
    setTimeout(step, 700);
  }

  async function trailLoop() {
    for (;;) {
      if (phase !== "trail" || document.hidden || !pip.pos) { await sleep(250); continue; }
      const st = state;
      if (st === "idle" || reduced()) { await sleep(400); continue; }
      const L = S.live || { tokens_per_sec: 0 };
      const fast = st === "stampede";
      const roam = S.meta && S.meta.portal ? S.meta.portal.pip_roams : true;
      const size = trailSize();
      const vigor = Math.min(1, L.tokens_per_sec / Math.max(30, (S.live && S.live.typical_tps) || 60));
      let x = pip.pos.x;
      if (roam) {
        const dist = fast ? 130 + Math.random() * 120 : 50 + Math.random() * 90 + vigor * 40;
        const lo = size * 0.6, hi = innerWidth - size * 0.6;
        let dir = Math.random() < 0.5 ? -1 : 1;
        if (x + dir * dist > hi) dir = -1;
        if (x + dir * dist < lo) dir = 1;
        x = Math.max(lo, Math.min(hi, x + dir * dist));
      }
      const ms = fast ? 260 : 460 - vigor * 140;
      await Pip.hopPath(pip, pip.pos, [{ x, y: groundY(), size, ms, height: roam ? (fast ? 60 : 34 + vigor * 22) : 22 }], (p) => land(p, lightScale(size) * (fast ? 1.15 : 1)));
      if (phase === "trail") await sleep(fast ? 20 : 120 + (1 - vigor) * 520);
    }
  }

  async function poll() {
    clearTimeout(liveTimer);
    try {
      const L = await getJSON("/api/live?seconds=60");
      S.live = L;
      const next = L.state;
      if (next !== state) {
        state = next;
        if (pip) { pip.setState(next); if (phase === "trail") pip.say(LINES[next](L), 3200); }
      }
      if (pip) pip.setTail(L.per_second, Math.max(L.typical_tps || 0, 5));
      $("ticker-text").textContent = LINES[state](L) + (L.active_requests > 1 ? " (" + L.active_requests + " requests at once)" : "");
      const v = $("stat-live"), d = $("stat-live-d");
      if (v) { v.textContent = compact(L.tokens_per_sec) + "/s"; d.textContent = L.active_requests ? L.active_requests + " request" + (L.active_requests === 1 ? "" : "s") + " in flight" : "nothing streaming"; }
    } catch (e) {
      $("ticker-text").textContent = "Pip can't reach Toktail right now.";
    }
    liveTimer = setTimeout(poll, document.hidden ? 5000 : 1000);
  }

  // Token squares drift off Pip's tail while it eats.
  function sparkles(now) {
    if (pip && pip.pos && phase !== "loading" && state !== "idle" && !reduced() && !document.hidden) {
      const tps = (S.live && S.live.tokens_per_sec) || 0;
      const every = state === "stampede" ? 40 : Math.max(70, 400 - tps * 2);
      if (now - lastSparkle > every) {
        lastSparkle = now;
        const s = pip.pos.size, dir = pip.el.classList.contains("facing-right") ? -1 : 1;
        Pip.sparkle(pip.pos.x + dir * s * 0.42, pip.pos.y - s * 0.42, state === "stampede" ? 2 : 1, s / 110);
      }
    }
    requestAnimationFrame(sparkles);
  }

  const QUIPS = [
    () => S.lastData ? "This range: " + compact(S.lastData.tot.output) + " tokens out. Yum." : "Every token leaves a trail.",
    () => "All time: " + fmt(S.meta ? S.meta.models.reduce((s, m) => s + m.output, 0) : 0) + " output tokens. I counted every one.",
    () => "Every token leaves a trail.",
    () => S.live && S.live.typical_tps ? "Your usual speed is " + S.live.typical_tps.toFixed(1) + " tok/s." : "Waiting for tokens…",
    () => "Try Replay the trail on the daily chart!",
  ];
  let quip = 0;
  async function onPipClick() {
    if (phase !== "trail" || !pip.pos) return;
    pip.say(QUIPS[quip++ % QUIPS.length](), 3600);
    if (reduced()) return;
    phase = "busy";
    const { x } = pip.pos, y = groundY(), size = trailSize();
    await Pip.hopPath(pip, pip.pos, [{ x, y, size, height: 70, ms: 420 }, { x, y, size, height: 40, ms: 320 }], (p) => land(p, lightScale(size) * 1.2));
    phase = "trail";
  }

  async function replay() {
    if (phase !== "trail" || !dailyGeom) return toast(dailyGeom ? "Pip is busy, try again in a moment." : "Nothing to replay in this range.");
    if (reduced()) return toast("Replay is off because your system asks for reduced motion.");
    phase = "replay";
    const card = $("card-daily");
    card.scrollIntoView({ behavior: "smooth", block: "center" });
    await sleep(650);
    const g = dailyGeom;
    const groups = [...g.svg.querySelectorAll("[data-day]")];
    const extras = [...g.svg.querySelectorAll(".ma,.peak")];
    groups.forEach((n) => { n.classList.remove("anim-bar", "replay-show"); n.classList.add("replay-hidden"); });
    extras.forEach((n) => n.classList.add("replay-hidden"));
    const hud = document.createElement("div"); hud.className = "replay-hud";
    card.append(hud);
    const n = g.days.length, every = Math.max(1, Math.ceil(n / 28));
    const size = 78;
    const toViewport = (i) => {
      const r = g.svg.getBoundingClientRect(), k = r.width / g.W;
      return { x: r.left + g.xc(i) * k, y: r.top + g.base * k };
    };
    let cum = 0, shown = -1;
    const idx = [];
    for (let i = 0; i < n; i += every) idx.push(i);
    if (idx[idx.length - 1] !== n - 1) idx.push(n - 1);
    pip.say("Here's how we got here!", 2000);
    for (const i of idx) {
      if (phase !== "replay") break;
      const t = toViewport(i);
      await Pip.hopPath(pip, pip.pos, [{ x: t.x, y: t.y, size, ms: shown < 0 ? 620 : 300, height: shown < 0 ? 160 : 34 }], (p) => {
        for (let k = shown + 1; k <= i; k++) {
          groups[k].classList.remove("replay-hidden"); groups[k].classList.add("replay-show");
          cum += g.totals[k];
        }
        shown = i;
        Pip.step(p.x, p.y, 0.75);
        hud.textContent = ""; hud.append(document.createTextNode(dayLabel(g.days[i])));
        const b = document.createElement("b"); b.textContent = fmt(cum); hud.append(b);
        hud.append(document.createTextNode(S.metric + " tokens so far"));
      });
    }
    extras.forEach((e) => { e.classList.remove("replay-hidden"); e.classList.add("anim-fade"); });
    pip.say("That's " + compact(cum) + " tokens. What a trail!", 3200);
    await sleep(900);
    await Pip.hopPath(pip, pip.pos, [{ x: innerWidth * 0.5, y: groundY(), size: trailSize(), ms: 700, height: 180 }], (p) => land(p, lightScale(trailSize())));
    hud.remove();
    groups.forEach((n) => n.classList.remove("replay-hidden"));
    phase = "trail";
  }

  async function start() {
    await Pip.load();
    pip = Pip.create({ size: trailSize() });
    pip.el.tabIndex = 0;
    pip.el.title = "Pip the Token Fox";
    document.body.append(pip.el);
    pip.el.addEventListener("click", onPipClick);
    pip.el.addEventListener("keydown", (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onPipClick(); } });
    poll();
    requestAnimationFrame(sparkles);
    const wantIntro = S.meta && S.meta.portal ? S.meta.portal.intro_animation : true;
    if (wantIntro && !reduced()) await intro();
    else Pip.place(pip, innerWidth * 0.5, groundY(), trailSize());
    pip.setState(state);
    phase = "trail";
    trailLoop();
    addEventListener("resize", () => {
      if (phase === "trail" && pip.pos) Pip.place(pip, Math.min(pip.pos.x, innerWidth - 60), groundY(), trailSize());
    });
  }

  return {
    start, replay,
    cheer(msg) { if (pip && phase === "trail") pip.say(msg, 2600); },
    applySettings() {},
  };
})();

// ================================================================== wire up
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
  S.metric = b.dataset.m; store.set("metric", S.metric);
  S.dataKey = ""; load();
});
$("tabs").addEventListener("click", (e) => { const b = e.target.closest("button"); if (!b) return; S.tab = b.dataset.t; renderTable(S.lastData); });
document.addEventListener("click", (e) => { const b = e.target.closest("[data-export]"); if (b) exportSvg(b.dataset.export); });
$("btn-table-csv").addEventListener("click", exportTableCsv);
$("btn-raw-csv").addEventListener("click", () => { location.href = "/api/export.csv?" + qs(); });
$("btn-replay").addEventListener("click", () => Stage.replay());
$("btn-settings").addEventListener("click", openSettings);
$("settings-cancel").addEventListener("click", () => $("settings").close());
$("settings-form").addEventListener("submit", (e) => { e.preventDefault(); saveSettings(); });
$("baseline").addEventListener("change", async (e) => {
  try {
    await getJSON("/api/settings", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ costs: { baseline: e.target.value } }) });
    S.dataKey = ""; load();
  } catch (err) { toast("Couldn't change the comparison: " + err.message); }
});
$("btn-sync").addEventListener("click", async () => {
  const r = await getJSON("/api/sync", { method: "POST" }).catch(() => ({}));
  toast(r.triggered ? "Syncing to S3" : "S3 is off in config.toml");
  setTimeout(load, 3000);
});
$("btn-theme").addEventListener("click", () => {
  const next = isDark() ? "light" : "dark";
  document.documentElement.dataset.theme = next;
  store.set("theme", next);
  if (S.meta) { S.animate = false; render(); }
});
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => { if (S.meta) { S.animate = false; render(); } });
let rz; addEventListener("resize", () => { clearTimeout(rz); rz = setTimeout(() => { if (S.meta) { S.animate = false; render(); } }, 160); });
setInterval(() => { if (!document.hidden) load(); }, 30000);
document.addEventListener("visibilitychange", () => { if (!document.hidden) load(); });

load().then(() => Stage.start());
