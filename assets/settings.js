// SPDX-License-Identifier: Apache-2.0
// Toktail portal: the Settings page. Hosts, saving prompts and responses,
// encryption and key stores, S3, Pip and the menu bar, cost comparison and
// hardware. Each section saves on its own.
"use strict";

const Settings = (() => {
  let st = null;
  const SECTIONS = [
    ["hosts", "Hosts"], ["capture", "Saving prompts"], ["encryption", "Encryption"], ["s3", "S3 mirror"],
    ["budgets", "Budgets & alerts"], ["pip", "Pip & menu bar"], ["costs", "Cost comparison"], ["hardware", "Hardware"], ["api", "Metrics & API"],
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
    body.append(hostsSection(), captureSection(), encryptionSection(), s3Section(), budgetsSection(), pipSection(), costsSection(), hardwareSection(), apiSection());
    body.append(h("p", { class: "label-sm" }, "Settings live in " + st.config_file + ". Secrets are in your " + (st.secret_store || "Keychain") + ". Your CSV is in " + st.data_dir + ". ", h("button", { type: "button", class: "small", text: "Open the folder", onclick: () => getJSON("/api/open-data-folder", { method: "POST" }).catch((e) => toast(e.message)) })));
    if ((typeof Stage !== "undefined")) Stage.platformsChanged();
  }

  // ---------------------------------------------------------------- hosts
  function hostsSection() {
    const sec = section("hosts", "Hosts", "The LM Studio servers Toktail watches, up to " + st.max_hosts + ". Each one gets its own Toktail port: point an app at that port and its tokens are counted for that host.");
    const list = h("div", { class: "host-list" });
    for (const x of st.hosts) list.append(hostRow(x));
    sec.append(list);
    const add = h("button", { type: "button", class: "small", text: "Add a host", disabled: st.hosts.length >= st.max_hosts });
    add.addEventListener("click", () => {
      const used = new Set(st.hosts.map((x) => x.listen_port));
      let port = 1240; while (used.has(port) || port === st.portal_port) port++;
      list.append(hostRow({ id: "", name: "", url: "http://", listen_port: port, allow_lan: false, enabled: true, has_token: false }, true));
      add.disabled = true;
    });
    sec.append(h("div", { class: "row-end" }, span(st.hosts.length + " of " + st.max_hosts + " hosts", "label-sm"), add));
    return sec;
  }
  function hostRow(x, isNew) {
    const ok = x.enabled && x.proxy_ok && x.ok;
    const row = h("div", { class: "host-row" + (isNew ? " open" : "") });
    const summary = h("button", { type: "button", class: "host-sum" },
      h("span", { class: "ht-dot " + (isNew ? "" : !x.enabled ? "" : ok ? "ok" : "bad") }),
      h("b", { text: x.name || "New host" }),
      isNew ? null : span(x.url.replace(/^https?:\/\//, "") + " → localhost:" + x.listen_port, "label-sm"),
      isNew ? null : span(!x.enabled ? "turned off" : ok ? "✓ " + (x.models || []).length + " models" : "⚠ " + (x.proxy_ok ? x.message : x.proxy_message), "label-sm host-state"));
    const form = h("div", { class: "host-form" });
    const name = textField("Name", "What the dashboard calls it", x.name, "Mac Studio");
    const url = textField("LM Studio's address", "Its server address and port, like http://127.0.0.1:1235 or http://studio.local:1234", x.url, "http://127.0.0.1:1235");
    const tok = secretField("API token", x.has_token ? "Saved in your " + vault() + ". Type a new one to replace it." : "Only if this LM Studio asks for one. " + "Kept in your " + vault() + ", never in a file.", { saved: x.has_token, hint: x.token_hint });
    const clear = x.has_token ? toggleField("Remove the saved token", null, false) : null;
    const port = numberField("Toktail listens on port", "Apps call this port instead of LM Studio's.", x.listen_port, "1", "", 1, 65535);
    const lan = toggleField("Let other devices on your network use it", "Off means only apps on this Mac can reach this port.", x.allow_lan);
    const en = toggleField("Watch this host", "Off stops the proxy for this host; its history stays.", x.enabled);
    const hint = h("div", { class: "label-sm client-hint" });
    const showHint = () => { hint.textContent = "Apps use http://" + (lan.input.checked ? "<this Mac's name or IP>" : "localhost") + ":" + (port.input.value || x.listen_port) + "/v1"; };
    showHint(); port.input.addEventListener("input", showHint); lan.input.addEventListener("change", showHint);
    form.append(name.row, url.row, tok.row);
    if (clear) form.append(clear.row);
    form.append(port.row, lan.row, en.row, hint);
    const testMsg = h("span", { class: "test-msg" });
    if (!isNew && x.checked) { testMsg.textContent = (x.ok ? "✓ " : "⚠ ") + x.message; testMsg.dataset.ok = x.ok ? "1" : "0"; }
    const test = h("button", { type: "button", class: "small", text: "Test connection" });
    test.addEventListener("click", async () => {
      test.disabled = true; testMsg.textContent = "Knocking on LM Studio's door…"; testMsg.dataset.ok = "";
      try {
        const r = await postJSON("/api/hosts/test", { url: url.input.value, token: tok.input.value, id: x.id || null });
        testMsg.dataset.ok = r.ok ? "1" : "0";
        testMsg.textContent = (r.ok ? "✓ " : "⚠ ") + r.message + (r.ok && r.models.length ? ": " + r.models.slice(0, 4).join(", ") + (r.models.length > 4 ? "…" : "") : "") + (r.ok ? " (" + r.ms + " ms)" : "");
      } catch (e) { testMsg.dataset.ok = "0"; testMsg.textContent = "⚠ " + e.message; }
      test.disabled = false;
    });
    const save = h("button", { type: "button", class: "primary small", text: isNew ? "Add host" : "Save host" });
    save.addEventListener("click", async () => {
      save.disabled = true;
      try {
        const r = await postJSON("/api/hosts", { id: x.id || null, name: name.input.value, url: url.input.value, listen_port: parseInt(port.input.value, 10) || 0, allow_lan: lan.input.checked, enabled: en.input.checked, token: tok.input.value || null, clear_token: clear ? clear.input.checked : false });
        st = r.settings; S.settings = st;
        toast(isNew ? "Added. Pip is watching " + name.input.value + "." : "Saved");
        render();
        if ((typeof Main !== "undefined")) Main.refreshMeta();
      } catch (e) { testMsg.dataset.ok = "0"; testMsg.textContent = "⚠ " + e.message; save.disabled = false; }
    });
    const del = isNew ? h("button", { type: "button", class: "small", text: "Cancel", onclick: () => render() })
      : h("button", { type: "button", class: "small danger", text: "Remove", disabled: st.hosts.length <= 1, title: st.hosts.length <= 1 ? "Toktail needs at least one host" : "" });
    if (!isNew) del.addEventListener("click", async () => {
      if (!(await confirmBox("Remove " + x.name + "?", "Toktail stops listening on port " + x.listen_port + " and forgets this host's API token. Its token history stays in your CSV and on the dashboard.", "Remove", true))) return;
      try { st = await getJSON("/api/hosts/" + encodeURIComponent(x.id), { method: "DELETE" }); S.settings = st; render(); if ((typeof Main !== "undefined")) Main.refreshMeta(); } catch (e) { toast(e.message); }
    });
    form.append(h("div", { class: "row-end" }, testMsg, test, del, save));
    summary.addEventListener("click", () => row.classList.toggle("open"));
    row.append(summary, form);
    return row;
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
    sec.append(intro.row, roams.row, anim.row, count.row, stamp.row);
    saveRow(sec, "Save", (msg) => post({ portal: { intro_animation: intro.input.checked, pip_roams: roams.input.checked }, app: { animate_menu_bar: anim.input.checked, show_count_in_menu_bar: count.input.checked, stampede_tokens_per_sec: num(stamp.input) } }, msg).then(() => { if ((typeof Stage !== "undefined")) Stage.applySettings(); }));
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
    sec.append(show.row, base.row, kwh.row, watts.row);
    const pick = h("div", { class: "models" });
    for (const m of st.prices.models) {
      const i = h("input", { type: "checkbox", value: m.id, class: "set-compare" });
      i.checked = c.compare.includes(m.id);
      pick.append(h("label", {}, i, h("span", {}, m.provider + " " + m.name.replace(m.provider + " ", ""), h("small", { text: "$" + m.input + " in, $" + m.output + " out per 1M" + (m.per_request ? ", plus $" + m.per_request + " a request" : "") }))));
    }
    sec.append(h("div", { class: "label-sm", style: "margin-top:10px", text: "Compare against:" }), pick);
    sec.append(h("p", { class: "label-sm", text: "To update prices, put your own pricing.json next to " + st.config_file + "." }));
    saveRow(sec, "Save", (msg) => post({ costs: { show_comparison: show.input.checked, baseline: base.input.value, electricity_usd_per_kwh: num(kwh.input), watts_while_generating: num(watts.input), compare: [...sec.querySelectorAll(".set-compare:checked")].map((i) => i.value) } }, msg));
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
  function apiSection() {
    const sec = section("api", "Metrics and API", "Everything the portal shows comes from a local JSON API, and Toktail also speaks Prometheus. Both answer only on this computer.");
    const base = location.origin;
    sec.append(field("Prometheus", "Scrape this from a Prometheus or Grafana Agent running on this computer: tokens, requests and errors per host and model, live tokens/sec, requests in flight, host health and budget use.", h("code", { class: "metrics-url", text: base + "/metrics" }), { stack: true }));
    const scrape = "scrape_configs:\n  - job_name: toktail\n    static_configs:\n      - targets: ['" + location.host + "']";
    sec.append(h("details", { class: "params" }, h("summary", { text: "prometheus.yml example" }), h("pre", { text: scrape })));
    const eps = [["/api/meta", "hosts, models, totals and status"], ["/api/daily?from=&to=&hosts=&models=", "tokens per day, host and model"], ["/api/live", "live tokens/sec, mood, requests in flight and just finished"], ["/api/models/stats", "per-model speed, wait, errors and cloud value"], ["/api/budgets", "budgets with use and projection"], ["/api/pip/usage?period=last%20month&host=studio", "exact totals, the same numbers Pip uses"], ["/api/pip/facts?q=…", "what Pip would look up for a question"], ["/api/export.csv", "the raw CSV for a range"]];
    const t = table([{ h: "Endpoint", l: true, v: (r) => h("a", { href: r[0], target: "_blank", rel: "noopener", text: r[0] }) }, { h: "What", l: true, v: (r) => r[1] }], eps);
    sec.append(h("h3", { text: "JSON endpoints" }), h("div", { class: "tbl" }, t));
    return sec;
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

