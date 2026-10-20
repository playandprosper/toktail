// SPDX-License-Identifier: Apache-2.0
// Toktail portal, shared pieces: state, formatting, fetching, SVG chart
// helpers, tooltips, tables, export, dialogs and form fields.
"use strict";

// ================================================================== state
const S = {
  meta: null, daily: [], heat: [], recent: [], repeats: { repeated_requests: 0, top: [] }, costs: null,
  settings: null, live: null, page: "dashboard",
  range: "30", metric: "combined", split: "model", hidden: new Set(), hosts: null, tab: "daily",
  colorOf: new Map(), hostColorOf: new Map(), lastData: null, animate: true, dataKey: "",
};
const SLOTS = 8;
const $ = (id) => document.getElementById(id);
const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const store = {
  get(k) { try { return localStorage.getItem("toktail." + k); } catch (e) { return null; } },
  set(k, v) { try { localStorage.setItem("toktail." + k, v); } catch (e) {} },
};
S.range = store.get("range") || S.range;
S.metric = ["combined", "output", "input"].includes(store.get("metric2")) ? store.get("metric2") : "combined";
S.split = store.get("split") === "host" ? "host" : "model";
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const reduced = () => Pip.reduceMotion.matches;

// ================================================================== format
const nf = new Intl.NumberFormat();
const fmt = (n) => nf.format(Math.round(n));
function compact(n) {
  const a = Math.abs(n);
  const f = (v, u) => (v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)).replace(/\.0+$|(\.\d*?)0+$/, "$1") + u;
  if (a >= 1e12) return f(n / 1e12, "T");
  if (a >= 1e9) return f(n / 1e9, "B");
  if (a >= 1e6) return f(n / 1e6, "M");
  if (a >= 1e3) return f(n / 1e3, "K");
  return String(Math.round(n));
}
/** "17.0 trillion": the words under a big number. */
function words(n) {
  const a = Math.abs(n);
  const f = (v, u) => (v >= 100 ? v.toFixed(0) : v.toFixed(1)).replace(/\.0$/, "") + " " + u;
  if (a >= 1e12) return f(n / 1e12, "trillion");
  if (a >= 1e9) return f(n / 1e9, "billion");
  if (a >= 1e6) return f(n / 1e6, "million");
  if (a >= 1e3) return f(n / 1e3, "thousand");
  return "";
}
const tickFmt = (n) => (n === 0 ? "0" : compact(n));
const usdFmt = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const usdFmt0 = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 0 });
function usd(v) {
  if (v === 0) return "$0";
  if (Math.abs(v) < 0.01) return v < 0 ? "−<$0.01" : "<$0.01";
  return Math.abs(v) >= 1000 ? usdFmt0.format(v) : usdFmt.format(v);
}
const usd0 = (v) => usdFmt0.format(v);
const usdTick = (v) => (v === 0 ? "$0" : Math.abs(v) < 1 ? "$" + v.toFixed(2).replace(/0$/, "") : (v < 0 ? "−$" : "$") + compact(Math.abs(v)));
const MONTHS = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
const DAYS = ["Mon","Tue","Wed","Thu","Fri","Sat","Sun"];
const DAYS_LONG = ["Monday","Tuesday","Wednesday","Thursday","Friday","Saturday","Sunday"];
function parseD(s) { const [y, m, d] = s.split("-").map(Number); return new Date(Date.UTC(y, m - 1, d)); }
function isoD(d) { return d.toISOString().slice(0, 10); }
function addDays(s, n) { const d = parseD(s); d.setUTCDate(d.getUTCDate() + n); return isoD(d); }
function dayLabel(s) { const d = parseD(s); return MONTHS[d.getUTCMonth()] + " " + d.getUTCDate(); }
function longDay(s) { const d = parseD(s); return DAYS[(d.getUTCDay() + 6) % 7] + ", " + MONTHS[d.getUTCMonth()] + " " + d.getUTCDate() + ", " + d.getUTCFullYear(); }
function monthLabel(s) { const [y, m] = s.split("-"); return MONTHS[+m - 1] + " " + y; }
function pct(x) { return (x >= 0 ? "+" : "−") + Math.abs(x * 100).toFixed(Math.abs(x) < 0.1 ? 1 : 0) + "%"; }
function ago(iso) {
  const s = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return Math.floor(s / 60) + " min ago";
  if (s < 86400) return Math.floor(s / 3600) + " h ago";
  return Math.floor(s / 86400) + " d ago";
}
function when(iso) {
  const d = new Date(iso);
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: d.getFullYear() === new Date().getFullYear() ? undefined : "numeric" }) + ", " + d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", second: "2-digit" });
}
const METRIC_NAME = { combined: "combined", output: "output", input: "input" };
const METRIC_TITLE = { combined: "Combined", output: "Output", input: "Input" };
/** A row's (or totals') value for the chosen metric. */
const mv = (e, key) => (key === "combined" ? (e.input || 0) + (e.output || 0) : e[key] || 0);

