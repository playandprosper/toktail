// SPDX-License-Identifier: Apache-2.0
// Toktail portal: the Agents page. The coding sessions roc runs on this
// computer, autopilot on each (autopilot.rs), the agent's questions and
// your answers, and a banner on every page while autopilot is driving.
//   #/agents        every session
//   #/agents/<id>   one session: autopilot, questions, messages, history
"use strict";

const Agents = (() => {
  let current = null;
  let timer = null;
  let busy = false;

  const STATE = {
    running: ["Autopilot on", "ok"],
    stopping: ["Stopping after this turn", "warn"],
    paused: ["Paused", "warn"],
    stopped: ["Stopped", ""],
  };
  const stateBadge = (a) => {
    if (!a) return span("Autopilot off", "ap-badge");
    const [t, c] = STATE[a.state] || [a.state, ""];
    return span(t, "ap-badge " + c);
  };
  const when = (iso) => (iso ? ago(iso) : "—");

  function init() {
    setInterval(pollBanner, 15000);
    pollBanner();
  }

  async function show(sub) {
    current = sub || null;
    clearInterval(timer);
    await render();
    timer = setInterval(() => { if (!document.hidden && S.page === "agents" && !busy && !typing()) render(); }, current ? 5000 : 10000);
  }

  /** Something you're in the middle of (an answer, a message) that a refresh would wipe. */
  function typing() {
    const root = $("agents-body");
    if (!root) return false;
    const a = document.activeElement;
    if (a && root.contains(a) && /^(INPUT|TEXTAREA|SELECT)$/.test(a.tagName)) return true;
    for (const t of root.querySelectorAll("textarea, input[type=text]")) if (t.value.trim() && !t.dataset.prefilled) return true;
    return !!root.querySelector(".q-box input:checked");
  }

  async function render() {
    const root = $("agents-body");
    if (!root) return;
    try {
      if (current) await renderDetail(root, current);
      else await renderList(root);
    } catch (e) {
      root.textContent = "";
      root.append(h("div", { class: "banner warn", text: "⚠ " + e.message }));
    }
  }

  // ---------------------------------------------------------------- list
  async function renderList(root) {
    const o = await getJSON("/api/agents");
    $("agents-sub").textContent = "The coding agents roc runs on this computer. Turn on autopilot and Toktail keeps one working, in your words, until you stop it.";
    const frag = document.createDocumentFragment();
    if (o.problem) frag.append(h("div", { class: "banner warn" }, h("b", { text: "Can't see roc's sessions. " }), o.problem, " ", h("a", { href: "#/settings/autopilot", text: "Autopilot settings" })));
    else if (!o.sessions.length) frag.append(h("div", { class: "card empty-card" }, h("p", { text: "roc isn't running any sessions right now." }), h("p", { class: "label-sm", text: "Start one with roc in a terminal (for example: cd ~/friends_of/planning && roc). opencode sessions from roc 0.3 or later can be driven from here." })));
    const grid = h("div", { class: "agents-grid" });
    for (const s of o.sessions) grid.append(sessionCard(s));
    frag.append(grid);
    if (o.ended && o.ended.length) {
      frag.append(h("h3", { class: "agents-h3", text: "Earlier runs" }));
      const g2 = h("div", { class: "agents-grid" });
      for (const s of o.ended) g2.append(sessionCard({ ...s, ended: true }));
      frag.append(g2);
    }
    root.textContent = "";
    root.append(frag);
  }

  function sessionCard(s) {
    const a = s.autopilot;
    const need = a && (a.need_human || a.pending_questions > 0 || a.state === "paused");
    const c = h("a", { class: "card agent-card" + (need ? " needs" : "") + (s.ended ? " ended" : ""), href: "#/agents/" + s.id },
      h("div", { class: "agent-top" }, h("span", { class: "agent-label", text: s.label || s.id }), stateBadge(a)),
      h("div", { class: "agent-project", text: s.project || "" }),
      h("div", { class: "label-sm", text: [s.binary, s.model, s.toktail_host ? "counted on " + s.toktail_host : null].filter(Boolean).join(" · ") }));
    if (a) {
      c.append(h("div", { class: "agent-stats" },
        span(fmt(a.turns) + " prompt" + (a.turns === 1 ? "" : "s") + " sent", ""),
        a.done_claims ? span(fmt(a.done_claims) + " “I'm done”", "") : null,
        a.last_prompt_at ? span("last " + when(a.last_prompt_at), "") : null));
      if (a.pending_questions) c.append(h("div", { class: "agent-need", text: "❓ " + a.pending_questions + " question" + (a.pending_questions === 1 ? "" : "s") + " waiting for you" }));
      if (a.need_human) c.append(h("div", { class: "agent-need", text: "✋ Needs you: " + a.need_human }));
      if (a.state === "paused" || a.state === "stopped") if (a.reason) c.append(h("div", { class: "label-sm", text: a.reason }));
    } else if (!s.ended && !s.controllable) {
      c.append(h("div", { class: "label-sm", text: s.binary === "opencode" ? "No remote control (needs roc 0.3+ and opencode's TUI)." : "Autopilot drives opencode sessions only." }));
    }
    return c;
  }

  // ---------------------------------------------------------------- one session
  async function renderDetail(root, id) {
    const d = await getJSON("/api/agents/" + encodeURIComponent(id));
    const s = d.session, a = d.autopilot, live = d.live;
    const name = s ? s.label : a ? a.label : id;
    $("agents-sub").textContent = "";
    const frag = document.createDocumentFragment();
    frag.append(h("p", {}, h("a", { href: "#/agents", text: "← All agents" })));
    const head = h("div", { class: "agent-head" },
      h("h2", { text: name + (s && s.project ? " · " + s.project : a && a.project ? " · " + a.project : "") }),
      stateBadge(a));
    frag.append(head);
    if (s) frag.append(h("div", { class: "label-sm agent-meta", text: [s.binary, s.model, s.workdir, s.toktail_host ? "tokens counted on " + s.toktail_host : "tokens not counted by Toktail (roc talks to the model server directly)", "started " + when(s.started_at)].filter(Boolean).join(" · ") }));
    else frag.append(h("div", { class: "banner", text: "This roc session has ended. Its autopilot history is below." }));

    const cols = h("div", { class: "agent-cols" });
    const left = h("div", { class: "agent-col" }), right = h("div", { class: "agent-col" });
    cols.append(left, right);
    frag.append(cols);

    // Needs you
    if (a && a.need_human) {
      const ack = h("button", { type: "button", class: "primary small", text: "I've seen it" });
      ack.addEventListener("click", () => act(id, "acknowledge"));
      left.append(h("div", { class: "banner warn" }, h("b", { text: "The agent asks for you: " }), a.need_human, " ", ack));
    }

    // Autopilot
    left.append(autopilotCard(id, s, a));

    // Questions
    const qs = (live && live.questions) || [];
    if (qs.length || (a && a.questions && a.questions.length)) left.append(questionsCard(id, qs, a));

    // Live conversation
    if (live) right.append(liveCard(id, live));
    // History
    if (a && a.history && a.history.length) right.append(historyCard(a));

    // Keep the conversation where you were reading (at the bottom, unless you scrolled up).
    const old = root.querySelector(".agent-msgs");
    const stick = !old || old.scrollTop + old.clientHeight >= old.scrollHeight - 30;
    const keep = old ? old.scrollTop : 0;
    root.textContent = "";
    root.append(frag);
    const box = root.querySelector(".agent-msgs");
    if (box) box.scrollTop = stick ? box.scrollHeight : keep;
  }

  function autopilotCard(id, s, a) {
    const card = h("section", { class: "card" }, h("div", { class: "card-head" }, h("div", {}, h("h2", { text: "Autopilot" }), h("div", { class: "sub", text: "When the agent goes quiet, Toktail sends your words from Settings → Autopilot: a firm “it isn't finished” when it says it's done, “continue” otherwise. It stops when you stop it." }))));
    const can = s && s.controllable;
    if (a) {
      const facts = h("div", { class: "agent-facts" },
        fact("Prompts sent", fmt(a.turns)),
        fact("“I'm done” replies", fmt(a.done_claims) + (a.done_in_row > 1 ? " (" + a.done_in_row + " in a row)" : "")),
        fact("Started", when(a.started_at) + " by " + (a.started_by === "pip" ? "Pip" : "you")),
        fact("Last prompt", when(a.last_prompt_at)));
      card.append(facts);
      if (a.reason) card.append(h("div", { class: "label-sm ap-reason", text: a.reason }));
      if (a.goal) card.append(h("div", { class: "label-sm" }, h("b", { text: "Goal: " }), a.goal));
    }
    const running = a && (a.state === "running" || a.state === "stopping");
    const row = h("div", { class: "row-end ap-actions" });
    if (can && !running) {
      const goal = h("textarea", { class: "wide", rows: 2, placeholder: "Optional: what you want done, added to every prompt (e.g. apply go-engineering-standards and go-documentation-discipline to every file under internal/)" });
      if (a && a.goal) { goal.value = a.goal; goal.dataset.prefilled = "1"; }
      goal.addEventListener("input", () => { delete goal.dataset.prefilled; });
      card.append(field("Goal", "", goal, { stack: true }));
      const start = h("button", { type: "button", class: "primary", text: a && a.state === "paused" ? "Resume autopilot" : "Start autopilot" });
      start.addEventListener("click", () => act(id, "start", { goal: goal.value }));
      row.append(start);
    }
    if (running) {
      const stop = h("button", { type: "button", class: "", text: "Stop after this turn" });
      stop.addEventListener("click", () => act(id, "stop"));
      const now = h("button", { type: "button", class: "danger", text: "Stop now" });
      now.addEventListener("click", async () => { if (await confirmBox("Stop now?", "The turn in progress is aborted, which can leave an edit half-done. Stop after this turn waits for it to finish.", "Stop now", true)) act(id, "stop_now"); });
      row.append(stop, now);
    }
    if (s) {
      const kill = h("button", { type: "button", class: "small danger", text: "End the roc session" });
      kill.addEventListener("click", async () => { if (await confirmBox("End " + (s.label || "this session") + "?", "Runs roc -kill: the agent's container stops, roc removes everything it created and frees the worker. Unsaved work in the container is lost.", "End it", true)) act(id, "kill"); });
      row.append(kill);
    }
    if (!can && s) card.append(h("div", { class: "label-sm", text: s.binary === "opencode" ? "This session has no remote control. Start opencode sessions with roc 0.3 or later (its TUI, not opencode run) to drive them from here." : "Autopilot drives opencode sessions; this one runs " + s.binary + "." }));
    card.append(row);
    return card;
  }
  const fact = (k, v) => h("div", { class: "fact" }, span(k, "label-sm"), h("b", { text: v }));

  async function act(id, action, extra) {
    busy = true;
    try {
      await postJSON("/api/agents/" + encodeURIComponent(id) + "/autopilot", { action, ...(extra || {}) });
      toast({ start: "Autopilot is on. Pip keeps it going until you stop it.", stop: "Autopilot stops after this turn.", stop_now: "Autopilot stopped.", kill: "The roc session is ending.", acknowledge: "Noted." }[action] || "Done");
      if (action === "start" && typeof Stage !== "undefined") Stage.cheer("On it! I'll keep them going until you say stop.");
    } catch (e) { toast(e.message); }
    busy = false;
    render(); pollBanner();
  }

  function questionsCard(id, live, a) {
    const card = h("section", { class: "card" }, h("div", { class: "card-head" }, h("div", {}, h("h2", { text: "Questions" }), h("div", { class: "sub", text: "The agent waits for these. Nothing else is sent until they're answered." }))));
    for (const q of live) card.append(questionForm(id, q));
    const past = ((a && a.questions) || []).filter((q) => q.answered_at).slice(-8).reverse();
    if (past.length) {
      const list = h("div", { class: "q-past" });
      for (const q of past) list.append(h("div", { class: "q-past-row" }, h("div", { text: questionText(q.questions) }), h("div", { class: "label-sm", text: "→ " + (q.answers ? q.answers.map((x) => x.join(", ")).join(" / ") : "answered") + " · " + ({ you: "you", auto: "autopilot's default answer", tui: "in the TUI" }[q.by] || q.by) + " · " + when(q.answered_at) })));
      card.append(h("details", { class: "params" }, h("summary", { text: "Answered (" + past.length + ")" }), list));
    }
    return card;
  }
  const questionText = (qs) => (qs || []).map((q) => q.question).join(" · ");

  function questionForm(id, req) {
    const box = h("div", { class: "q-box" });
    const pickers = [];
    (req.questions || []).forEach((q, i) => {
      const multi = !!q.multiple;
      const group = h("div", { class: "q-opts" });
      const name = "q-" + req.id + "-" + i;
      for (const o of q.options || []) {
        const inp = h("input", { type: multi ? "checkbox" : "radio", name, value: o.label });
        group.append(h("label", { class: "q-opt" }, inp, h("span", {}, h("b", { text: o.label }), o.description ? span(" — " + o.description, "label-sm") : null)));
      }
      const custom = q.custom === false ? null : h("input", { type: "text", class: "wide", placeholder: "Or type your own answer" });
      box.append(h("div", { class: "q-head", text: q.header || "" }), h("p", { class: "q-text", text: q.question }), group, custom);
      pickers.push(() => {
        const typed = custom && custom.value.trim();
        if (typed) return [typed];
        return [...group.querySelectorAll("input:checked")].map((x) => x.value);
      });
    });
    const send = h("button", { type: "button", class: "primary small", text: "Answer" });
    send.addEventListener("click", async () => {
      const answers = pickers.map((f) => f());
      if (answers.some((x) => !x.length)) return toast("Answer every question first.");
      send.disabled = true; busy = true;
      try { await postJSON("/api/agents/" + encodeURIComponent(id) + "/answer", { question: req.id, answers }); toast("Answer sent."); } catch (e) { toast(e.message); }
      busy = false; render();
    });
    box.append(h("div", { class: "row-end" }, send));
    return box;
  }

  function liveCard(id, live) {
    const card = h("section", { class: "card" }, h("div", { class: "card-head" }, h("div", {}, h("h2", { text: "The conversation" }), h("div", { class: "sub", text: live.error ? "⚠ " + live.error : "opencode is " + live.status + "." }))));
    if (live.error) return card;
    const todos = Array.isArray(live.todos) ? live.todos : [];
    if (todos.length) {
      const done = todos.filter((t) => t.status === "completed").length;
      const list = h("ul", { class: "todo-list" });
      for (const t of todos) list.append(h("li", { class: "todo " + (t.status || "") }, span({ completed: "✓", in_progress: "▸", cancelled: "✕" }[t.status] || "○", "todo-mark"), span(t.content || "", "")));
      card.append(h("details", { class: "params", open: true }, h("summary", { text: "The agent's todo list: " + done + " of " + todos.length + " done" }), list));
    }
    const msgs = h("div", { class: "agent-msgs" });
    for (const m of live.messages || []) {
      if (!m.text && !(m.tools || []).length && !m.error) continue;
      const tools = (m.tools || []).length ? span("used " + [...new Set(m.tools)].join(", "), "label-sm") : null;
      msgs.append(h("div", { class: "agent-msg " + m.role },
        h("div", { class: "agent-msg-head" }, span(m.role === "user" ? "Prompt" : "Agent", "who"), m.claims_done ? span("says it's done", "ap-badge warn") : null, tools, span(m.created ? ago(new Date(m.created).toISOString()) : "", "label-sm")),
        m.error ? h("div", { class: "agent-err", text: "⚠ " + m.error }) : null,
        m.text ? h("div", { class: "agent-text", text: m.text }) : null));
    }
    card.append(msgs);
    const text = h("textarea", { class: "wide", rows: 2, placeholder: "Write to the agent yourself (sent as you, like typing in the TUI)" });
    const send = h("button", { type: "button", class: "small", text: "Send" });
    send.addEventListener("click", async () => {
      if (!text.value.trim()) return;
      send.disabled = true; busy = true;
      try { await postJSON("/api/agents/" + encodeURIComponent(id) + "/message", { text: text.value }); text.value = ""; toast("Sent."); } catch (e) { toast(e.message); }
      busy = false; send.disabled = false; render();
    });
    card.append(h("div", { class: "with-btn agent-send" }, text, send));
    return card;
  }

  const KIND = { prompt: "Sent", reply: "Agent", question: "Asked", answer: "Answered", note: "Note", pause: "Paused", stop: "Stopped", error: "Problem", verifier: "Verifier" };
  function historyCard(a) {
    const card = h("section", { class: "card" }, h("div", { class: "card-head" }, h("div", {}, h("h2", { text: "Autopilot's log" }), h("div", { class: "sub", text: "Everything autopilot sent and saw, newest first." }))));
    const list = h("div", { class: "ap-log" });
    for (const e of a.history.slice().reverse().slice(0, 120)) {
      const long = e.text.length > 280;
      const body = h("div", { class: "ap-log-text", text: long ? e.text.slice(0, 280) + "…" : e.text });
      if (long) { body.classList.add("clickable"); body.title = "Show all"; body.addEventListener("click", () => { body.textContent = e.text; body.classList.remove("clickable"); }); }
      list.append(h("div", { class: "ap-log-row " + e.kind }, h("div", { class: "ap-log-head" }, span(KIND[e.kind] || e.kind, "ap-kind"), span(ago(e.at), "label-sm")), body));
    }
    card.append(list);
    return card;
  }

  // ---------------------------------------------------------------- banner on every page
  async function pollBanner() {
    const b = $("agents-banner");
    if (!b || document.hidden) return;
    let o;
    try { o = await getJSON("/api/agents"); } catch (e) { return; }
    const runs = (o.sessions || []).filter((s) => s.autopilot && ["running", "stopping", "paused"].includes(s.autopilot.state));
    if (!runs.length) { b.hidden = true; return; }
    const need = runs.filter((s) => s.autopilot.need_human || s.autopilot.pending_questions || s.autopilot.state === "paused");
    b.hidden = false;
    b.className = "agents-banner" + (need.length ? " needs" : "");
    b.textContent = "";
    const names = runs.map((s) => s.label + (s.project ? " (" + s.project + ")" : "")).join(", ");
    b.append(span(need.length ? "✋" : "🦊", "ab-icon"),
      h("span", {}, h("b", { text: "Autopilot " + (runs.length === 1 ? "is keeping " : "is keeping these going: ") + names + (runs.length === 1 ? " going." : ".") }),
        need.length ? " " + need.map((s) => s.label + ": " + (s.autopilot.pending_questions ? s.autopilot.pending_questions + " question(s) waiting" : s.autopilot.need_human ? "asks for you" : "paused (" + (s.autopilot.reason || "needs a look") + ")")).join("; ") + "." : " It runs until you stop it; look in now and then."),
      h("a", { class: "btn small", href: need.length ? "#/agents/" + need[0].id : runs.length === 1 ? "#/agents/" + runs[0].id : "#/agents", text: need.length ? "Answer" : "Watch" }));
    const prev = Number(b.dataset.need || 0);
    if (need.length > prev && typeof Stage !== "undefined") Stage.cheer("Psst! " + need[0].label + " needs you.");
    b.dataset.need = String(need.length);
  }

  return { init, show, pollBanner };
})();
