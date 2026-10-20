// SPDX-License-Identifier: Apache-2.0
// Toktail portal: the Savings page. Your machines, their lifecycles, and what
// they save against public API prices month by month and year by year.
"use strict";

const Savings = (() => {
  const V = { data: null, settings: null, period: store.get("sav.period") === "yearly" ? "yearly" : "monthly" };

  async function show() {
    try {
      const [data, settings] = await Promise.all([getJSON("/api/savings"), getJSON("/api/settings")]);
      V.data = data; V.settings = settings;
      render();
    } catch (e) { toast("Couldn't load savings: " + e.message); }
  }

  function render() {
    const D = V.data;
    const sel = $("sav-baseline"); sel.textContent = "";
    for (const m of V.settings.prices.models) {
      const o = h("option", { value: m.id, text: "Compare with " + m.provider + " " + m.name.replace(m.provider + " ", "") });
      if (m.id === D.baseline) o.selected = true;
      sel.append(o);
    }
    renderHero(D);
    renderMachines(D);
    renderPeriods(D);
    if ((typeof Stage !== "undefined")) Stage.platformsChanged();
  }

  // ---------------------------------------------------------------- hero
  function renderHero(D) {
    const box = $("sav-hero"); box.textContent = "";
    const t = D.total;
    const saved = t.savings;
    const big = h("div", { class: "sav-big" },
      h("div", { class: "sav-lede", text: saved >= 0 ? "Your hardware has saved you" : "So far your hardware has cost" }),
      h("div", { class: "sav-num " + (saved >= 0 ? "pos" : "neg"), text: usd0(Math.abs(saved)) }),
      h("div", { class: "sav-sub" }, "against " + D.baseline_name + " list prices, " + (t.requests ? "over " + fmt(t.requests) + " requests and " + compact(t.input_tokens + t.output_tokens) + " combined tokens." : "once there's traffic to price.")));
    const items = [
      ["Cloud price of your tokens", usd0(t.baseline_cloud), "at " + D.baseline_name],
      ["Hardware share", usd0(t.hardware), "price spread over each lifecycle"],
      ["Electricity", usd0(t.electricity), "at " + usd(D.usd_per_kwh) + " per kWh"],
      ["Pace, last 30 days", (D.pace_monthly >= 0 ? "" : "−") + usd0(Math.abs(D.pace_monthly)) + "/mo", (D.pace_yearly >= 0 ? usd0(D.pace_yearly) + " a year" : "costing " + usd0(-D.pace_yearly) + " a year") + " at this pace"],
    ];
    const grid = h("div", { class: "sav-stats" });
    for (const [k, v, d] of items) grid.append(h("div", { class: "stat" }, h("div", { class: "k", text: k }), h("div", { class: "v", text: v }), h("div", { class: "d", text: d })));
    box.append(big, grid);
    if (D.unassigned_hosts.length) box.append(h("p", { class: "banner" }, "Traffic from " + D.unassigned_hosts.map(hostName).join(", ") + " has no machine assigned, so it counts as savings with no hardware cost. ", h("a", { href: "#", text: "Add the machine", onclick: (e) => { e.preventDefault(); editMachine(null, { host: D.unassigned_hosts[0] }); } }), "."));
    if (!D.machines.length) box.append(h("p", { class: "banner" }, "Add the machines behind your hosts (what you paid, when, and how long you'll keep them) to see your real savings after hardware.", " ", h("button", { type: "button", class: "small primary", text: "Add hardware", onclick: () => editMachine(null) })));
  }

  // ---------------------------------------------------------------- machines
  function lifeColor(p) { return p < 0.5 ? "var(--mint)" : p < 0.85 ? "var(--lav)" : p < 1 ? "var(--orange)" : "var(--bad)"; }
  function renderMachines(D) {
    const box = $("machines"); box.textContent = "";
    const active = D.machines.filter((m) => m.active), retired = D.machines.filter((m) => !m.active);
    for (const m of active) box.append(machineCard(m, D));
    if (retired.length) {
      box.append(h("h2", { class: "sec-title", text: "Retired" }));
      for (const m of retired) box.append(machineCard(m, D));
    }
  }
  function machineCard(m, D) {
    const used = Math.max(0, m.lifecycle_used);
    const pctTxt = Math.min(999, Math.round(used * 100)) + "%";
    const ageMonths = m.age_days / 30.4375;
    const card = h("section", { class: "card machine" + (m.active ? "" : " retired") });
    const replacedBy = m.replaced_by && D.machines.find((x) => x.id === m.replaced_by);
    card.append(h("div", { class: "card-head" },
      h("div", {}, h("h2", { text: m.name }), h("div", { class: "sub" }, m.model || "", m.model ? " · " : "", swatchText(hostName(m.host), hostColorVar(m.host)))),
      h("div", { class: "actions" },
        m.active ? h("button", { type: "button", class: "small", text: "Replace…", title: "Retire this machine and start counting a new one on the same host", onclick: () => replaceMachine(m) }) : null,
        h("button", { type: "button", class: "small", text: "Edit", onclick: () => editMachine(m) }),
        h("button", { type: "button", class: "small danger", text: "Delete", onclick: () => deleteMachine(m) }))));
    // Lifecycle: a bar that glows at how far along it is.
    const life = h("div", { class: "life" },
      h("div", { class: "life-head" },
        h("span", {}, "Lifecycle ", h("b", { text: pctTxt })),
        span(m.active ? (m.months_left > 0 ? Math.round(m.months_left) + " months left, ends " + monthLabel(m.lifecycle_end.slice(0, 7)) : "Past its " + m.lifecycle_months + "-month lifecycle: paid off, all savings now") : "Retired " + longDay(m.retired) + (replacedBy ? ", replaced by " + replacedBy.name : ""), "label-sm")),
      h("div", { class: "life-track", role: "progressbar", "aria-valuemin": "0", "aria-valuemax": "100", "aria-valuenow": String(Math.round(Math.min(1, used) * 100)), "aria-label": "Lifecycle used" },
        h("i", { class: "life-fill" + (m.active ? " live" : ""), style: `width:${Math.min(100, used * 100).toFixed(1)}%;--glow:${lifeColor(used)}` }),
        ...[0.25, 0.5, 0.75].map((q) => h("span", { class: "life-tick", style: `left:${q * 100}%` }))),
      h("div", { class: "life-foot label-sm" }, "Bought " + longDay(m.purchased) + " · " + ageMonths.toFixed(1) + " of " + m.lifecycle_months + " months · " + usd0(m.monthly_cost) + " a month while in lifecycle"));
    card.append(life);
    const tpsBest = m.tps_30d ?? m.tps_7d ?? m.tps_lifetime;
    const grid = h("div", { class: "machine-grid" },
      h("div", { class: "mstat" }, h("div", { class: "k", text: "Paid" }), h("div", { class: "v", text: usd0(m.price_usd) }), h("div", { class: "d" }, "serial ", m.serial ? h("code", { class: "fp", text: m.serial, title: "Click to copy", onclick: () => navigator.clipboard && navigator.clipboard.writeText(m.serial).then(() => toast("Serial number copied")) }) : "—")),
      h("div", { class: "mstat" }, h("div", { class: "k", text: "Lifetime tokens" }), h("div", { class: "v", text: compact(m.input_tokens + m.output_tokens) }), h("div", { class: "d", text: compact(m.input_tokens) + " in · " + compact(m.output_tokens) + " out · " + fmt(m.requests) + " requests" })),
      h("div", { class: "mstat" }, h("div", { class: "k", text: "Rolling speed" }), h("div", { class: "v", text: tpsBest != null ? tpsBest.toFixed(1) + " tok/s" : "—" }),
        h("div", { class: "d", text: "7 days " + (m.tps_7d != null ? m.tps_7d.toFixed(1) : "—") + " · 30 days " + (m.tps_30d != null ? m.tps_30d.toFixed(1) : "—") + " · lifetime " + (m.tps_lifetime != null ? m.tps_lifetime.toFixed(1) : "—") }),
        m.tps_weekly.length > 1 ? sparkline(m.tps_weekly.map((w) => w[1]), 180, 28, hostColor(m.host), hostColor(m.host)) : null),
      h("div", { class: "mstat" }, h("div", { class: "k", text: "Cloud value" }), h("div", { class: "v", text: usd0(m.cloud_value) }), h("div", { class: "d", text: "its tokens at " + D.baseline_name })),
      h("div", { class: "mstat" }, h("div", { class: "k", text: "Net savings" }), h("div", { class: "v " + (m.net_savings >= 0 ? "pos" : "neg"), text: (m.net_savings < 0 ? "−" : "") + usd0(Math.abs(m.net_savings)) }), h("div", { class: "d", text: "after " + usd0(m.amortized_so_far) + " hardware share and " + usd0(m.electricity) + " electricity" })),
      h("div", { class: "mstat" }, h("div", { class: "k", text: "Paid back" }), h("div", { class: "v", text: Math.round(m.payback * 100) + "%" }),
        h("div", { class: "payback" }, h("i", { style: `width:${Math.min(100, m.payback * 100).toFixed(1)}%` })),
        h("div", { class: "d", text: m.payback >= 1 ? "It has paid for itself." : "of its price and power, in cloud value" })));
    card.append(grid);
    if (m.notes) card.append(h("p", { class: "label-sm", text: m.notes }));
    return card;
  }

  async function editMachine(m, preset) {
    const isNew = !m;
    const hosts = (S.meta ? S.meta.hosts : []).map((x) => [x.id, x.name]);
    const base = m || { name: "Mac Studio", model: "", serial: "", price_usd: 0, purchased: S.meta ? S.meta.today : isoD(new Date()), lifecycle_months: V.data.default_lifecycle_months, host: (preset && preset.host) || (hosts[0] || [""])[0], watts: 180, idle_watts: 0, notes: "", ...(preset || {}) };
    const form = machineForm(base, hosts, !isNew && !!m.retired);
    const save = async () => {
      const body = form.read();
      if (!isNew) { body.id = m.id; body.retired = form.retired ? form.retired() : m.retired; body.replaced_by = m.replaced_by; }
      try { await postJSON("/api/hardware", body); return true; } catch (e) { form.error(e.message); return false; }
    };
    const ok = await modal(isNew ? "Add hardware" : "Edit " + m.name, form.el, [{ label: "Cancel", value: false }, { label: isNew ? "Add" : "Save", value: true, cls: "primary", validate: save }]);
    if (ok) { toast(isNew ? "Added. Pip is counting its savings." : "Saved"); show(); }
  }
  async function replaceMachine(m) {
    const hosts = [[m.host, hostName(m.host)]];
    const form = machineForm({ name: m.name.replace(/\s*\(\d{4}\)$/, "") + " (" + (S.meta ? S.meta.today.slice(0, 4) : new Date().getFullYear()) + ")", model: "", serial: "", price_usd: m.price_usd, purchased: S.meta ? S.meta.today : isoD(new Date()), lifecycle_months: m.lifecycle_months, host: m.host, watts: m.watts, idle_watts: m.idle_watts, notes: "" }, hosts, false, true);
    form.el.prepend(h("p", { class: "label-sm", text: m.name + " retires on the new machine's purchase date. Its lifetime tokens, speed and savings stay with it; from that day on, " + hostName(m.host) + "'s traffic counts toward the new machine." }));
    const save = async () => {
      try { await postJSON("/api/hardware/" + encodeURIComponent(m.id) + "/replace", form.read()); return true; } catch (e) { form.error(e.message); return false; }
    };
    const ok = await modal("Replace " + m.name, form.el, [{ label: "Cancel", value: false }, { label: "Replace", value: true, cls: "primary", validate: save }]);
    if (ok) { toast("Replaced. Fresh hardware, fresh trail!"); if ((typeof Stage !== "undefined")) Stage.cheer("New hardware! Let's go!"); show(); }
  }
  async function deleteMachine(m) {
    const ok = await confirmBox("Delete " + m.name + "?", "This forgets the machine (its price, serial number and lifecycle). Your token history isn't touched. To keep its numbers but stop counting it, use Replace instead.", "Delete", true);
    if (!ok) return;
    try { await getJSON("/api/hardware/" + encodeURIComponent(m.id), { method: "DELETE" }); toast("Deleted"); show(); } catch (e) { toast(e.message); }
  }
  function machineForm(m, hosts, showRetired, replacing) {
    const el2 = h("div", { class: "form" });
    const f = {
      name: textField("Name", "What you call it", m.name, "Mac Studio"),
      model: textField("Model", "Optional, e.g. Mac Studio (M3 Ultra, 512 GB)", m.model, ""),
      serial: textField("Serial number", "Optional. Stays on this Mac.", m.serial, ""),
      price: numberField("Price paid", null, m.price_usd, "1", "$"),
      purchased: (() => { const i = h("input", { type: "date", value: m.purchased }); return { row: field(replacing ? "In service from" : "Bought on", null, i), input: i }; })(),
      life: numberField("Lifecycle", "How long you expect to keep it. Its price is spread over this.", m.lifecycle_months, "1", "months", 1, 600),
      host: selectField("Serves host", null, hosts, m.host),
      watts: numberField("Power while generating", "A Mac Studio typically draws 150 to 300 W under load.", m.watts, "5", "W"),
      idle: numberField("Power the rest of the time", "Optional. Counted around the clock.", m.idle_watts, "1", "W"),
      notes: textField("Notes", null, m.notes, ""),
    };
    if (replacing) f.host.input.disabled = true;
    for (const k in f) el2.append(f[k].row);
    let retired = null;
    if (showRetired) {
      const i = h("input", { type: "date", value: m.retired || "" });
      el2.append(field("Retired on", "Clear to put it back in service.", i));
      retired = () => i.value || null;
    }
    const err = h("div", { class: "form-err", role: "alert" });
    el2.append(err);
    return {
      el: el2,
      retired,
      error: (msg) => { err.textContent = msg; },
      read: () => ({
        name: f.name.input.value.trim(), model: f.model.input.value.trim(), serial: f.serial.input.value.trim(),
        price_usd: num(f.price.input) ?? 0, purchased: f.purchased.input.value, lifecycle_months: Math.round(num(f.life.input) ?? 72),
        host: f.host.input.value, watts: num(f.watts.input) ?? 0, idle_watts: num(f.idle.input) ?? 0, notes: f.notes.input.value,
      }),
    };
  }

  // ---------------------------------------------------------------- months and years
  function renderPeriods(D) {
    for (const b of $("sav-period").children) b.setAttribute("aria-pressed", String(b.dataset.p === V.period));
    const periods = V.period === "yearly" ? D.yearly : D.monthly.slice(-24);
    const box = $("chart-savings");
    $("sav-monthly-sub").textContent = (V.period === "yearly" ? "Year by year" : "The last " + periods.length + " months") + ": what " + D.baseline_name + " would have charged, against your hardware share and electricity";
    legend($("legend-savings"), [{ label: D.baseline_name, color: css("--lav") }, { label: "Hardware share", color: css("--orange") }, { label: "Electricity", color: css("--mint") }, { label: "Saved (cumulative)", color: css("--ink"), line: true }]);
    if (!periods.length) emptyState(box, "Nothing to show yet.");
    else barChart(box, periods, D);
    // Table: every period, every compared service.
    const tb = $("sav-table"); tb.textContent = "";
    const label = (p) => (V.period === "yearly" ? p.label + (p.partial ? " (so far)" : "") : monthLabel(p.label) + (p.partial ? " (so far)" : ""));
    const cols = [
      { h: V.period === "yearly" ? "Year" : "Month", l: true, v: label },
      { h: "Combined tokens", v: (p) => compact(p.input_tokens + p.output_tokens) },
      { h: D.baseline_name, v: (p) => usd0(p.baseline_cloud) },
      { h: "Hardware share", v: (p) => usd0(p.hardware) },
      { h: "Electricity", v: (p) => usd(p.electricity) },
      { h: "Your cost", v: (p) => usd0(p.local) },
      { h: "Saved", v: (p) => span((p.savings < 0 ? "−" : "") + usd0(Math.abs(p.savings)), p.savings >= 0 ? "pos" : "neg") },
    ];
    for (const [id, name] of D.compared) if (id !== D.baseline) cols.push({ h: name, v: (p) => usd0(p.cloud[id] || 0), title: "What " + name + " would have charged" });
    const rows = (V.period === "yearly" ? D.yearly : D.monthly).slice().reverse();
    rows.push(D.total);
    const t = table(cols, rows);
    t.tBodies[0].lastChild.className = "total";
    tb.append(t);
    $("sav-notes").textContent = "Hardware share spreads each machine's price evenly over its lifecycle (from Settings → Hardware, " + D.default_lifecycle_months + " months by default); after that it's paid off. Electricity counts the hours each host spent generating at its machine's wattage, plus idle power if you set it. Cloud prices are list prices; embeddings aren't priced.";
  }

  function barChart(box, periods, D) {
    const W = Math.max(320, box.clientWidth), H = 300;
    const m = { l: 60, r: 14, t: 14, b: 32 };
    const svg = newSvg(box, W, H, "Savings by " + (V.period === "yearly" ? "year" : "month"));
    let cum = 0;
    const cums = periods.map((p) => (cum += p.savings));
    const lo = Math.min(0, ...cums), hi = Math.max(...periods.map((p) => Math.max(p.baseline_cloud, p.local)), ...cums, 1);
    const { step } = niceMax(hi - lo, 4);
    const min = lo < 0 ? -Math.ceil(-lo / step) * step : 0;
    const top = min + Math.ceil((hi - min) / step - 1e-9) * step;
    const iw = W - m.l - m.r, ih = H - m.t - m.b;
    const y = (v) => m.t + ih - ((v - min) / (top - min)) * ih;
    yAxis(svg, m.l, W - m.r, y, top, step, usdTick, min);
    const n = periods.length, band = iw / n, bw = Math.max(2, Math.min(26, band * 0.36));
    const xc = (i) => m.l + band * i + band / 2;
    periods.forEach((p, i) => {
      const g = el("g", {}, svg);
      if (S.animate !== false && !reduced()) { g.classList.add("anim-bar"); g.style.animationDelay = Math.min(600, i * 30) + "ms"; }
      el("path", { d: topRounded(xc(i) - bw - 1, y(p.baseline_cloud), bw, Math.max(0, y(0) - y(p.baseline_cloud)), 3), fill: css("--lav") }, g);
      const hy = y(p.hardware), ey = y(p.hardware + p.electricity);
      el("rect", { x: xc(i) + 1, y: hy, width: bw, height: Math.max(0, y(0) - hy), fill: css("--orange") }, g);
      el("path", { d: topRounded(xc(i) + 1, ey, bw, Math.max(0, hy - ey), 3), fill: css("--mint") }, g);
    });
    const d = cums.map((v, i) => (i ? "L" : "M") + xc(i).toFixed(1) + "," + y(v).toFixed(1)).join("");
    drawIn(el("path", { d, fill: "none", stroke: css("--ink"), "stroke-width": 2, "stroke-linejoin": "round" }, svg), 300);
    cums.forEach((v, i) => el("circle", { cx: xc(i), cy: y(v), r: 3, fill: css("--ink") }, svg));
    const lbl = (p) => (V.period === "yearly" ? p.label : MONTHS[+p.label.slice(5, 7) - 1] + (p.label.slice(5, 7) === "01" || i0(p) ? " " + p.label.slice(2, 4) : ""));
    const i0 = (p) => p === periods[0];
    xDateTicks(svg, periods, xc, H - 10, Math.floor(iw / 56), lbl);
    const hit = el("g", { "data-ui": "1" }, svg);
    periods.forEach((p, i) => {
      const r = el("rect", { x: m.l + band * i, y: m.t, width: band, height: ih, fill: "transparent" }, hit);
      r.addEventListener("pointermove", (e) => showTip(e, V.period === "yearly" ? p.label : monthLabel(p.label), [
        { color: css("--lav"), label: D.baseline_name, value: usd(p.baseline_cloud) },
        { color: css("--orange"), label: "Hardware share", value: usd(p.hardware) },
        { color: css("--mint"), label: "Electricity", value: usd(p.electricity) },
        { label: "Combined tokens", value: compact(p.input_tokens + p.output_tokens) },
      ], { label: "Saved" + (p.partial ? " so far" : "") + " · running total " + usd0(cums[i]), value: usd(p.savings) }));
      r.addEventListener("pointerleave", hideTip);
    });
  }

  function init() {
    $("hw-add").addEventListener("click", () => editMachine(null));
    $("sav-period").addEventListener("click", (e) => { const b = e.target.closest("button"); if (!b) return; V.period = b.dataset.p; store.set("sav.period", V.period); renderPeriods(V.data); });
    $("sav-baseline").addEventListener("change", async (e) => {
      try { await postJSON("/api/settings", { costs: { baseline: e.target.value } }); show(); } catch (err) { toast(err.message); }
    });
    document.addEventListener("click", (e) => {
      const b = e.target.closest("#page-savings [data-export]"); if (!b) return;
      exportChart("#chart-savings", "#legend-savings", "Savings against " + V.data.baseline_name, V.period === "yearly" ? "By year" : "By month", "toktail-savings-" + V.period + ".svg");
    });
  }
  return { init, show, rerender: () => V.data && render() };
})();