// ================================================================== fetch
async function getJSON(url, opts) {
  const r = await fetch(url, { cache: "no-store", ...(opts || {}) });
  if (!r.ok) {
    const text = (await r.text().catch(() => "")).trim();
    throw new Error(text && text.length < 400 ? text : url + " answered " + r.status);
  }
  return r.json();
}
const postJSON = (url, body, method = "POST") => getJSON(url, { method, headers: { "Content-Type": "application/json" }, body: JSON.stringify(body) });

// ================================================================== DOM helpers
function h(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  if (attrs) for (const k in attrs) {
    const v = attrs[k];
    if (v === undefined || v === null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k === "text") e.textContent = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (k === "style") e.style.cssText = v;
    else if (v === true) e.setAttribute(k, "");
    else e.setAttribute(k, v);
  }
  for (const c of kids.flat()) if (c !== null && c !== undefined && c !== false) e.append(typeof c === "string" || typeof c === "number" ? document.createTextNode(String(c)) : c);
  return e;
}
function span(text, cls) { const s = document.createElement("span"); if (cls) s.className = cls; s.textContent = text; return s; }
function bold(text) { const b = document.createElement("b"); b.textContent = text; return b; }
function isDark() {
  const t = document.documentElement.dataset.theme;
  return t ? t === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
}
let toastTimer;
function toast(msg) { const t = $("toast"); t.textContent = msg; t.style.display = "block"; clearTimeout(toastTimer); toastTimer = setTimeout(() => (t.style.display = "none"), 3200); }

