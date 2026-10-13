// SPDX-License-Identifier: Apache-2.0
// Toktail portal: pages, header status, theme, and refreshes.
//   #/dashboard            the dashboard (default)
//   #/prompts[/<id>|/chat] saved prompts and responses, and chat
//   #/models[/<model>]     per-model speed, wait, errors and cost
//   #/savings              hardware, lifecycle and savings
//   #/settings[/<section>] settings
"use strict";

const Main = (() => {
  const PAGES = ["dashboard", "prompts", "models", "savings", "settings"];

  function parse() {
    const parts = location.hash.replace(/^#\/?/, "").split("/");
    // Old links: #settings
    if (parts[0] === "settings" || parts[0] === "") return { page: parts[0] || "dashboard", sub: parts.slice(1).join("/") };
    return { page: PAGES.includes(parts[0]) ? parts[0] : "dashboard", sub: parts.slice(1).join("/") };
  }

  async function route() {
    const { page, sub } = parse();
    const changed = page !== S.page;
    S.page = page;
    for (const m of document.querySelectorAll("main.page")) m.hidden = m.dataset.page !== page;
    for (const a of $("nav").children) a.setAttribute("aria-current", a.dataset.page === page ? "page" : "false");
    document.title = page === "dashboard" ? "Toktail" : "Toktail · " + page[0].toUpperCase() + page.slice(1);
    if (changed) scrollTo({ top: 0 });
    if (page === "dashboard") { if (changed && S.meta) { S.animate = false; Dashboard.render(); } }
    else if (page === "prompts") await Prompts.show(sub);
    else if (page === "models") await Models.show(sub);
    else if (page === "savings") await Savings.show();
    else if (page === "settings") await Settings.show(sub);
    Stage.platformsChanged();
  }

  function renderStatus() {
    const m = S.meta;
    if (!m) return;
    const ph = $("pill-hosts");
    const enabled = m.hosts.filter((x) => x.enabled && !x.removed);
    const ok = enabled.filter((x) => x.proxy_ok && x.lmstudio && x.lmstudio.ok);
    ph.className = "pill clickable " + (enabled.length && ok.length === enabled.length ? "ok" : "bad");
    ph.lastChild.textContent = enabled.length === 1
      ? (ok.length ? "Counting on port " + enabled[0].listen.split(":").pop() : enabled[0].name + ": " + (!enabled[0].proxy_ok ? "proxy down" : "LM Studio unreachable"))
      : ok.length + " of " + enabled.length + " hosts counting";
    ph.title = enabled.map((x) => (x.proxy_ok && x.lmstudio.ok ? "✓ " : "⚠ ") + x.name + ": localhost:" + x.listen.split(":").pop() + " → " + x.upstream + (x.lmstudio.ok ? "" : " (" + (x.proxy_ok ? x.lmstudio.message : x.proxy_message) + ")")).join("\n") + "\nClick to manage hosts";
    const ps = $("pill-s3");
    if (!m.s3.enabled) { ps.className = "pill"; ps.lastChild.textContent = "S3 off"; ps.title = "Turn on the S3 mirror in Settings"; }
    else if (m.s3.last_error) { ps.className = "pill bad"; ps.lastChild.textContent = "S3 error"; ps.title = m.s3.last_error; }
    else if (m.s3.last_ok) { ps.className = "pill ok"; ps.lastChild.textContent = "S3 synced " + ago(m.s3.last_ok); ps.title = m.s3.target + (m.capture.pending_upload ? "\n" + fmt(m.capture.pending_upload) + " records waiting" : ""); }
    else { ps.className = "pill"; ps.lastChild.textContent = m.s3.in_progress ? "S3 syncing" : "S3 waiting"; ps.title = m.s3.target; }
    $("btn-sync").hidden = !m.s3.enabled;
    if (m.demo && !document.querySelector(".sample-badge")) $("nav").after(h("span", { class: "sample-badge", text: "SAMPLE DATA", title: "make demo: pretend hosts, made-up history and traffic" }));
    $("btn-theme").textContent = isDark() ? "Light mode" : "Dark mode";
  }

  async function refresh() {
    if (S.page === "dashboard") await Dashboard.load();
    else await refreshMeta();
    renderStatus();
    if (S.page === "prompts") Prompts.refresh();
  }
  async function refreshMeta() {
    try { S.meta = await getJSON("/api/meta"); assignColors(); renderStatus(); } catch (e) { /* next time */ }
  }

  // ---------------------------------------------------------------- alerts
  // Budgets crossing 80% and 100%, hosts going quiet and coming back, and
  // stampedes. New ones pop up once; the bell keeps the list.
  const A = { last: Number(store.get("alerts.seen") || 0), list: [] };
  async function pollAlerts(first) {
    try {
      const r = await getJSON("/api/alerts");
      A.list = r.alerts || [];
      const fresh = A.list.filter((a) => a.id > A.last).reverse();
      if (!first) for (const a of fresh.slice(-3)) {
        toast(a.title + (a.message ? ": " + a.message : ""));
        if (typeof Stage !== "undefined") Stage.cheer(a.kind === "budget" ? "Psst! " + a.title + "." : a.kind === "host_down" ? "Uh oh, " + a.title + "." : a.title);
      }
      paintBell();
    } catch (e) { /* next time */ }
  }
  function paintBell() {
    const unseen = A.list.filter((a) => a.id > A.last);
    const b = $("btn-alerts");
    b.classList.toggle("has", unseen.length > 0);
    b.classList.toggle("bad", unseen.some((a) => a.level === "bad"));
    $("alerts-count").textContent = unseen.length ? unseen.length + " new" : "Alerts";
    b.title = unseen.length ? unseen.map((a) => a.title).join("\n") : "No new alerts";
    if (!$("alerts-pop").hidden) paintAlerts();
  }
  function paintAlerts() {
    const pop = $("alerts-pop");
    pop.textContent = "";
    const clear = h("button", { type: "button", class: "small", text: "Clear", onclick: async () => { await getJSON("/api/alerts/clear", { method: "POST" }).catch(() => {}); A.list = []; paintBell(); paintAlerts(); } });
    pop.append(h("h3", {}, span("Alerts", ""), A.list.length ? clear : null));
    if (!A.list.length) pop.append(h("p", { class: "empty", text: "All quiet. Pip will tell you when a budget fills up or a host stops answering." }), h("p", { class: "label-sm", style: "text-align:center" }, h("a", { href: "#/settings/budgets", text: "Budgets and alerts settings", onclick: () => toggleAlerts(false) })));
    for (const a of A.list) pop.append(h("a", { class: "alert " + a.level, href: a.href || "#/dashboard", onclick: () => toggleAlerts(false) }, h("i"), h("span", {}, h("b", { text: a.title }), a.message ? h("small", { text: a.message }) : null, h("time", { text: ago(a.at) }))));
  }
  function toggleAlerts(open) {
    const pop = $("alerts-pop"), b = $("btn-alerts");
    const show = open === undefined ? pop.hidden : open;
    pop.hidden = !show;
    b.setAttribute("aria-expanded", String(show));
    if (show) {
      const r = b.getBoundingClientRect();
      pop.style.top = r.bottom + 8 + "px";
      pop.style.left = Math.max(12, Math.min(innerWidth - pop.offsetWidth - 12, r.right - pop.offsetWidth)) + "px";
      paintAlerts();
      A.last = A.list.reduce((m, a) => Math.max(m, a.id), A.last);
      store.set("alerts.seen", String(A.last));
      setTimeout(paintBell, 1200);
    }
  }

  function init() {
    PipChat.init();
    Dashboard.init();
    Prompts.init();
    Models.init();
    Savings.init();
    Palette.init();
    $("btn-alerts").addEventListener("click", (e) => { e.stopPropagation(); toggleAlerts(); });
    document.addEventListener("click", (e) => { if (!$("alerts-pop").hidden && !e.target.closest("#alerts-pop")) toggleAlerts(false); });
    document.addEventListener("keydown", (e) => { if (e.key === "Escape" && !$("alerts-pop").hidden) toggleAlerts(false); });
    setInterval(() => { if (!document.hidden) pollAlerts(false); }, 10000);
    $("btn-data-folder").addEventListener("click", async () => { try { await getJSON("/api/open-data-folder", { method: "POST" }); } catch (e) { toast(e.message); } });
    $("btn-sync").addEventListener("click", async () => {
      const r = await getJSON("/api/sync", { method: "POST" }).catch(() => ({}));
      toast(r.triggered ? "Syncing to S3" : "S3 is off");
      setTimeout(refresh, 3000);
    });
    $("btn-theme").addEventListener("click", () => {
      const next = isDark() ? "light" : "dark";
      document.documentElement.dataset.theme = next;
      store.set("theme", next);
      Pip.setDark(isDark());
      rerender();
    });
    matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => { Pip.setDark(isDark()); rerender(); });
    let rz; addEventListener("resize", () => { clearTimeout(rz); rz = setTimeout(rerender, 160); });
    setInterval(() => { if (!document.hidden) refresh(); }, 30000);
    document.addEventListener("visibilitychange", () => { if (!document.hidden) refresh(); });
    addEventListener("hashchange", route);
  }
  function rerender() {
    renderStatus();
    if (!S.meta) return;
    S.animate = false;
    if (S.page === "dashboard") Dashboard.render();
    if (S.page === "savings") Savings.rerender();
    if (S.page === "models") Models.rerender();
  }

  async function boot() {
    init();
    await Dashboard.load();
    renderStatus();
    await route();
    pollAlerts(true);
    Stage.start();
  }
  return { boot, refreshMeta, route, toggleAlerts };
})();

Main.boot();

