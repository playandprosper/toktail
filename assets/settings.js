// SPDX-License-Identifier: Apache-2.0
// Toktail portal: the Settings page. Hosts, saving prompts and responses,
// encryption and key stores, S3, Pip and the menu bar, cost comparison and
// hardware. Each section saves on its own.
"use strict";

const Settings = (() => {
  let st = null;
  const SECTIONS = [
    ["hosts", "Hosts"], ["flights", "In flight"], ["cache", "Prompt cache"], ["capture", "Saving prompts"], ["encryption", "Encryption"], ["s3", "S3 mirror"],
    ["budgets", "Budgets & alerts"], ["pip", "Pip & menu bar"], ["costs", "Cost comparison"], ["hardware", "Hardware"], ["autopilot", "Autopilot"], ["api", "API & MCP"], ["host", "This computer"],
  ];

  let bud = null;
  /** Where secrets go on this computer, in words. */
  const vault = () => (st && st.secret_store) || "Keychain";
  async function show(section) {
    try { [st, bud] = await Promise.all([getJSON("/api/settings"), getJSON("/api/budgets")]); } catch (e) { return toast("Couldn't load settings: " + e.message); }
    S.settings = st;
    $("settings-sub").textContent = "Changes apply right away. Secrets go to your " + vault() + ", never to a file.";
    render();
    if (section) {
      const t = $("set-" + section);
      if (t) setTimeout(() => t.scrollIntoView({ behavior: reduced() ? "auto" : "smooth", block: "start" }), 30);
    }
  }

  function section(id, title, intro) {
    const sec = h("section", { class: "card set-sec", id: "set-" + id }, h("div", { class: "card-head" }, h("div", {}, h("h2", { text: title }), intro ? h("div", { class: "sub", text: intro }) : null)));
    return sec;
  }
  function saveRow(sec, label, fn, extra) {
    const msg = h("span", { class: "test-msg" });
    const b = h("button", { type: "button", class: "primary", text: label || "Save" });
    b.addEventListener("click", async () => {
      b.disabled = true; msg.textContent = ""; msg.dataset.ok = "";
      try { await fn(msg); } catch (e) { msg.dataset.ok = "0"; msg.textContent = "⚠ " + e.message; }
      b.disabled = false;
    });
    sec.append(h("div", { class: "row-end" }, msg, ...(extra || []), b));
    return msg;
  }
  async function post(body, msg, okText) {
    st = await postJSON("/api/settings", body);
    S.settings = st;
    if (msg) { msg.dataset.ok = "1"; msg.textContent = "✓ " + (okText || "Saved"); }
    if ((typeof Main !== "undefined")) Main.refreshMeta();
  }

  function render() {
    const nav = $("settings-nav"); nav.textContent = "";
    for (const [id, name] of SECTIONS) nav.append(h("a", { href: "#/settings/" + id, text: name }));
    const body = $("settings-body"); body.textContent = "";
    body.append(hostsSection(), flightsSection(), cacheSection(), captureSection(), encryptionSection(), s3Section(), budgetsSection(), pipSection(), costsSection(), hardwareSection(), autopilotSection(), apiSection(), hostSection());
    body.append(h("p", { class: "label-sm" }, "Settings live in " + st.config_file + ". Secrets are in your " + (st.secret_store || "Keychain") + ". Your CSV is in " + st.data_dir + ". ", h("button", { type: "button", class: "small", text: "Open the folder", onclick: () => getJSON("/api/open-data-folder", { method: "POST" }).catch((e) => toast(e.message)) })));
    if ((typeof Stage !== "undefined")) Stage.platformsChanged();
  }

  // ---------------------------------------------------------------- hosts
  function hostsSection() {
    const sec = section("hosts", "Hosts", "The servers Toktail watches, up to " + st.max_hosts + ": LM Studio on your network, or a public API like Anthropic or OpenAI. Each one gets its own Toktail port: point an app (opencode, Claude Code, Codex, Goose, roc…) at that port and its tokens are counted for that host.");
    const list = h("div", { class: "host-list" });
    for (const x of st.hosts) list.append(hostRow(x));
    sec.append(list);
    const add = h("button", { type: "button", class: "small", text: "Add a host", disabled: st.hosts.length >= st.max_hosts });
    add.addEventListener("click", () => {
      const used = new Set(st.hosts.map((x) => x.listen_port));
      let port = 1240; while (used.has(port) || port === st.portal_port) port++;
      list.append(hostRow({ id: "", name: "", url: "http://", listen_port: port, allow_lan: false, enabled: true, has_token: false, kind: "lmstudio", inject_key: true, gateway_keys: [] }, true));
      add.disabled = true;
    });
    sec.append(h("div", { class: "row-end" }, span(st.hosts.length + " of " + st.max_hosts + " hosts", "label-sm"), add));
    return sec;
  }
  const KINDS = [["lmstudio", "LM Studio (on this machine or your network)"], ["anthropic", "Anthropic API"], ["openai", "OpenAI API"], ["other", "Another OpenAI-compatible API"]];
  const KIND_URL = { anthropic: "https://api.anthropic.com", openai: "https://api.openai.com", other: "https://", lmstudio: "http://127.0.0.1:1235" };
  const kindName = (k) => ({ anthropic: "Anthropic", openai: "OpenAI", other: "the API" })[k] || "LM Studio";
  function hostRow(x, isNew) {
    const ok = x.enabled && x.proxy_ok && x.ok;
    const isPublic = (k) => k !== "lmstudio";
    const row = h("div", { class: "host-row" + (isNew ? " open" : "") });
    const summary = h("button", { type: "button", class: "host-sum" },
      h("span", { class: "ht-dot " + (isNew ? "" : !x.enabled ? "" : ok ? "ok" : "bad") }),
      h("b", { text: x.name || "New host" }),
      isNew ? null : span((x.public ? "☁ " : "") + x.url.replace(/^https?:\/\//, "") + " → localhost:" + x.listen_port, "label-sm"),
      isNew ? null : span(!x.enabled ? "turned off" : ok ? (x.public ? "✓ reachable" : "✓ " + (x.models || []).length + " models") : "⚠ " + (x.proxy_ok ? x.message : x.proxy_message), "label-sm host-state"));
    const form = h("div", { class: "host-form" });
    const kind = selectField("Host type", "LM Studio hosts take an optional API token that Toktail adds for every app. Public API hosts keep each app's own key, or hand out Toktail gateway keys that stand in for a key stored here.", KINDS, x.kind || "lmstudio");
    const name = textField("Name", "What the dashboard calls it", x.name, "Mac Studio");
    const url = textField("LM Studio's address", "Its server address and port, like http://127.0.0.1:1235 or http://studio.local:1234", x.url, "http://127.0.0.1:1235");
    const tok = secretField("API token", "", { saved: x.has_token, hint: x.token_hint });
    const clear = x.has_token ? toggleField("Remove the saved token", null, false) : null;
    const inject = toggleField("Add the stored key for apps on this Mac that send none", "Only for requests from this computer. Apps on other machines always need their own key or a gateway key; Toktail never lends its stored key over the network.", x.inject_key !== false);
    const port = numberField("Toktail listens on port", "Apps call this port instead of the server's.", x.listen_port, "1", "", 1, 65535);
    const lan = toggleField("Let other devices on your network use it", "Off means only apps on this Mac can reach this port.", x.allow_lan);
    const en = toggleField("Watch this host", "Off stops the proxy for this host; its history stays.", x.enabled);
    const hint = h("div", { class: "label-sm client-hint" });
    const lanNote = h("div", { class: "label-sm warn-note", hidden: true });
    const tokLabel = tok.row.querySelector(".row-label");
    tokLabel.append(h("div", { class: "hint" }));
    const tokHint = tokLabel.querySelector(".hint");
    const setLabel = (f, text) => { const l = f.row.querySelector(".row-label"); if (l && l.firstChild && l.firstChild.nodeType === 3) l.firstChild.textContent = text; };
    const applyKind = () => {
      const k = kind.input.value, pub = isPublic(k);
      setLabel(url, pub ? kindName(k) + "'s address" : "LM Studio's address");
      url.input.placeholder = KIND_URL[k];
      if ((x.kind || "lmstudio") !== k && (url.input.value === "" || url.input.value === "http://" || url.input.value === KIND_URL[x.kind || "lmstudio"])) url.input.value = KIND_URL[k];
      setLabel(tok, pub ? kindName(k) + " API key" : "API token");
      if (tokHint) tokHint.textContent = pub
        ? (x.has_token ? "Stored in your " + vault() + ", used only on this computer or behind a gateway key. Type a new one to replace it." : "Optional. Kept in your " + vault() + ", never in a file, never logged, never sent to another machine. Apps that bring their own key don't need it.")
        : (x.has_token ? "Saved in your " + vault() + ". Type a new one to replace it." : "Only if this LM Studio asks for one. Kept in your " + vault() + ", never in a file.");
      inject.row.hidden = !pub;
      gw.hidden = !pub || isNew;
      lanNote.hidden = !(pub && lan.input.checked);
      lanNote.textContent = "Other machines reach " + kindName(k) + " through this port with their own key (sent over plain HTTP on your LAN) or, better, with a Toktail gateway key issued below. Requests without either are refused.";
    };
    const showHint = () => { hint.textContent = "Apps use http://" + (lan.input.checked ? "<this Mac's name or IP>" : "localhost") + ":" + (port.input.value || x.listen_port) + (kind.input.value === "anthropic" ? "  (ANTHROPIC_BASE_URL; Claude Code and roc point there)" : "/v1"); applyKind(); };
    // Gateway keys (public hosts only).
    const gw = h("div", { class: "gateway-keys", hidden: true });
    const gwList = h("div", { class: "gw-list" });
    const renderKeys = () => {
      gwList.textContent = "";
      for (const k of (x.gateway_keys || [])) {
        const rm = h("button", { type: "button", class: "small danger", text: "Revoke" });
        rm.addEventListener("click", async () => {
          if (!(await confirmBox("Revoke " + (k.label || k.id) + "?", "Apps using this gateway key stop working at once.", "Revoke", true))) return;
          try { st = await getJSON("/api/hosts/" + encodeURIComponent(x.id) + "/gateway-keys/" + encodeURIComponent(k.id), { method: "DELETE" }); S.settings = st; x.gateway_keys = (st.hosts.find((hh) => hh.id === x.id) || {}).gateway_keys || []; renderKeys(); } catch (e) { toast(e.message); }
        });
        gwList.append(h("div", { class: "gw-row" }, h("code", { text: k.id }), span(k.label || "", ""), span(k.lan ? "this Mac + network" : "this Mac only", "label-sm"), span(new Date(k.created).toLocaleDateString(), "label-sm"), rm));
      }
      if (!(x.gateway_keys || []).length) gwList.append(h("div", { class: "label-sm", text: "No gateway keys yet." }));
    };
    renderKeys();
    const gwLabel = h("input", { type: "text", placeholder: "Label, like roc on the laptop", class: "wide" });
    const gwLan = h("label", { class: "inline" }, h("input", { type: "checkbox" }), " usable from other machines");
    const issue = h("button", { type: "button", class: "small", text: "Issue a gateway key" });
    const reveal = h("div", { class: "gw-reveal", hidden: true });
    issue.addEventListener("click", async () => {
      issue.disabled = true;
      try {
        const r = await postJSON("/api/hosts/" + encodeURIComponent(x.id) + "/gateway-keys", { label: gwLabel.value, lan: gwLan.querySelector("input").checked });
        st = r.settings; S.settings = st; x.gateway_keys = (st.hosts.find((hh) => hh.id === x.id) || {}).gateway_keys || []; renderKeys();
        reveal.hidden = false; reveal.textContent = "";
        const code = h("code", { class: "gw-key", text: r.key });
        const copy = h("button", { type: "button", class: "small", text: "Copy" });
        copy.addEventListener("click", () => { navigator.clipboard && navigator.clipboard.writeText(r.key).then(() => toast("Gateway key copied")); });
        reveal.append(h("div", { class: "label-sm", text: "Copy it now. Toktail keeps only a hash and can't show it again." }), h("div", { class: "with-btn" }, code, copy),
          h("div", { class: "label-sm", text: "Use it as the API key: ANTHROPIC_AUTH_TOKEN / OPENAI_API_KEY / ROC_AI_API_TOKEN = this key, base URL = the address above." }));
        gwLabel.value = "";
      } catch (e) { toast(e.message); }
      issue.disabled = false;
    });
    gw.append(h("div", { class: "label-sm", style: "margin-top:12px", text: "Gateway keys" }),
      h("div", { class: "hint", text: "A gateway key is a stand-in: an app sends it, Toktail swaps in the stored " + kindName(x.kind) + " key for the life of the request, and the real key never leaves this Mac's " + vault() + ". Each key has its own one-way id in the CSV, so you can see what each app spent." }),
      gwList, h("div", { class: "with-btn" }, gwLabel, gwLan, issue), reveal);
    showHint(); port.input.addEventListener("input", showHint); lan.input.addEventListener("change", showHint); kind.input.addEventListener("change", showHint);
    form.append(kind.row, name.row, url.row, tok.row);
    if (clear) form.append(clear.row);
    form.append(inject.row, port.row, lan.row, lanNote, en.row, hint, gw);
    if (x.lan_key_from) form.append(h("div", { class: "label-sm warn-note", text: "⚠ A provider key arrived from " + x.lan_key_from + " over plain HTTP. Give that machine a gateway key instead." }));
    const testMsg = h("span", { class: "test-msg" });
    if (!isNew && x.checked) { testMsg.textContent = (x.ok ? "✓ " : "⚠ ") + x.message; testMsg.dataset.ok = x.ok ? "1" : "0"; }
    const test = h("button", { type: "button", class: "small", text: "Test connection" });
    test.addEventListener("click", async () => {
      test.disabled = true; testMsg.textContent = "Knocking on " + kindName(kind.input.value) + "'s door…"; testMsg.dataset.ok = "";
      try {
        const r = await postJSON("/api/hosts/test", { url: url.input.value, token: tok.input.value, id: x.id || null, kind: kind.input.value });
        testMsg.dataset.ok = r.ok ? "1" : "0";
        testMsg.textContent = (r.ok ? "✓ " : "⚠ ") + r.message + (r.ok && r.models.length ? ": " + r.models.slice(0, 4).join(", ") + (r.models.length > 4 ? "…" : "") : "") + (r.ok ? " (" + r.ms + " ms)" : "");
      } catch (e) { testMsg.dataset.ok = "0"; testMsg.textContent = "⚠ " + e.message; }
      test.disabled = false;
    });
    const save = h("button", { type: "button", class: "primary small", text: isNew ? "Add host" : "Save host" });
    save.addEventListener("click", async () => {
      save.disabled = true;
      try {
        const r = await postJSON("/api/hosts", { id: x.id || null, name: name.input.value, url: url.input.value, listen_port: parseInt(port.input.value, 10) || 0, allow_lan: lan.input.checked, enabled: en.input.checked, token: tok.input.value || null, clear_token: clear ? clear.input.checked : false, kind: kind.input.value, inject_key: inject.input.checked });
        st = r.settings; S.settings = st;
        toast(isNew ? "Added. Pip is watching " + name.input.value + "." : "Saved");
        render();
        if ((typeof Main !== "undefined")) Main.refreshMeta();
      } catch (e) { testMsg.dataset.ok = "0"; testMsg.textContent = "⚠ " + e.message; save.disabled = false; }
    });
    const del = isNew ? h("button", { type: "button", class: "small", text: "Cancel", onclick: () => render() })
      : h("button", { type: "button", class: "small danger", text: "Remove", disabled: st.hosts.length <= 1, title: st.hosts.length <= 1 ? "Toktail needs at least one host" : "" });
    if (!isNew) del.addEventListener("click", async () => {
      if (!(await confirmBox("Remove " + x.name + "?", "Toktail stops listening on port " + x.listen_port + " and forgets this host's stored key. Its token history stays in your CSV and on the dashboard.", "Remove", true))) return;
      try { st = await getJSON("/api/hosts/" + encodeURIComponent(x.id), { method: "DELETE" }); S.settings = st; render(); if ((typeof Main !== "undefined")) Main.refreshMeta(); } catch (e) { toast(e.message); }
    });
    form.append(h("div", { class: "row-end" }, testMsg, test, del, save));
    summary.addEventListener("click", () => row.classList.toggle("open"));
    row.append(summary, form);
    return row;
  }

  // ---------------------------------------------------------------- in flight
  function flightsSection() {
    const c = st.flights || { join_duplicates: true, abandoned_grace_secs: 180 };
    const sec = section("flights", "Requests in flight", "Coding agents pile requests up: a long prompt is still being read when the agent times out and sends it again, and subagents queue behind each other. On the dashboard's Right now card you can cancel any request, reroute one that hasn't started answering to another host and model, or let it share an identical request's answer.");
    const join = toggleField("When an agent sends the same prompt again, answer it with the one already being read", "Same host, model, prompt, tools and key (a retry may change settings like max_tokens or temperature). LM Studio reads one prompt at a time per model, so a resent prompt would otherwise wait in line and be read all over again. The latest attempt gets the original's answer, streamed from the start.", c.join_duplicates);
    const grace = numberField("When an agent stops waiting, keep its request (and its finished answer) for", "Long enough for the agent's retry to arrive and pick up the answer, even if the answer finished in the meantime. If no retry comes, Toktail stops it on the model server. 0 stops it at once.", c.abandoned_grace_secs, "10", "seconds", 0, 3600);
    sec.append(join.row, grace.row);
    saveRow(sec, "Save", (msg) => post({ flights: { join_duplicates: join.input.checked, abandoned_grace_secs: num(grace.input) } }, msg));
    return sec;
  }

  // ---------------------------------------------------------------- prompt cache
  function cacheSection() {
    const c = st.cache || { enabled: false, ttl_hours: 72, max_mb: 2048 };
    const sec = section("cache", "Prompt cache", "When on, a request identical to one already answered (same host, endpoint, body and credential) gets the saved answer back at once; no model runs. Answers are encrypted on disk like saved records. Streams are replayed as streams. Errors, refusals and cancelled requests are never cached. Apps can skip it with Cache-Control: no-cache or X-Toktail-Cache: bypass.");
    const en = toggleField("Answer repeated prompts from the cache", "Applies to every host Toktail listens for.", c.enabled);
    const ttl = numberField("Keep answers for", "Each answer expires this long after it was stored.", c.ttl_hours, "1", "hours", 1, 8760);
    const cap = numberField("Cache size", "Oldest answers go first when the cache is full.", c.max_mb, "64", "MB", 16, 1048576);
    sec.append(en.row, ttl.row, cap.row);
    const stat = h("div", { class: "label-sm cache-stat", text: "…" });
    const refreshStat = () => getJSON("/api/cache").then((r) => { stat.textContent = fmt(r.entries) + " answers, " + (r.bytes / 1048576).toFixed(1) + " MB · " + fmt(r.hits) + " hits, " + fmt(r.misses) + " misses since launch · in " + r.dir; }).catch(() => { stat.textContent = ""; });
    refreshStat();
    const clear = h("button", { type: "button", class: "small danger", text: "Empty the cache" });
    clear.addEventListener("click", async () => {
      if (!(await confirmBox("Empty the prompt cache?", "Every saved answer is deleted. Your CSV and saved records are untouched.", "Empty", true))) return;
      try { const r = await getJSON("/api/cache/clear", { method: "POST" }); toast("Removed " + fmt(r.removed) + " answers"); refreshStat(); } catch (e) { toast(e.message); }
    });
    sec.append(stat);
    saveRow(sec, "Save", (msg) => post({ cache: { enabled: en.input.checked, ttl_hours: num(ttl.input), max_mb: num(cap.input) } }, msg), [clear]);
    return sec;
  }

  // ---------------------------------------------------------------- capture
  const RETENTION = [[0, "Keep forever"], [30, "30 days"], [90, "90 days"], [180, "6 months"], [365, "1 year"], [730, "2 years"], [1095, "3 years"], [1825, "5 years"], [-1, "Custom…"]];
  function retentionField(label, hint, days) {
    const known = RETENTION.some(([d]) => d === days);
    const s = selectField(label, hint, RETENTION, known ? days : -1);
    const custom = h("input", { type: "number", min: "1", max: "36500", step: "1", value: known ? "" : String(days), class: "days", hidden: known });
    s.input.after(custom);
    s.input.addEventListener("change", () => { custom.hidden = s.input.value !== "-1"; if (!custom.hidden) custom.focus(); });
    return { row: s.row, value: () => (s.input.value === "-1" ? Math.max(1, parseInt(custom.value, 10) || 365) : parseInt(s.input.value, 10)) };
  }
  function captureSection() {
    const c = st.capture;
    const sec = section("capture", "Saving prompts and responses", "Off by default. When on, Toktail keeps each request's prompt and the model's response as gzip files named for the moment they finished (YYYY-MM-DD-HH-mm-SS.prompt and .response), encrypted if you turn that on. The CSV still holds only counts and fingerprints.");
    const en = toggleField("Save prompts and responses", "Lets you read, search and re-run them on the Prompts page.", c.enabled);
    const dir = h("input", { type: "text", class: "wide", value: c.dir, placeholder: c.effective_dir, spellcheck: "false" });
    const pick = h("button", { type: "button", class: "small", text: "Choose…" });
    pick.addEventListener("click", async () => {
      try { const r = await getJSON("/api/pick-folder", { method: "POST" }); if (r.path) dir.value = r.path; }
      catch (e) { toast(e.message); dir.focus(); }
    });
    const dirRow = field("Save to", "Leave empty for " + c.effective_dir + ". Files go in <host>/<year>/<month>/<day>/ inside it.", h("div", { class: "with-btn" }, dir, pick), { stack: true });
    const up = toggleField("Upload them to S3 too", st.s3.enabled ? "Mirrored to " + (st.s3.prefix || "") + "records/ in your bucket." : "S3 is off. Turn it on below to use this.", c.s3_upload);
    const local = retentionField("Keep on this Mac", "Rolling window. With S3 on, a file is only removed here once it's uploaded.", c.local_retention_days);
    const s3r = retentionField("Keep in S3", "Rolling window, for example 3 years. Keep forever suits your own MinIO.", c.s3_retention_days);
    sec.append(en.row, dirRow, up.row, local.row, s3r.row);
    if (c.pending_upload) sec.append(h("p", { class: "label-sm", text: fmt(c.pending_upload) + " records are waiting to upload." }));
    if (c.error) sec.append(h("p", { class: "banner warn", text: "Last save failed: " + c.error }));
    if (en.input.checked && st.encryption.mode === "off") sec.append(h("p", { class: "label-sm", text: "Tip: files are only compressed until you turn on encryption below." }));
    saveRow(sec, "Save", (msg) => post({ capture: { enabled: en.input.checked, dir: dir.value, s3_upload: up.input.checked, local_retention_days: local.value(), s3_retention_days: s3r.value() } }, msg, en.input.checked ? "Saved. New requests are being kept." : "Saved"));
    return sec;
  }

  // ---------------------------------------------------------------- encryption
  function encryptionSection() {
    const e = st.encryption;
    const sec = section("encryption", "Encryption", "Encrypts saved prompts and responses at rest, here and in S3 (AES-256-GCM, keys from Argon2id). Toktail decrypts them for you on the Prompts page.");
    const status = h("p", { class: "banner" + (e.problem ? " warn" : "") , text: e.status });
    sec.append(status);
    const modes = [["off", "Off", "Files are gzip only."], ["password", "One password", "Every file is encrypted with a key made from your password."], ["per_file", "Random password for every file", "Each file gets its own random key, and Toktail keeps track of the keys in a secrets store you choose."]];
    const modeBox = h("div", { class: "radios", role: "radiogroup", "aria-label": "Encryption" });
    for (const [v, t, d] of modes) {
      const r = h("input", { type: "radio", name: "enc-mode", value: v });
      r.checked = e.mode === v;
      modeBox.append(h("label", { class: "radio" }, r, h("span", {}, h("b", { text: t }), h("small", { text: d }))));
    }
    sec.append(modeBox);
    const pw = secretField("Encryption password", "" + "Kept in your " + vault() + ". Needed for One password, and to seal the key file below.", e.password, "At least 8 characters");
    pw.input.autocomplete = "new-password";
    const fresh = e.has_key_check ? toggleField("Start fresh with a new password", "Only if you've lost the old one. Files saved with the old password stay unreadable until it's set back.", false) : null;
    const pwBox = h("div", { class: "sub-box" }, pw.row);
    if (fresh) pwBox.append(fresh.row);
    if (e.password_from_env) pwBox.append(h("p", { class: "label-sm", text: "Set by the TOKTAIL_ENCRYPTION_PASSWORD environment variable." }));
    sec.append(pwBox);

    const stores = [["file", "Encrypted file in Toktail's folder", "keystore.tkt, sealed with your password, mirrored to S3."], ["1password", "1Password", "One item per key in a vault, through the op command-line tool."], ["vault", "HashiCorp Vault", "One secret per key in a KV v2 engine."]];
    const ksBox = h("div", { class: "sub-box" }, h("div", { class: "label-sm", text: "Where to keep the per-file keys" }));
    const ksRadios = h("div", { class: "radios" });
    for (const [v, t, d] of stores) {
      const r = h("input", { type: "radio", name: "enc-ks", value: v });
      r.checked = e.keystore === v;
      ksRadios.append(h("label", { class: "radio" }, r, h("span", {}, h("b", { text: t }), h("small", { text: d }))));
    }
    ksBox.append(ksRadios);
    const op = h("div", { class: "ks-fields", "data-ks": "1password" });
    const opVault = textField("1Password vault", "Toktail creates one item per key here.", e.onepassword_vault, "Toktail");
    const opTok = secretField("Service account token", "Optional: without it, Toktail uses the 1Password app's sign-in. " + "Kept in your " + vault() + ".", e.onepassword_token, "ops_…");
    op.append(opVault.row, opTok.row);
    const vt = h("div", { class: "ks-fields", "data-ks": "vault" });
    const vAddr = textField("Vault address", null, e.vault_addr, "https://vault.example.com:8200");
    const vMount = textField("KV v2 mount", null, e.vault_mount, "secret");
    const vPath = textField("Path", "Keys go under <mount>/data/<path>/<key id>.", e.vault_path, "toktail");
    const vNs = textField("Namespace", "Vault Enterprise only. Leave empty otherwise.", e.vault_namespace, "");
    const vTok = secretField("Vault token", "Needs create, read and delete on that path. " + "Kept in your " + vault() + ".", e.vault_token, "hvs.…");
    vt.append(vAddr.row, vMount.row, vPath.row, vNs.row, vTok.row);
    ksBox.append(op, vt);
    sec.append(ksBox);
    const sync = () => {
      const mode = sec.querySelector("input[name=enc-mode]:checked").value;
      const ks = sec.querySelector("input[name=enc-ks]:checked").value;
      ksBox.hidden = mode !== "per_file";
      pwBox.hidden = !(mode === "password" || (mode === "per_file" && ks === "file"));
      op.hidden = ks !== "1password"; vt.hidden = ks !== "vault";
    };
    sec.addEventListener("change", sync);
    sync();
    const testMsg = h("span", { class: "test-msg" });
    const test = h("button", { type: "button", class: "small", text: "Test key store" });
    test.addEventListener("click", async () => {
      test.disabled = true; testMsg.textContent = "Checking…"; testMsg.dataset.ok = "";
      try { const r = await getJSON("/api/encryption/test", { method: "POST" }); testMsg.dataset.ok = r.ok ? "1" : "0"; testMsg.textContent = (r.ok ? "✓ " : "⚠ ") + r.message; }
      catch (err) { testMsg.dataset.ok = "0"; testMsg.textContent = "⚠ " + err.message; }
      test.disabled = false;
    });
    saveRow(sec, "Save", async (msg) => {
      const mode = sec.querySelector("input[name=enc-mode]:checked").value;
      const ks = sec.querySelector("input[name=enc-ks]:checked").value;
      if (mode !== "off" && (mode === "password" || ks === "file") && !e.password.saved && !pw.input.value) throw new Error("Enter an encryption password first.");
      await post({ encryption: {
        mode, keystore: ks, password: pw.input.value || undefined, start_fresh: fresh ? fresh.input.checked : undefined,
        onepassword_vault: opVault.input.value, onepassword_token: opTok.input.value || undefined,
        vault_addr: vAddr.input.value, vault_mount: vMount.input.value, vault_path: vPath.input.value, vault_namespace: vNs.input.value, vault_token: vTok.input.value || undefined,
      } }, msg);
      pw.input.value = ""; opTok.input.value = ""; vTok.input.value = "";
      status.textContent = st.encryption.status;
      status.className = "banner" + (st.encryption.problem ? " warn" : "");
      msg.textContent = st.encryption.problem ? "⚠ Saved, but: " + st.encryption.problem : "✓ Saved. " + st.encryption.status;
      msg.dataset.ok = st.encryption.problem ? "0" : "1";
    }, [testMsg, test]);
    return sec;
  }

  // ---------------------------------------------------------------- S3
  function s3Section() {
    const s = st.s3;
    const sec = section("s3", "S3 mirror", "Copies your CSV files, and saved prompts and responses if you want, to any S3-compatible bucket: AWS, Cloudflare R2, Backblaze B2, MinIO. Your Mac's copy stays the source of truth.");
    const en = toggleField("Mirror to S3", null, s.enabled);
    const ep = textField("Endpoint", "For example https://s3.us-east-1.amazonaws.com or http://minio.local:9000", s.endpoint, "https://");
    const region = textField("Region", "auto works for R2 and MinIO", s.region, "auto");
    const bucket = textField("Bucket", null, s.bucket, "");
    const prefix = textField("Folder (prefix)", null, s.prefix, "toktail/");
    const ps = toggleField("Path-style URLs", "Usually on for MinIO and other self-hosted servers.", s.path_style);
    const key = textField("Access key ID", null, s.access_key_id, "");
    const secret = secretField("Secret access key", "" + "Kept in your " + vault() + ".", s.secret, "");
    const every = numberField("Sync every", null, s.sync_interval_minutes, "1", "minutes", 1, 1440);
    sec.append(en.row, ep.row, region.row, bucket.row, prefix.row, ps.row, key.row, secret.row, every.row);
    const testMsg = h("span", { class: "test-msg" });
    const test = h("button", { type: "button", class: "small", text: "Test bucket" });
    const body = () => ({ s3: { endpoint: ep.input.value, region: region.input.value, bucket: bucket.input.value, prefix: prefix.input.value, path_style: ps.input.checked, access_key_id: key.input.value, secret: secret.input.value || undefined, sync_interval_minutes: num(every.input) } });
    test.addEventListener("click", async () => {
      test.disabled = true; testMsg.textContent = "Saving and writing a test object…"; testMsg.dataset.ok = "";
      try {
        await post(body());
        secret.input.value = "";
        const r = await getJSON("/api/s3/test", { method: "POST" });
        testMsg.dataset.ok = r.ok ? "1" : "0"; testMsg.textContent = (r.ok ? "✓ " : "⚠ ") + r.message;
      } catch (err) { testMsg.dataset.ok = "0"; testMsg.textContent = "⚠ " + err.message; }
      test.disabled = false;
    });
    saveRow(sec, "Save", async (msg) => { const b = body(); b.s3.enabled = en.input.checked; await post(b, msg, en.input.checked ? "Saved. Syncing now." : "Saved"); secret.input.value = ""; }, [testMsg, test]);
    return sec;
  }

  // ---------------------------------------------------------------- pip & menu bar
  function pipSection() {
    const sec = section("pip", "Pip and the menu bar");
    const intro = toggleField("Run out from the middle when the portal opens", "The intro plays once each time you open the portal.", st.portal.intro_animation);
    const roams = toggleField("Let Pip roam", "Pip runs along the trail while tokens stream, and hops onto the cards near your pointer.", st.portal.pip_roams);
    const anim = toggleField("Animate Pip in the menu bar", "Off keeps Pip still in the menu bar: awake while tokens flow, asleep otherwise.", st.app.animate_menu_bar);
    const count = toggleField("Show today's count in the menu bar", null, st.app.show_count_in_menu_bar);
    const stamp = numberField("Stampede at", "0 means automatic: three times your usual speed.", st.app.stampede_tokens_per_sec, "10", "tokens/sec");
    const sm = st.summaries || { enabled: true, bubbles: true, max_queue: 200, status: {} };
    const sums = toggleField("Summarize every prompt in one sentence", "For each prompt that goes to an LM Studio host on your network, Pip asks the same model, while it has nothing else to do, what the prompt is (\u201cSends back the output of go test, which has two failures.\u201d). The sentence is kept with the request, searchable on the Prompts page and readable through the API, sealed like your saved records. The same prompt sent again is summarized once. Never for public API hosts.", sm.enabled);
    const bub = toggleField("Pip says each summary in its speech bubble", "Watch what your agents are doing as it happens. Off: Pip's bubbles go back to its whimsical commentary below.", sm.bubbles);
    const q = numberField("Prompts waiting for a summary", "When agents keep the model busy, the oldest waiting ones are skipped.", sm.max_queue, "10", "", 1, 10000);
    const ss = sm.status || {};
    const sline = h("div", { class: "label-sm cache-stat", text: fmt(ss.stored || 0) + " summaries kept · " + fmt(ss.queued || 0) + " waiting · since launch: " + fmt(ss.done_since_start || 0) + " written, " + fmt(ss.dropped_since_start || 0) + " skipped, " + fmt(ss.failed_since_start || 0) + " failed" });
    const cm = st.commentary || { enabled: true, every_minutes: 5 };
    const talk = toggleField("Let Pip comment on what your agents are doing", "When opencode, Claude Code, Codex, Goose or roc send a prompt to an LM Studio host on your network, Pip asks that same local model for one whimsical sentence about it and says it in a speech bubble (when summaries aren't in the bubbles). Never for public API hosts; nothing leaves your network. Press u or Space on the dashboard to ask Pip right away.", cm.enabled);
    const every = numberField("At most once every", "Pip waits this long between comments.", cm.every_minutes, "1", "minutes", 1, 1440);
    sec.append(intro.row, roams.row, anim.row, count.row, stamp.row, sums.row, bub.row, q.row, sline, talk.row, every.row);
    saveRow(sec, "Save", (msg) => post({ portal: { intro_animation: intro.input.checked, pip_roams: roams.input.checked }, app: { animate_menu_bar: anim.input.checked, show_count_in_menu_bar: count.input.checked, stampede_tokens_per_sec: num(stamp.input) }, summaries: { enabled: sums.input.checked, bubbles: bub.input.checked, max_queue: Math.round(num(q.input) || 200) }, commentary: { enabled: talk.input.checked, every_minutes: num(every.input) } }, msg).then(() => { if ((typeof Stage !== "undefined")) Stage.applySettings(); }));
    return sec;
  }

  // ---------------------------------------------------------------- costs
  function costsSection() {
    const c = st.costs;
    const sec = section("costs", "Cloud cost comparison", "Prices what you ran locally at public API list prices (" + st.prices.as_of + "), against what your hosts cost: hardware share and electricity.");
    const show = toggleField("Show the cost comparison on the dashboard", null, c.show_comparison);
    const base = selectField("Measure savings against", null, st.prices.models.map((m) => [m.id, m.provider + " " + m.name.replace(m.provider + " ", "")]), c.baseline);
    const kwh = numberField("Electricity price", null, c.electricity_usd_per_kwh, "0.01", "$ per kWh");
    const watts = numberField("Power while generating", "Used for hosts without a machine on the Savings page.", c.watts_while_generating, "10", "W");
    const cacheAware = toggleField("Price repeated prompt prefixes at the providers' cached-input rate", "Coding agents resend the whole conversation each turn. Providers charge the repeated part (the prefix a prompt cache would hit) less, typically a tenth of the input price. On, the cloud estimate uses that rate for the repeated tokens; off, every input token costs full price.", c.cache_aware !== false && c.cache_aware === true);
    const mk = st.market || { enabled: true };
    const market = toggleField("Check market prices once a day", "Fetches the public model price lists from LiteLLM and OpenRouter, alternating, keeps a dated history, and uses the newest prices for the comparison. No API key needed; the OpenRouter key below is optional.", c.market_prices !== false && mk.enabled !== false);
    const orKey = secretField("OpenRouter API key (optional)", "Only if OpenRouter's public model list ever requires one. Kept in your " + vault() + ".", mk.openrouter_key || { saved: false, hint: null });
    sec.append(show.row, base.row, kwh.row, watts.row, cacheAware.row, market.row, orKey.row);
    const mst = mk.status || {};
    const mline = h("div", { class: "label-sm cache-stat", text: mst.latest ? "Latest: " + mst.latest.models + " models from " + mst.latest.source + " on " + mst.latest.date + " · " + mst.days + " day" + (mst.days === 1 ? "" : "s") + " of history in " + mst.dir + (mk.applied ? " · in use" : " · not in use") : "No market prices fetched yet. Toktail checks a minute after launch, then once a day." });
    const check = h("button", { type: "button", class: "small", text: "Check prices now" });
    check.addEventListener("click", async () => {
      check.disabled = true; mline.textContent = "Asking " + (mk.next_source || "litellm") + "…";
      try { const r = await postJSON("/api/market/check", {}); if (r.settings) { st = r.settings; S.settings = st; } mline.textContent = (r.ok ? "✓ " : "⚠ ") + r.message; if (r.ok) toast("Prices updated"); }
      catch (e) { mline.textContent = "⚠ " + e.message; }
      check.disabled = false;
    });
    sec.append(h("div", { class: "with-btn" }, mline, check));
    const pick = h("div", { class: "models" });
    for (const m of st.prices.models) {
      const i = h("input", { type: "checkbox", value: m.id, class: "set-compare" });
      i.checked = c.compare.includes(m.id);
      pick.append(h("label", {}, i, h("span", {}, m.provider + " " + m.name.replace(m.provider + " ", ""), h("small", { text: "$" + m.input + " in, $" + m.output + " out per 1M" + (m.per_request ? ", plus $" + m.per_request + " a request" : "") }))));
    }
    sec.append(h("div", { class: "label-sm", style: "margin-top:10px", text: "Compare against:" }), pick);
    sec.append(h("p", { class: "label-sm", text: "To update prices, put your own pricing.json next to " + st.config_file + "." }));
    saveRow(sec, "Save", (msg) => post({ costs: { show_comparison: show.input.checked, baseline: base.input.value, electricity_usd_per_kwh: num(kwh.input), watts_while_generating: num(watts.input), cache_aware: cacheAware.input.checked, market_prices: market.input.checked, compare: [...sec.querySelectorAll(".set-compare:checked")].map((i) => i.value) }, market: { enabled: market.input.checked, openrouter_key: orKey.input.value || undefined } }, msg).then(() => { orKey.input.value = ""; }));
    return sec;
  }

  // ---------------------------------------------------------------- budgets and alerts
  /** "50M", "2.5 billion", "120,000" -> tokens. */
  function parseTokens(t) {
    const m = /^\s*([\d.,]+)\s*([kmbt]|thousand|million|billion|trillion)?\s*$/i.exec(String(t || ""));
    if (!m) return NaN;
    const n = parseFloat(m[1].replace(/,/g, ""));
    const mult = { k: 1e3, thousand: 1e3, m: 1e6, million: 1e6, b: 1e9, billion: 1e9, t: 1e12, trillion: 1e12 }[(m[2] || "").toLowerCase()] || 1;
    return Math.round(n * mult);
  }
  function budgetsSection() {
    const sec = section("budgets", "Budgets and alerts", "A budget is a token limit per day, week (Monday to Sunday) or month, for every host or just one, every model or just one. The dashboard shows how far along each one is and where it's heading; Pip taps you on the shoulder at 80% and again at 100%.");
    const list = h("div", {});
    const head = h("div", { class: "budget-edit budget-head" }, ...["Name", "Host", "Model", "Tokens", "Per", "Limit", "Notify", ""].map((t) => span(t, "")));
    const hosts = [["", "All hosts"]].concat((S.meta ? S.meta.hosts : st.hosts).filter((x) => !x.removed).map((x) => [x.id, x.name]));
    const models = [["", "All models"]].concat((S.meta ? S.meta.models : []).map((m) => [m.model, m.model]));
    const rows = [];
    const addRow = (b) => {
      const name = h("input", { type: "text", value: b.name || "", placeholder: "Daily tokens", "aria-label": "Name" });
      const host = h("select", { "aria-label": "Host" }); for (const [v, t] of hosts) host.append(h("option", { value: v, text: t })); host.value = b.host || "";
      const model = h("select", { "aria-label": "Model" }); for (const [v, t] of models) model.append(h("option", { value: v, text: t }));
      if (b.model && ![...model.options].some((o) => o.value === b.model)) model.append(h("option", { value: b.model, text: b.model }));
      model.value = b.model || "";
      const metric = h("select", { "aria-label": "Which tokens" }); for (const [v, t] of [["combined", "Combined"], ["output", "Output"], ["input", "Input"]]) metric.append(h("option", { value: v, text: t })); metric.value = b.metric || "combined";
      const period = h("select", { "aria-label": "Period" }); for (const [v, t] of [["day", "Day"], ["week", "Week"], ["month", "Month"]]) period.append(h("option", { value: v, text: t })); period.value = b.period || "day";
      const limit = h("input", { type: "text", value: b.limit ? compact(b.limit) : "", placeholder: "50M", "aria-label": "Limit (tokens)", title: "Tokens. 50M, 2.5B and 120,000 all work." });
      const notify = h("input", { type: "checkbox", class: "switch", "aria-label": "Notify" }); notify.checked = b.notify !== false;
      const rm = h("button", { type: "button", class: "small danger", text: "Remove" });
      const row = h("div", { class: "budget-edit" }, name, host, model, metric, period, limit, notify, rm);
      const r = { row, read: () => ({ id: b.id || "", name: name.value.trim() || "Budget", host: host.value, model: model.value, metric: metric.value, period: period.value, limit: parseTokens(limit.value), notify: notify.checked }) };
      rm.onclick = () => { rows.splice(rows.indexOf(r), 1); row.remove(); };
      rows.push(r);
      list.append(row);
    };
    for (const b of (bud && bud.budgets) || []) addRow(b);
    const add = h("button", { type: "button", class: "small", text: "Add a budget", onclick: () => addRow({ name: rows.length ? "Monthly tokens" : "Daily tokens", period: rows.length ? "month" : "day", limit: rows.length ? 1e9 : 50e6 }) });
    const sync = () => { head.hidden = !rows.length; };
    add.addEventListener("click", sync);
    list.addEventListener("click", () => setTimeout(sync));
    sync();
    sec.append(head, list, h("div", { class: "row-end" }, add));
    const al = (bud && bud.alerts) || { budgets: true, host_down: true, stampede: false };
    const aBud = toggleField("Notify me about budgets", "A macOS notification at 80% and at 100% of any budget with Notify on. Elsewhere it shows in the portal and the log.", al.budgets);
    const aDown = toggleField("Notify me when a host stops answering", "After a minute without an answer, and again when it comes back.", al.host_down);
    const aStamp = toggleField("Notify me about stampedes", "Four or more requests at once, or three times your usual speed. At most once every 10 minutes.", al.stampede);
    sec.append(h("h3", { text: "Alerts" }), aBud.row, aDown.row, aStamp.row);
    saveRow(sec, "Save", async (msg) => {
      const list2 = rows.map((r) => r.read());
      const bad = list2.find((b) => !(b.limit > 0));
      if (bad) throw new Error(bad.name + ": set a limit like 50M or 2.5B.");
      bud = await postJSON("/api/budgets", { budgets: list2, alerts: { budgets: aBud.input.checked, host_down: aDown.input.checked, stampede: aStamp.input.checked } });
      msg.dataset.ok = "1"; msg.textContent = "✓ Saved. " + (list2.length ? list2.length + " budget" + (list2.length === 1 ? "" : "s") + " on the dashboard." : "No budgets.");
      S.dataKey = "";
    });
    return sec;
  }

  // ---------------------------------------------------------------- metrics and API
  // ---------------------------------------------------------------- autopilot
  // What Toktail sends to a roc coding session on your behalf (autopilot.rs).
  function autopilotSection() {
    const sec = section("autopilot", "Autopilot", "Autopilot keeps a coding agent that roc runs working until you stop it (Agents page, or ask Pip). Everything it sends is written here, in your words; it never adds anything of its own.");
    const body = h("div", {}, h("div", { class: "label-sm", text: "Loading…" }));
    sec.append(body);
    getJSON("/api/agents/settings").then((r) => renderAutopilot(body, r)).catch((e) => { body.textContent = "⚠ " + e.message; });
    return sec;
  }
  function areaField(label, hint, value, rows) {
    const t = h("textarea", { class: "wide", rows: String(rows || 5), spellcheck: "true" });
    t.value = value || "";
    return { row: field(label, hint, t, { stack: true }), input: t };
  }
  function renderAutopilot(body, r) {
    const c = r.settings, d = r.defaults;
    body.textContent = "";
    const marks = "{{goal}} becomes the goal you give a run, {{verifier}} the verifier's output, {{turn}} the prompt's number and {{done_claims}} how often the agent has said it's done.";
    const cont = areaField("When the agent stops without saying it's done", "Sent after the session has been quiet for the wait below. " + marks, c.continue_template, 5);
    const done = areaField("When the agent says it's done", "Sent instead when its last reply says the work is finished. Firm and specific works better than harsh: it goes out in your name. Tell it how to reach you: " + r.need_human + " and the reason.", c.done_template, 6);
    const reset = (f, v) => { const b = h("button", { type: "button", class: "small", text: "Use the default" }); b.addEventListener("click", () => { f.input.value = v; }); return b; };
    cont.row.append(h("div", { class: "row-end" }, reset(cont, d.continue_template)));
    done.row.append(h("div", { class: "row-end" }, reset(done, d.done_template)));
    const ver = textField("Verifier command", "Optional. Runs inside the agent's container, in its working directory, before each prompt; its output becomes {{verifier}}. A script that lists the files not yet meeting your skills turns \"not done\" into a work list.", c.verifier_command, "e.g. make audit, or ./scripts/unfinished.sh");
    const verT = numberField("Verifier time limit", "", c.verifier_timeout_secs, "1", "seconds", 5, 3600);
    const verN = numberField("Verifier output kept", "Longer output is cut.", c.verifier_max_chars, "100", "characters", 200, 100000);
    const grace = numberField("Wait before answering", "How long the session must be quiet first.", c.grace_secs, "1", "seconds", 5, 3600);
    const silence = numberField("Silence", "With no new message for this long, autopilot tries again after a prompt that got no answer, and tells you when a busy session looks stuck. Prompt reading on a big local model can take a while.", c.silence_minutes, "1", "minutes", 1, 1440);
    const qWait = numberField("Answer questions for me after", "If you haven't answered an agent's question by then, autopilot sends the default answer below (and marks it). 0 = always wait for you.", c.question_timeout_minutes, "1", "minutes", 0, 10080);
    const qAns = textField("Default answer", "", c.question_default_answer, "");
    const turns = numberField("Prompts per run", "0 = until you stop it.", c.max_turns, "1", "", 0, 100000);
    const ceil = numberField("Token ceiling per run", "Input + output Toktail counts for the session's model while the run lasts (only when roc goes through a Toktail port). 0 = none.", c.token_ceiling, "1000000", "tokens", 0);
    const rep = numberField("Pause after identical replies", "In a row, with no work in between.", c.repeat_limit, "1", "", 2, 100);
    const err = numberField("Stop after failed turns", "In a row (model errors, empty replies); autopilot says what to fix.", c.error_limit, "1", "", 1, 100);
    const dc = numberField("Tell me after “I'm done” replies", "In a row; autopilot keeps going, but you may want to look in. 0 = never.", c.done_claims_alert, "1", "", 0, 1000);
    const state = textField("roc's state file", "Found: " + r.found.roc_state, c.roc_state, "~/.local/roc/state.json");
    const roc = textField("roc command", "For “End the roc session”. Found: " + r.found.roc, c.roc_command, "roc");
    const docker = textField("docker command", "For the verifier. Found: " + r.found.docker, c.docker_command, "docker");
    body.append(h("h3", { text: "What autopilot says" }), cont.row, done.row, ver.row, verT.row, verN.row,
      h("h3", { text: "When" }), grace.row, silence.row, qWait.row, qAns.row,
      h("h3", { text: "Limits" }), turns.row, ceil.row, rep.row, err.row, dc.row,
      h("details", { class: "params" }, h("summary", { text: "Where roc is" }), state.row, roc.row, docker.row));
    saveRow(body, "Save", async (msg) => {
      const n = (i, dflt) => { const v = num(i); return v === undefined ? dflt : Math.round(v); };
      const r2 = await postJSON("/api/agents/settings", {
        roc_state: state.input.value.trim(), roc_command: roc.input.value.trim(), docker_command: docker.input.value.trim(),
        grace_secs: n(grace.input, 60), silence_minutes: n(silence.input, 10),
        continue_template: cont.input.value, done_template: done.input.value,
        verifier_command: ver.input.value.trim(), verifier_timeout_secs: n(verT.input, 600), verifier_max_chars: n(verN.input, 6000),
        max_turns: n(turns.input, 0), token_ceiling: n(ceil.input, 0), repeat_limit: n(rep.input, 3), error_limit: n(err.input, 3),
        done_claims_alert: n(dc.input, 5), question_timeout_minutes: n(qWait.input, 15), question_default_answer: qAns.input.value,
      });
      msg.dataset.ok = "1"; msg.textContent = "✓ Saved";
      setTimeout(() => renderAutopilot(body, r2), 900);
    });
  }

  // ---------------------------------------------------------------- this computer
  // Toktail's own memory and CPU, and how the computer it runs on is doing
  // (hoststatus.rs). Sampled every 2 s, kept an hour, drawn live.
  let hostTimer = null;
  function hostSection() {
    const sec = section("host", "This computer", "How much Toktail itself uses, and how the computer it runs on is doing. Sampled every 2 seconds and kept for an hour, in memory only.");
    const range = h("div", { class: "seg", role: "group", "aria-label": "Range" });
    let minutes = 10;
    for (const [m, t] of [[10, "10 min"], [60, "1 hour"]]) {
      const b = h("button", { type: "button", text: t, "aria-pressed": String(m === minutes) });
      b.addEventListener("click", () => { minutes = m; for (const x of range.children) x.setAttribute("aria-pressed", String(x === b)); tick(); });
      range.append(b);
    }
    const grid = h("div", { class: "hs-grid" });
    const info = h("div", { class: "label-sm hs-info" });
    const disk = h("div", { class: "hs-disk" });
    sec.querySelector(".card-head").append(h("div", { class: "actions" }, range));
    sec.append(grid, info, disk);
    let shown = false;
    async function tick() {
      // Stop once the section has left the page (it starts out not yet on it).
      if (!sec.isConnected) { if (shown) { clearInterval(hostTimer); hostTimer = null; } return; }
      shown = true;
      if (document.hidden) return;
      let r;
      try { r = await getJSON("/api/host-status?minutes=" + minutes); } catch (e) { info.textContent = "⚠ " + e.message; return; }
      drawHost(grid, info, disk, r);
    }
    clearInterval(hostTimer);
    hostTimer = setInterval(tick, 2000);
    setTimeout(tick, 50);
    return sec;
  }
  const bytes = (n) => { const u = ["B", "KB", "MB", "GB", "TB"]; let i = 0, v = n || 0; while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; } return (i ? v.toFixed(v >= 100 ? 0 : 1) : v) + " " + u[i]; };
  function spark(values, max, cls) {
    const W = 260, H = 54, n = values.length;
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("viewBox", `0 0 ${W} ${H}`); svg.setAttribute("class", "hs-spark " + cls); svg.setAttribute("preserveAspectRatio", "none");
    if (n < 2) return svg;
    const top = Math.max(max || 0, ...values, 1e-9);
    const pts = values.map((v, i) => [(i / (n - 1)) * W, H - 2 - (Math.max(0, v) / top) * (H - 6)]);
    const line = pts.map((p) => p[0].toFixed(1) + "," + p[1].toFixed(1)).join(" ");
    const area = document.createElementNS("http://www.w3.org/2000/svg", "polygon");
    area.setAttribute("points", `0,${H} ${line} ${W},${H}`); area.setAttribute("class", "area");
    const pl = document.createElementNS("http://www.w3.org/2000/svg", "polyline");
    pl.setAttribute("points", line); pl.setAttribute("class", "line");
    svg.append(area, pl);
    return svg;
  }
  function tile(title, value, sub, values, max, cls) {
    return h("div", { class: "hs-tile" }, h("div", { class: "label-sm", text: title }), h("div", { class: "hs-value", text: value }), sub ? h("div", { class: "label-sm", text: sub }) : null, spark(values, max, cls));
  }
  function drawHost(grid, info, disk, r) {
    const s = r.series || [], now = r.now;
    grid.textContent = "";
    if (!now) { grid.append(h("div", { class: "label-sm", text: "Measuring…" })); return; }
    const peak = (k) => Math.max(...s.map((x) => x[k] || 0));
    grid.append(
      tile("Toktail's memory", bytes(now.rss), "peak " + bytes(peak("rss")) + " · " + (now.threads ? now.threads + " threads" : ""), s.map((x) => x.rss), 0, "lav"),
      tile("Toktail's CPU", now.cpu.toFixed(1) + "%", "of one core · " + now.in_flight + " request" + (now.in_flight === 1 ? "" : "s") + " in flight", s.map((x) => x.cpu), 100, "lav"),
      tile("Memory in use", bytes(now.mem_used) + " of " + bytes(now.mem_total), Math.round(100 * now.mem_used / Math.max(1, now.mem_total)) + "% · swap " + bytes(now.swap_used) + (now.swap_total ? " of " + bytes(now.swap_total) : ""), s.map((x) => x.mem_used), now.mem_total, "mint"),
      tile("CPU", now.sys_cpu.toFixed(0) + "%", "all " + r.system.cpus + " cores" + (r.system.load ? " · load " + r.system.load.map((x) => x.toFixed(2)).join(" ") : ""), s.map((x) => x.sys_cpu), 100, "mint"),
      now.pressure == null ? null : tile("Memory pressure", pressureWord(now.pressure, r.system.pressure_kind), r.system.pressure_kind, s.map((x) => x.pressure || 0), 100, now.pressure >= 50 ? "bad" : "mint"));
    info.textContent = [r.system.os, r.system.host, "up " + Math.round((r.system.uptime_secs || 0) / 3600) + " h", "Toktail " + r.toktail.version + " (pid " + r.toktail.pid + ")", s.length + " samples"].filter(Boolean).join(" · ");
    disk.textContent = "";
    if (r.disk && r.disk.total != null) {
      const rows = ["history", "records", "cache", "summaries", "market"].filter((k) => r.disk[k]).map((k) => [k, r.disk[k]]);
      disk.append(h("h3", { text: "Disk: " + bytes(r.disk.total) }), h("div", { class: "tbl" }, table([{ h: "What", l: true, v: (x) => ({ history: "Token history (CSV)", records: "Saved prompts and responses", cache: "Prompt cache", summaries: "Pip's summaries", market: "Market prices" })[x[0]] }, { h: "Size", v: (x) => bytes(x[1].bytes) }, { h: "Folder", l: true, v: (x) => h("code", { text: x[1].path }) }], rows)));
    }
  }
  function pressureWord(p, kind) {
    if (kind.startsWith("macOS")) return p >= 100 ? "Critical" : p >= 50 ? "Warning" : "Normal";
    return p.toFixed(1) + "% stalled";
  }

  // ---------------------------------------------------------------- API & MCP
  // Toktail's API (/api/v1, OpenAPI) and MCP server (/mcp) on their own port,
  // with keys (api.rs). Prometheus stays on the portal at /metrics.
  function apiSection() {
    const sec = section("api", "API & MCP", "Let other apps and AI clients use your Toktail: a documented REST API and an MCP server with the same operations, on a port of their own. Open WebUI, Claude Code or your own scripts can ask about your tokens and read your saved prompts from wherever they run. Every request needs a key you issue here; nothing here can change Toktail or drive an agent.");
    const body = h("div", { class: "api-body" }, h("div", { class: "label-sm", text: "Loading…" }));
    sec.append(body);
    const load = () => getJSON("/api/api-settings").then((a) => renderApi(body, a)).catch((e) => { body.textContent = "⚠ " + e.message; });
    load();
    // Prometheus and the portal's sign-in stay here too.
    const base = location.origin;
    const prom = h("div", { class: "api-prom" },
      h("h3", { text: "Prometheus" }),
      field("Metrics", "Scrape this from a Prometheus or Grafana Agent on this computer: tokens, requests and errors per host and model, live tokens/sec, requests in flight, host health and budget use. It answers without signing in and holds counts only.", h("code", { class: "metrics-url", text: base + "/metrics" }), { stack: true }),
      h("details", { class: "params" }, h("summary", { text: "prometheus.yml example" }), h("pre", { text: "scrape_configs:\n  - job_name: toktail\n    static_configs:\n      - targets: ['" + location.host + "']" })));
    const out = h("button", { type: "button", class: "small danger", text: "Sign out every browser" });
    out.addEventListener("click", async () => {
      if (!(await confirmBox("Sign out every browser?", "Every browser signed in to this portal, this one included, has to be opened again from the menu bar (or with Toktail open).", "Sign out", true))) return;
      try { await getJSON("/api/signout", { method: "POST" }); location.reload(); } catch (e) { toast(e.message); }
    });
    sec.append(prom, h("h3", { text: "Portal sign-in" }), h("div", { class: "hint", text: "The portal only opens for browsers you opened from the menu bar or with Toktail open, so programs on this computer (coding agents in containers included) can't read your prompts or change these settings." }), h("div", { class: "row-end" }, out));
    return sec;
  }

  const WHERE = [["local", "On this computer"], ["docker", "In Docker on this computer (Open WebUI's usual setup)"], ["network", "On another machine"]];
  function apiBase(a, where) {
    if (where === "docker") return a.urls.docker;
    if (where === "network") return a.urls.network || ("http://<this computer's address>:" + a.port);
    return a.urls.this_computer;
  }

  function renderApi(body, a) {
    body.textContent = "";
    const status = h("div", { class: "label-sm api-status", text: (a.status.startsWith("listening") ? "✓ " : a.status === "off" ? "" : "⚠ ") + "API and MCP: " + a.status });
    const en = toggleField("Turn on the API and MCP server", "Off closes the port.", a.enabled);
    const port = numberField("Port", "Separate from the portal's. Keys work the same on any port.", a.port, "1", "", 1024, 65535);
    const lan = toggleField("Let other machines connect", "Listens on your network as well as this computer. Each key still says whether it works from the network.", a.allow_network);
    const encrypted = a.encryption_mode !== "off";
    const bodies = toggleField("Let keys with the records scope read whole prompts and responses", encrypted
      ? "Your saved records are encrypted. Off: those keys get the metadata, chain positions and short snippets. On: whole text, decrypted for them. Whatever reads it (Open WebUI keeps its chats in its own database) stores it unencrypted."
      : "Your saved records aren't encrypted, so records keys already get whole text; this matters only if you turn encryption on.", a.decrypted_bodies);
    bodies.input.disabled = !encrypted;
    body.append(status, en.row, port.row, lan.row, bodies.row);
    saveRow(body, "Save", async (msg) => {
      const r = await postJSON("/api/api-settings", { enabled: en.input.checked, port: Math.round(num(port.input) || a.port), allow_network: lan.input.checked, decrypted_bodies: bodies.input.checked });
      msg.dataset.ok = "1"; msg.textContent = "✓ Saved";
      renderApi(body, r);
    });

    // Keys
    const list = h("div", { class: "gw-list api-keys" });
    for (const k of a.keys) list.append(keyRow(body, a, k));
    if (!a.keys.length) list.append(h("div", { class: "label-sm", text: "No keys yet. Connect a client to make one." }));
    const connect = h("button", { type: "button", class: "primary", text: "Connect a client…" });
    connect.addEventListener("click", () => connectClient(body, a));
    body.append(h("h3", { text: "Keys" }), h("div", { class: "hint", text: "read: usage, models, costs, savings, budgets, hosts, what's live. records: saved prompts and responses, and their session chains. Toktail keeps only a hash of each key." }), list, h("div", { class: "row-end" }, connect));

    // What's on offer
    const t = table([{ h: "Tool / endpoint", l: true, v: (o) => h("code", { text: o.name }) }, { h: "REST", l: true, v: (o) => o.path }, { h: "Scope", l: true, v: (o) => o.scope }], a.tools);
    body.append(h("details", { class: "params" }, h("summary", { text: a.tools.length + " operations (MCP tools and REST endpoints)" }),
      h("p", { class: "hint" }, "OpenAPI document: ", h("code", { text: a.urls.this_computer + a.urls.openapi_path }), ". MCP: ", h("code", { text: a.urls.this_computer + a.urls.mcp_path }), "."),
      h("div", { class: "tbl" }, t)));
  }

  function keyRow(body, a, k) {
    const scopes = h("span", { class: "api-scopes" });
    for (const sc of a.scopes) {
      const c = h("input", { type: "checkbox" }); c.checked = k.scopes.includes(sc);
      c.addEventListener("change", async () => {
        const want = a.scopes.filter((x) => x === sc ? c.checked : k.scopes.includes(x));
        try { const r = await postJSON("/api/api-keys/" + encodeURIComponent(k.id), { scopes: want }); renderApi(body, r); } catch (e) { c.checked = !c.checked; toast(e.message); }
      });
      scopes.append(h("label", { class: "inline" }, c, " " + sc));
    }
    const net = h("input", { type: "checkbox" }); net.checked = k.network;
    net.addEventListener("change", async () => {
      try { const r = await postJSON("/api/api-keys/" + encodeURIComponent(k.id), { network: net.checked }); renderApi(body, r); } catch (e) { net.checked = !net.checked; toast(e.message); }
    });
    const rm = h("button", { type: "button", class: "small danger", text: "Revoke" });
    rm.addEventListener("click", async () => {
      if (!(await confirmBox("Revoke " + (k.label || k.id) + "?", "Apps using this key stop working at once.", "Revoke", true))) return;
      try { renderApi(body, await getJSON("/api/api-keys/" + encodeURIComponent(k.id), { method: "DELETE" })); } catch (e) { toast(e.message); }
    });
    const used = k.last_used ? "used " + ago(k.last_used) + (k.last_from ? " from " + k.last_from : "") : "not used yet";
    return h("div", { class: "gw-row api-key" }, h("code", { text: k.id }), span(k.label || "", ""), scopes, h("label", { class: "inline" }, net, " from the network"), span(used, "label-sm"), rm);
  }

  async function connectClient(body, a) {
    const label = h("input", { type: "text", class: "wide", value: "Open WebUI", placeholder: "What it's for" });
    const where = h("select"); for (const [v, t] of WHERE) where.append(h("option", { value: v, text: t }));
    const read = h("input", { type: "checkbox" }); read.checked = true;
    const recs = h("input", { type: "checkbox" }); recs.checked = true;
    const form = h("div", { class: "connect-form" },
      field("Name", "Shown next to the key here.", label, { stack: true }),
      field("Where does it run?", "Decides the address to use, and whether the key works from the network.", where, { stack: true }),
      h("div", { class: "row" }, h("span", { class: "row-label", text: "It may see" }), h("span", {}, h("label", { class: "inline" }, read, " read (counts)"), " ", h("label", { class: "inline" }, recs, " records (saved prompts)"))));
    const ok = await modal("Connect a client", form, [{ label: "Cancel", value: false }, { label: "Make a key", value: true, cls: "primary" }]);
    if (!ok) return;
    const w = where.value;
    const scopes = [read.checked ? "read" : null, recs.checked ? "records" : null].filter(Boolean);
    let r;
    try {
      if (w === "network" && !a.allow_network) a = await postJSON("/api/api-settings", { allow_network: true });
      r = await postJSON("/api/api-keys", { label: label.value, scopes, network: w === "network" });
    } catch (e) { return toast(e.message); }
    renderApi(body, r.settings);
    const base = apiBase(r.settings, w);
    const mcp = base + "/mcp", spec = base + "/api/v1/openapi.json";
    const copyRow = (text, what) => { const b = h("button", { type: "button", class: "small", text: "Copy" }); b.addEventListener("click", () => navigator.clipboard && navigator.clipboard.writeText(text).then(() => toast(what + " copied"))); return h("div", { class: "with-btn" }, h("code", { class: "gw-key", text: text }), b); };
    const claude = "claude mcp add --transport http toktail " + mcp + ' --header "Authorization: Bearer ' + r.key + '"';
    const steps = h("div", { class: "connect-done" },
      h("p", { class: "label-sm", text: "Copy the key now. Toktail keeps only a hash and can't show it again." }),
      copyRow(r.key, "Key"),
      h("h4", { text: "Open WebUI (0.6.31 or later, as an admin)" }),
      h("ol", {}, h("li", { text: "Settings → Admin → Integrations → External Tool Servers → + Add Connection." }), h("li", { text: "Type: MCP (Streamable HTTP)." }), h("li", {}, "Server URL: ", copyRow(mcp, "URL")), h("li", { text: "Auth: Bearer, and paste the key into Key. Save." })),
      h("h4", { text: "Anything that reads OpenAPI" }), copyRow(spec, "OpenAPI URL"),
      h("h4", { text: "Claude Code" }), copyRow(claude, "Command"),
      w === "network" && !r.settings.urls.network ? h("p", { class: "label-sm warn-note", text: "Toktail couldn't find this computer's network address; use its name or IP in place of the placeholder." }) : null,
      w === "docker" ? h("p", { class: "hint", text: "host.docker.internal reaches this computer from Docker Desktop. On Linux Docker, start the container with --add-host=host.docker.internal:host-gateway." }) : null);
    await modal("Your key for " + (label.value || "the client"), steps, [{ label: "Done", value: true, cls: "primary" }]);
  }

  // ---------------------------------------------------------------- hardware
  function hardwareSection() {
    const sec = section("hardware", "Hardware", "The machines themselves (serial numbers, prices, purchase dates) are on the Savings page.");
    const life = numberField("Default lifecycle", "How long you expect to keep a machine. 72 months (6 years) is a common plan; some keep theirs 96 or 144 months.", st.hardware.default_lifecycle_months, "1", "months", 1, 600);
    sec.append(life.row, h("p", { class: "label-sm" }, h("a", { href: "#/savings", text: "Manage your machines on the Savings page →" })));
    saveRow(sec, "Save", (msg) => post({ hardware: { default_lifecycle_months: Math.round(num(life.input) || 72) } }, msg));
    return sec;
  }

  return { show };
})();


