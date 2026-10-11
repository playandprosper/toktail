// SPDX-License-Identifier: Apache-2.0
// Toktail portal: pages, header status, theme, and refreshes.
//   #/dashboard            the dashboard (default)
//   #/prompts[/<id>|/chat] saved prompts and responses, and chat
//   #/savings              hardware, lifecycle and savings
//   #/settings[/<section>] settings
"use strict";

const Main = (() => {
  const PAGES = ["dashboard", "prompts", "savings", "settings"];

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

  function init() {
    Dashboard.init();
    Prompts.init();
    Savings.init();
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
  }

  async function boot() {
    init();
    await Dashboard.load();
    renderStatus();
    await route();
    Stage.start();
  }
  return { boot, refreshMeta, route };
})();

Main.boot();