// Counts a number up to its value. One orchestrated moment, not decoration.
function countUp(el, to, ms = 1100, onFrame) {
  const from = Number(el.dataset.v || 0);
  el.dataset.v = to;
  const put = (v) => { el.textContent = fmt(v); if (onFrame) onFrame(); };
  if (!S.animate || from === to) { put(to); return; }
  const t0 = performance.now();
  const tick = (now) => {
    const t = Math.min(1, (now - t0) / ms);
    const e = 1 - Math.pow(1 - t, 3);
    put(from + (to - from) * e);
    if (t < 1) requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
}

/** Shrinks an element's font until its text fits its container (big numbers). */
function fitText(el, maxPx, minPx) {
  const box = el.parentElement;
  el.style.fontSize = maxPx + "px";
  const avail = box.clientWidth;
  if (!avail) return;
  let size = maxPx;
  for (let i = 0; i < 12 && el.scrollWidth > avail && size > minPx; i++) {
    size = Math.max(minPx, size * (avail / el.scrollWidth) * 0.98);
    el.style.fontSize = size.toFixed(1) + "px";
  }
}

// ================================================================== colors
// Colors follow the model (ranked by all-time tokens) or the host (in
// Settings order), never the filter.
function assignColors() {
  S.colorOf = new Map();
  S.meta.models.forEach((m, i) => S.colorOf.set(m.model, i < SLOTS ? "--s" + (i + 1) : "--other"));
  S.hostColorOf = new Map();
  S.meta.hosts.forEach((hh, i) => S.hostColorOf.set(hh.id, i < SLOTS ? "--s" + (((i + 1) % SLOTS) + 1) : "--other"));
}
const colorVar = (model) => S.colorOf.get(model) || "--other";
const color = (model) => css(colorVar(model));
const hostColorVar = (id) => S.hostColorOf.get(id) || "--other";
const hostColor = (id) => css(hostColorVar(id));
function hostName(id) {
  const x = S.meta && S.meta.hosts.find((hh) => hh.id === id);
  return x ? x.name : id;
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
function newSvg(container, w, hgt, title) {
  container.textContent = "";
  const svg = el("svg", {
    viewBox: `0 0 ${w} ${hgt}`, width: w, height: hgt, role: "img", "aria-label": title,
    "font-family": 'system-ui, -apple-system, "Segoe UI", sans-serif',
  }, container);
  svg.dataset.title = title;
  return svg;
}
function yAxis(svg, x0, x1, y, max, step, fmtFn, min = 0) {
  const g = el("g", {}, svg);
  for (let v = min; v <= max + 1e-9; v += step) {
    const yy = y(v);
    el("line", { x1: x0, x2: x1, y1: yy, y2: yy, stroke: Math.abs(v) < 1e-9 ? css("--axis") : css("--grid"), "stroke-width": 1, "shape-rendering": "crispEdges" }, g);
    txt(g, x0 - 8, yy + 4, (fmtFn || tickFmt)(v), { "text-anchor": "end" });
  }
}
function xDateTicks(svg, days, xc, y, maxTicks, labelFn) {
  const n = days.length, every = Math.max(1, Math.ceil(n / Math.max(1, maxTicks)));
  for (let i = 0; i < n; i++) if ((n - 1 - i) % every === 0) txt(svg, xc(i), y, (labelFn || dayLabel)(days[i]), { "text-anchor": "middle" });
}
function topRounded(x, y, w, hh, r) {
  r = Math.min(r, w / 2, hh);
  return `M${x},${y + hh}V${y + r}Q${x},${y} ${x + r},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + hh}Z`;
}
function rightRounded(x, y, w, hh, r) {
  r = Math.min(r, hh / 2, w);
  return `M${x},${y}H${x + w - r}Q${x + w},${y} ${x + w},${y + r}V${y + hh - r}Q${x + w},${y + hh} ${x + w - r},${y + hh}H${x}Z`;
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
/** A tiny line for host tiles and machine cards. */
function sparkline(values, w, hh, stroke, fill) {
  const svg = el("svg", { viewBox: `0 0 ${w} ${hh}`, width: w, height: hh, class: "spark", "aria-hidden": "true" });
  const max = Math.max(1e-9, ...values);
  const n = values.length;
  if (n < 2) return svg;
  const x = (i) => (i / (n - 1)) * w, y = (v) => hh - 2 - (v / max) * (hh - 4);
  const d = values.map((v, i) => (i ? "L" : "M") + x(i).toFixed(1) + "," + y(v).toFixed(1)).join("");
  if (fill) el("path", { d: d + `L${w},${hh}L0,${hh}Z`, fill, opacity: 0.18 }, svg);
  el("path", { d, fill: "none", stroke, "stroke-width": 1.6, "stroke-linejoin": "round", "stroke-linecap": "round" }, svg);
  return svg;
}

// ================================================================== tooltip
const tip = $("tip");
function showTip(evt, head, rows, total) {
  tip.textContent = "";
  const hd = document.createElement("div"); hd.className = "h"; hd.textContent = head; tip.append(hd);
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

/** Horizontal bars with a label above each bar and the value at its tip. */
function hbars(box, title, rows) {
  const W = Math.max(300, box.clientWidth), rowH = 46, m = { l: 4, r: 96, t: 4, b: 4 };
  const H = m.t + m.b + rows.length * rowH;
  const svg = newSvg(box, W, H, title);
  const iw = W - m.l - m.r;
  const max = Math.max(...rows.map((r) => r.v), 1e-9);
  rows.forEach((r, i) => {
    const y0 = m.t + i * rowH;
    txt(svg, m.l, y0 + 15, r.label, { fill: css("--ink"), "font-size": 13, "font-weight": r.strong ? 700 : 400 });
    if (r.right) txt(svg, W - 4, y0 + 15, r.right, { "text-anchor": "end", fill: css("--ink-2"), "font-size": 12 });
    el("rect", { x: m.l, y: y0 + 23, width: iw, height: 12, fill: css("--surface-2"), rx: 4 }, svg);
    if (r.parts) {
      // Stacked: several pieces side by side (hardware + electricity).
      let x0 = m.l;
      r.parts.forEach((p, k) => {
        const bw = (p.v / max) * iw;
        if (bw <= 0) return;
        const last = k === r.parts.length - 1 || r.parts.slice(k + 1).every((q) => q.v <= 0);
        const bar = el("path", { d: last ? rightRounded(x0, y0 + 23, bw, 12, 4) : `M${x0},${y0 + 23}h${bw}v12h${-bw}Z`, fill: p.color }, svg);
        if (S.animate) { bar.classList.add("anim-hbar"); bar.style.animationDelay = i * 70 + "ms"; }
        x0 += bw;
      });
    } else {
      const bw = Math.max(r.v > 0 ? 3 : 0, (r.v / max) * iw);
      const bar = el("path", { d: rightRounded(m.l, y0 + 23, bw, 12, 4), fill: r.color }, svg);
      if (S.animate) { bar.classList.add("anim-hbar"); bar.style.animationDelay = i * 70 + "ms"; }
    }
    txt(svg, W - 4, y0 + 34, r.value, { "text-anchor": "end", fill: css("--ink"), "font-weight": 700, "font-size": 12.5 });
    const hit = el("rect", { x: 0, y: y0, width: W, height: rowH, fill: "transparent", "data-ui": "1" }, svg);
    if (r.tip) {
      hit.addEventListener("pointermove", (e) => { const [hd, rr] = r.tip(); showTip(e, hd, rr); });
      hit.addEventListener("pointerleave", hideTip);
    }
  });
  return svg;
}

// ================================================================== line chart
function lineChart(box, days, series, opts) {
  const W = Math.max(300, box.clientWidth), H = opts.height || 250;
  const m = { l: 56, r: opts.endLabels ? 78 : 14, t: 12, b: 30 };
  const svg = newSvg(box, W, H, opts.title);
  const n = days.length;
  const vals = series.flatMap((s) => s.values.filter((v) => v != null));
  const { max, step } = niceMax(Math.max(...vals, 1e-9), 4);
  const iw = W - m.l - m.r, ih = H - m.t - m.b;
  const x = (i) => m.l + (n === 1 ? iw / 2 : (i / (n - 1)) * iw);
  const y = (v) => m.t + ih - (v / max) * ih;
  yAxis(svg, m.l, W - m.r, y, max, step, opts.tickFmt);
  xDateTicks(svg, days, x, H - 9, Math.floor(iw / 72), opts.xLabel);
  const surface = css("--surface");
  if (opts.between) {
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
    const p = el("path", { d, fill: "none", stroke: s.color, "stroke-width": s.width || 2, "stroke-dasharray": s.dash || null, "stroke-linejoin": "round", "stroke-linecap": "round" }, svg);
    if (!s.dash) drawIn(p, si * 120);
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
    showTip(e, (opts.tipHead || longDay)(days[i]), rows, opts.tipTotal ? opts.tipTotal(i) : null);
  });
  ov.addEventListener("pointerleave", () => { cross.setAttribute("opacity", 0); dots.forEach((d) => d.setAttribute("opacity", 0)); hideTip(); });
  return svg;
}

// ================================================================== tables
function table(cols, rows) {
  const t = document.createElement("table");
  const thead = t.createTHead().insertRow();
  for (const c of cols) { const th = document.createElement("th"); th.textContent = c.h; if (c.l) th.className = "l"; if (c.title) th.title = c.title; thead.append(th); }
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
function fpCell(hx) {
  if (!hx) return document.createTextNode("—");
  const c = document.createElement("code"); c.className = "fp"; c.textContent = hx.slice(0, 12) + "…"; c.title = hx + " (click to copy)";
  c.onclick = () => { navigator.clipboard && navigator.clipboard.writeText(hx).then(() => toast("Fingerprint copied")); };
  return c;
}
function swatchText(label, varName) {
  const s = document.createElement("span"); s.className = "swt";
  const i = document.createElement("i"); i.style.background = "var(" + varName + ")";
  s.append(i, document.createTextNode(label)); return s;
}

// ================================================================== export
function download(name, blob) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob); a.download = name;
  document.body.append(a); a.click(); a.remove();
  setTimeout(() => URL.revokeObjectURL(a.href), 2000);
}
/** Exports a chart with its title, legend and a footer, as a standalone SVG. */
function exportChart(chartSel, legendSel, title, subtitle, fileName) {
  const svg = document.querySelector(chartSel + " svg");
  if (!svg) return toast("Nothing to export here.");
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
  txt(out, pad, pad + 16, title || svg.dataset.title, { "font-size": 17, "font-weight": 800, fill: css("--ink"), "font-family": 'ui-rounded, "SF Pro Rounded", system-ui, sans-serif' });
  if (subtitle) txt(out, pad, pad + 36, subtitle, { "font-size": 12, fill: css("--ink-2") });
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
  download(fileName, new Blob([xml], { type: "image/svg+xml" }));
  if ((typeof Stage !== "undefined")) Stage.cheer("Exported! That chart is all yours.");
}
function csvEscape(v) { const s = String(v ?? ""); return /[",\n]/.test(s) ? '"' + s.replace(/"/g, '""') + '"' : s; }

// ================================================================== dialog
/** Shows a modal with a body and buttons; resolves with the clicked button's value. */
function modal(title, body, buttons) {
  const d = $("modal");
  $("modal-title").textContent = title;
  const b = $("modal-body"); b.textContent = "";
  if (typeof body === "string") b.append(h("p", { text: body })); else b.append(body);
  const f = $("modal-foot"); f.textContent = "";
  return new Promise((resolve) => {
    let done = false;
    const finish = (v) => { if (done) return; done = true; d.close(); resolve(v); };
    for (const btn of buttons) {
      const e = h("button", { type: "button", class: btn.cls || "", text: btn.label });
      e.addEventListener("click", async () => {
        if (btn.validate) { const ok = await btn.validate(); if (!ok) return; }
        finish(btn.value);
      });
      f.append(e);
    }
    d.addEventListener("cancel", () => finish(null), { once: true });
    d.showModal();
  });
}
const confirmBox = (title, text, yes, danger) => modal(title, text, [{ label: "Cancel", value: false }, { label: yes, value: true, cls: danger ? "primary danger" : "primary" }]);

// ================================================================== form fields
/** A labelled row. `control` is an element; hint and label are text. */
function field(label, hint, control, opts = {}) {
  const r = h("label", { class: "row" + (opts.stack ? " stack" : "") });
  const t = h("span", { class: "row-label" }, label, hint ? h("div", { class: "hint", text: hint }) : null);
  r.append(t, control);
  return r;
}
function toggleField(label, hint, checked) {
  const i = h("input", { type: "checkbox", class: "switch" });
  i.checked = !!checked;
  return { row: field(label, hint, i), input: i };
}
function textField(label, hint, value, placeholder, type = "text") {
  const i = h("input", { type, class: "wide", placeholder: placeholder || "", autocomplete: "off", spellcheck: "false" });
  i.value = value ?? "";
  return { row: field(label, hint, i, { stack: true }), input: i };
}
function numberField(label, hint, value, step, unit, min = 0, max) {
  const i = h("input", { type: "number", step, min, max });
  i.value = value ?? "";
  const w = h("span", { class: "num-wrap" }, i, unit ? span(unit, "label-sm") : null);
  return { row: field(label, hint, w), input: i };
}
function selectField(label, hint, options, value) {
  const s = h("select");
  for (const [v, t] of options) { const o = h("option", { value: v, text: t }); if (String(v) === String(value)) o.selected = true; s.append(o); }
  return { row: field(label, hint, s), input: s };
}
/** A secret field: never shows the saved value, only its last characters. */
function secretField(label, hint, saved, placeholder) {
  const i = h("input", { type: "password", class: "wide", autocomplete: "new-password", spellcheck: "false", placeholder: saved && saved.saved ? "Saved: " + saved.hint + " (type to replace)" : placeholder || "Not set" });
  return { row: field(label, hint, i, { stack: true }), input: i };
}
const num = (input) => { const v = parseFloat(input.value); return Number.isFinite(v) ? v : undefined; };


