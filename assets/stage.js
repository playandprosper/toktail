// SPDX-License-Identifier: Apache-2.0
// Toktail portal: Pip's stage. The intro, the trail along the bottom, the
// live pulse, the chart replay, and Pip's platforming: Pip jumps up onto the
// card you're looking at, walks along its top edge, and hops from card to
// card through the page, unhurried.
"use strict";

const Stage = (() => {
  let pip = null, phase = "loading", state = "idle", liveTimer = 0, lastSparkle = 0, busy = false;
  /** Where Pip is: on the ground (the trail) or standing on a card. */
  let spot = { kind: "ground" };
  /** The card whose movement Pip follows (standing on it, or leaping to it). */
  let anchor = null, anchorRect = null;
  const pointer = { x: -1, y: -1, t: 0, card: null, since: 0 };
  let nextMoveAt = 0;

  const groundY = () => innerHeight - 22;
  const trailSize = () => (innerWidth < 600 ? 96 : 124);
  const cardSize = () => (innerWidth < 600 ? 58 : 74);
  const TRAIL_H = 96;
  const LINES = {
    idle: () => "Nothing to count. Nap time!",
    processing: (L) => "Nom nom nom… " + fmt(L.tokens_per_sec) + " tokens/sec!",
    stampede: () => "Holy tokens! We've got a stampede!",
  };
  const roams = () => (S.meta && S.meta.portal ? S.meta.portal.pip_roams : true);
  const rand = (a, b) => a + Math.random() * (b - a);

  // ================================================================ platforms
  /** Cards Pip can stand on: visible, wide enough, top edge on screen. */
  function platforms() {
    const out = [];
    for (const c of document.querySelectorAll("main.page:not([hidden]) .card")) {
      if (!c.offsetParent || c.closest("dialog") || c.closest("[hidden]")) continue;
      const r = c.getBoundingClientRect();
      if (r.width < 150 || r.height < 40) continue;
      out.push({ el: c, r });
    }
    return out;
  }
  const standable = (r) => r.top > 130 && r.top < innerHeight - TRAIL_H - 30 && r.right > 40 && r.left < innerWidth - 40;
  const edgeRange = (r, size) => [r.left + size * 0.62 + 6, r.right - size * 0.62 - 6];
  function cardAt(x, y) {
    let best = null, bd = Infinity;
    for (const p of platforms()) {
      const r = p.r;
      if (x < r.left || x > r.right) continue;
      // Inside the card, or just above its top edge.
      const d = y >= r.top && y <= r.bottom ? 0 : y < r.top && r.top - y < 70 ? r.top - y : Infinity;
      if (d < bd) { bd = d; best = p.el; }
    }
    return best;
  }
  /** Where to land on a card: near your focus, but not under your pointer. */
  function landingX(r, size) {
    const [lo, hi] = edgeRange(r, size);
    if (hi <= lo) return (r.left + r.right) / 2;
    let x;
    if (pointer.x >= r.left && pointer.x <= r.right && pointer.y >= r.top - 80 && pointer.y <= r.bottom) {
      const away = size * 1.5 + 50;
      x = pointer.x < (r.left + r.right) / 2 ? pointer.x + away : pointer.x - away;
    } else {
      x = Math.max(lo, Math.min(hi, pip.pos.x));
      if (x === pip.pos.x) x += rand(-60, 60);
    }
    return Math.max(lo, Math.min(hi, x));
  }
  function setAnchor(elm) {
    anchor = elm;
    anchorRect = elm ? elm.getBoundingClientRect() : null;
  }
  /** A card re-rendered (host tiles are rebuilt on refresh): find its twin. */
  function relink() {
    const key = anchor.id ? "#" + CSS.escape(anchor.id) : anchor.dataset.host !== undefined ? `[data-host="${CSS.escape(anchor.dataset.host)}"]` : null;
    const twin = key && document.querySelector(key);
    if (twin && twin.offsetParent) { anchor = twin; if (spot.kind === "card") spot.el = twin; }
    else setAnchor(null);
  }
  /** Keeps Pip on its card while the page scrolls or reflows. */
  function follow() {
    if (pip && anchor) {
      if (!anchor.isConnected) relink();
      if (!anchor) { if (spot.kind === "card") spot = { kind: "lost" }; }
      else {
        const r = anchor.getBoundingClientRect();
        const dx = r.left - anchorRect.left, dy = r.top - anchorRect.top;
        if (dx || dy) pip.shift(dx, dy);
        anchorRect = r;
        if (spot.kind === "card" && !pip.leapT && !standable(r)) spot = { kind: "lost" };
        else if (spot.kind === "card" && !pip.leapT && !pip.target) {
          // Card got narrower: step back onto it.
          const [lo, hi] = edgeRange(r, pip.pos.size);
          if (hi > lo && (pip.pos.x < lo - 2 || pip.pos.x > hi + 2)) pip.shift(Math.max(lo, Math.min(hi, pip.pos.x)) - pip.pos.x, 0);
        }
      }
    }
    requestAnimationFrame(follow);
  }

  // ================================================================ travel
  /** A chain of hops from where Pip is to the target card, via cards in between. */
  function route(target) {
    const tr = target.getBoundingClientRect();
    const hops = [];
    let at = { x: pip.pos.x, y: pip.pos.y };
    const used = new Set([target]);
    if (anchor) used.add(anchor);
    for (let k = 0; k < 4; k++) {
      const dy = Math.abs(tr.top - at.y);
      if (dy <= 380) break;
      // A stepping stone between here and there, roughly on the way.
      let best = null, bs = Infinity;
      for (const p of platforms()) {
        if (used.has(p.el) || !standable(p.r)) continue;
        const between = (p.r.top - at.y) * (tr.top - p.r.top) > 0;
        const step = Math.abs(p.r.top - at.y);
        if (!between || step > 380 || step < 80) continue;
        const score = Math.abs(p.r.top - tr.top) + Math.abs((p.r.left + p.r.right) / 2 - (tr.left + tr.right) / 2) * 0.3;
        if (score < bs) { bs = score; best = p; }
      }
      if (!best) break;
      used.add(best.el);
      const size = cardSize();
      const [lo, hi] = edgeRange(best.r, size);
      const x = Math.max(lo, Math.min(hi, (at.x + (tr.left + tr.right) / 2) / 2));
      hops.push({ el: best.el, x, y: best.r.top });
      at = { x, y: best.r.top };
    }
    return hops;
  }
  async function hopTo(elm, x) {
    const r = elm.getBoundingClientRect();
    setAnchor(elm);
    return pip.leap(x, r.top, { size: cardSize() });
  }
  /** Runs to a good take-off point on the current surface, then leaps. */
  async function travelTo(target) {
    if (busy) return;
    busy = true;
    pip.setSleepy(false);
    try {
      const size = cardSize();
      const hops = route(target);
      const tr = target.getBoundingClientRect();
      hops.push({ el: target, x: landingX(tr, size), y: tr.top });
      // Take-off: run along whatever Pip stands on toward the first hop.
      const first = hops[0];
      if (spot.kind === "card" && anchor) {
        const [lo, hi] = edgeRange(anchor.getBoundingClientRect(), pip.pos.size);
        const tx = Math.max(lo, Math.min(hi, first.x));
        if (Math.abs(tx - pip.pos.x) > 30) await pip.moveTo(tx, pip.pos.y, { speed: pip.pos.size * 3 });
      } else if (spot.kind === "ground") {
        const tx = Math.max(60, Math.min(innerWidth - 60, first.x - Math.sign(first.x - pip.pos.x) * 60));
        if (Math.abs(tx - pip.pos.x) > 40) await pip.moveTo(tx, groundY(), { speed: pip.pos.size * 3.2 });
      }
      for (const hp of hops) {
        const r = hp.el.getBoundingClientRect();
        if (!standable(r)) break;
        const ok = await hopTo(hp.el, Math.max(edgeRange(r, size)[0], Math.min(edgeRange(r, size)[1], hp.x)));
        if (!ok) return;
        spot = { kind: "card", el: hp.el };
        if (hp !== hops[hops.length - 1]) await sleep(rand(120, 260));
      }
      if (Math.random() < 0.35) pip.say(state === "idle" ? "Ooh, what's this?" : "Good view from up here!", 1800);
    } finally {
      busy = false;
      pip.setSleepy(state === "idle");
      nextMoveAt = performance.now() + rand(5000, 9000) * (state === "idle" ? 1.6 : 1);
    }
  }
  async function toGround() {
    if (busy) return;
    busy = true;
    try {
      setAnchor(null);
      const x = Math.max(80, Math.min(innerWidth - 80, pip.pos.x + rand(-80, 80)));
      await pip.leap(x, groundY(), { size: trailSize() });
      spot = { kind: "ground" };
    } finally {
      busy = false;
      pip.setSleepy(state === "idle");
      nextMoveAt = performance.now() + rand(4000, 7000);
    }
  }

  /** Decides, a couple of times a second, whether Pip should go somewhere. */
  async function brain() {
    for (;;) {
      await sleep(350);
      if (!pip || phase !== "trail" || busy || PipChat.isOpen() || document.hidden) continue;
      if (reduced() || !roams()) {
        if (spot.kind !== "ground") await toGround();
        continue;
      }
      const now = performance.now();
      if (spot.kind === "lost") {
        // The card scrolled away: go to the one you're on, or down to the trail.
        const c = pointer.card && standable(pointer.card.getBoundingClientRect()) ? pointer.card : null;
        if (c) await travelTo(c); else await toGround();
        continue;
      }
      if (now < nextMoveAt || pip.target) continue;
      const idleFor = now - pointer.t;
      if (pointer.y > innerHeight - TRAIL_H - 10 && spot.kind === "card" && now - pointer.since > 1200) { await toGround(); continue; }
      const c = pointer.card;
      if (c && c !== anchor && idleFor < 30000 && now - pointer.since > 1200 && standable(c.getBoundingClientRect())) {
        await travelTo(c);
        continue;
      }
      if (spot.kind === "card" && anchor && !pip.leapT) amble();
    }
  }
  /** Walks along the card's top edge now and then, more when tokens flow. */
  let ambling = false;
  async function amble() {
    if (ambling || state === "idle") return;
    ambling = true;
    try {
      const r = anchor.getBoundingClientRect();
      const size = pip.pos.size;
      const [lo, hi] = edgeRange(r, size);
      if (hi - lo < 30) return;
      let x = rand(lo, hi);
      // Stay out from under the pointer.
      if (Math.abs(x - pointer.x) < size * 1.2 && pointer.y > r.top - 90 && pointer.y < r.bottom) x = pointer.x < (lo + hi) / 2 ? hi - rand(0, 40) : lo + rand(0, 40);
      const fast = state === "stampede";
      await pip.moveTo(x, pip.pos.y, { speed: size * (fast ? 3.6 : 1.4) });
      await sleep(fast ? rand(150, 500) : rand(1200, 3500));
    } finally { ambling = false; }
  }

  // ================================================================ intro
  async function intro() {
    const overlay = $("intro");
    overlay.hidden = false;
    let skipped = false;
    const skip = () => { if (skipped) return; skipped = true; pip.cancel(); };
    $("intro-skip").onclick = skip;
    const onKey = (e) => { if (e.key === "Escape" || e.key === " " || e.key === "Enter") skip(); };
    addEventListener("keydown", onKey);
    overlay.addEventListener("click", skip, { once: true });
    const big = Math.min(innerHeight * 0.5, 400, innerWidth * 0.62);
    const cx = innerWidth / 2, cy = innerHeight / 2 + big * 0.12;
    pip.place(cx, cy, big);
    pip.setMood("processing");
    pip.setSleepy(true, true);
    pip.setTail(Array.from({ length: 40 }, (_, i) => 30 + 25 * Math.sin(i / 3) + i * 1.5), 0);
    typeTagline();
    await sleep(700);
    if (!skipped) { pip.setSleepy(false); await sleep(650); }
    if (!skipped) { pip.hop(); pip.say("Hi, I'm Pip! I count every token.", 2400); await sleep(900); }
    const Rx = innerWidth * 0.36, Ry = innerHeight * 0.26;
    const loop = [];
    for (let k = 1; k <= 7; k++) {
      const a = Math.PI * (0.05 + k * 0.29);
      const r = 0.45 + 0.55 * (k / 7);
      loop.push({ x: cx + Math.cos(a) * Rx * r, y: cy + Math.sin(a) * Ry * r - big * 0.1, size: big + (trailSize() - big) * (k / 8) });
    }
    loop.push({ x: innerWidth * 0.5, y: groundY(), size: trailSize() });
    for (let i = 0; i < loop.length && !skipped; i++) {
      const p = loop[i];
      if (i === loop.length - 2) overlay.classList.add("leaving");
      await pip.moveTo(p.x, p.y, { speed: Math.max(360, pip.pos.size * 3.2), size: p.size });
    }
    removeEventListener("keydown", onKey);
    overlay.classList.add("leaving");
    if (skipped) pip.place(innerWidth * 0.5, groundY(), trailSize());
    setTimeout(() => (overlay.hidden = true), 950);
  }
  function typeTagline() {
    const elm = $("intro-tag"), text = "Every token leaves a trail.";
    let i = 0; elm.textContent = "";
    const caret = document.createElement("span"); caret.className = "caret";
    const t = document.createTextNode(""); elm.append(t, caret);
    const step = () => { t.textContent = text.slice(0, ++i); if (i < text.length) setTimeout(step, 38); };
    setTimeout(step, 700);
  }

  // ================================================================ the trail
  async function trailLoop() {
    for (;;) {
      if (phase !== "trail" || busy || PipChat.isOpen() || spot.kind !== "ground" || document.hidden) { await sleep(200); continue; }
      const st = state;
      if (st === "idle" || reduced() || !roams()) { await sleep(300); continue; }
      const L = S.live || { tokens_per_sec: 0 };
      const fast = st === "stampede";
      const vigor = Math.min(1, L.tokens_per_sec / Math.max(30, (S.live && S.live.typical_tps) || 60));
      const size = trailSize();
      const lo = size * 0.9, hi = innerWidth - size * 0.9;
      const x0 = pip.pos.x;
      const span2 = fast ? 220 + Math.random() * 360 : 90 + Math.random() * 240;
      let dir = Math.random() < 0.5 ? -1 : 1;
      if (x0 + dir * span2 > hi) dir = -1;
      if (x0 + dir * span2 < lo) dir = 1;
      const x = Math.max(lo, Math.min(hi, x0 + dir * span2));
      const speed = (fast ? 4.2 : 1.6 + vigor * 1.6) * size;
      await pip.moveTo(x, groundY(), { speed, size });
      if (phase === "trail") await sleep(fast ? 60 + Math.random() * 120 : 300 + Math.random() * 1100 * (1 - vigor * 0.6));
    }
  }

  async function poll() {
    clearTimeout(liveTimer);
    try {
      const L = await getJSON("/api/live?seconds=60&since=" + (S.liveSeq || 0));
      if (typeof Dashboard !== "undefined") Dashboard.ingestLive(L);
      S.live = L;
      const next = L.state;
      if (next !== state) {
        state = next;
        if (pip) {
          pip.setMood(next);
          if (!busy) pip.setSleepy(next === "idle");
          if (phase === "trail") pip.say(LINES[next](L), 3200);
        }
      }
      if (pip) pip.setTail(L.per_second, Math.max(L.typical_tps || 0, 5));
      $("ticker-text").textContent = LINES[state](L) + (L.active_requests > 1 ? " (" + L.active_requests + " requests at once)" : "");
      if ((typeof Dashboard !== "undefined") && S.page === "dashboard") Dashboard.updateLive(L);
    } catch (e) {
      $("ticker-text").textContent = "Pip can't reach Toktail right now.";
    }
    liveTimer = setTimeout(poll, document.hidden ? 5000 : 1000);
  }

  // Token squares drift off Pip's tail while it eats.
  function sparkles(now) {
    if (pip && phase !== "loading" && state !== "idle" && !reduced() && !document.hidden && !pip.leapT) {
      const tps = (S.live && S.live.tokens_per_sec) || 0;
      const every = state === "stampede" ? 50 : Math.max(90, 420 - tps * 2);
      if (now - lastSparkle > every) {
        lastSparkle = now;
        const { x, y, size } = pip.pos;
        Pip.sparkle(x + -pip.facing * size * 0.62, y - size * 0.5, state === "stampede" ? 2 : 1, size / 124);
      }
    }
    requestAnimationFrame(sparkles);
  }

  const QUIPS = [
    () => S.lastData ? "This range: " + compact(S.lastData.tot.input + S.lastData.tot.output) + " tokens, in and out. Yum." : "Every token leaves a trail.",
    () => "All time: " + fmt(S.meta ? S.meta.models.reduce((s, m) => s + m.combined, 0) : 0) + " tokens. I counted every one.",
    () => "Press / and ask me anything about your trail.",
    () => S.live && S.live.typical_tps ? "Your usual speed is " + S.live.typical_tps.toFixed(1) + " tok/s." : "Waiting for tokens…",
    () => { const f = (S.flights || []).length; return f ? f + " request" + (f === 1 ? "" : "s") + " streaming right now. Nom!" : "Point at a card and I'll hop over."; },
    () => S.meta && S.meta.models[0] ? "Busiest model ever: " + S.meta.models[0].model + "." : "Every token leaves a trail.",
    () => "Double-click me to chat.",
  ];
  let quip = 0, clickTimer = 0;
  /** One click: a fact about your tokens. Two: open the chat. */
  function onPipClick() {
    if (phase !== "trail") return;
    clearTimeout(clickTimer);
    clickTimer = setTimeout(() => {
      pip.say(QUIPS[quip++ % QUIPS.length](), 3600);
      if (!reduced() && !pip.leapT) pip.hop();
    }, 230);
  }
  function onPipDouble() {
    if (phase !== "trail") return;
    clearTimeout(clickTimer);
    PipChat.open();
  }

  // ================================================================ replay
  async function replay(g) {
    if (phase !== "trail" || busy || !g) return toast(g ? "Pip is busy, try again in a moment." : "Nothing to replay in this range.");
    if (reduced()) return toast("Replay is off because your Mac asks for reduced motion.");
    phase = "replay"; busy = true;
    pip.cancel(); pip.setSleepy(false);
    setAnchor(null); spot = { kind: "ground" };
    const card = $("card-daily");
    card.scrollIntoView({ behavior: "smooth", block: "center" });
    await sleep(700);
    const groups = [...g.svg.querySelectorAll("[data-day]")];
    const extras = [...g.svg.querySelectorAll(".ma,.peak")];
    groups.forEach((n) => { n.classList.remove("anim-bar", "replay-show"); n.classList.add("replay-hidden"); });
    extras.forEach((n) => n.classList.add("replay-hidden"));
    const hud = h("div", { class: "replay-hud" });
    card.append(hud);
    const n = g.days.length;
    const toViewport = (i) => { const r = g.svg.getBoundingClientRect(), k = r.width / g.W; return { x: r.left + g.xc(i) * k, y: r.top + g.base * k }; };
    const size = 86;
    const start = toViewport(0), end = toViewport(n - 1);
    pip.say("Here's how we got here!", 2200);
    await pip.leap(start.x - 30, start.y, { size });
    let shown = -1, cum = 0;
    const reveal = () => {
      const r = g.svg.getBoundingClientRect(), k = r.width / g.W;
      while (shown + 1 < n && r.left + g.xc(shown + 1) * k <= pip.pos.x) {
        shown++;
        groups[shown].classList.remove("replay-hidden"); groups[shown].classList.add("replay-show");
        cum += g.totals[shown];
        hud.textContent = ""; hud.append(document.createTextNode(dayLabel(g.days[shown])), bold(fmt(cum)), document.createTextNode(METRIC_NAME[S.metric] + " tokens so far"));
      }
    };
    let watching = true;
    (function watch() { if (!watching) return; reveal(); requestAnimationFrame(watch); })();
    const ms = Math.max(2600, Math.min(7000, n * 140));
    await pip.moveTo(end.x + 24, end.y, { speed: (end.x - start.x + 54) / (ms / 1000), size });
    watching = false;
    reveal();
    for (let k = shown + 1; k < n; k++) { groups[k].classList.remove("replay-hidden"); cum += g.totals[k]; }
    extras.forEach((e) => { e.classList.remove("replay-hidden"); e.classList.add("anim-fade"); });
    pip.hop();
    pip.say("That's " + compact(cum) + " tokens. What a trail!", 3200);
    await sleep(1400);
    await pip.leap(innerWidth * 0.5, groundY(), { size: trailSize() });
    hud.remove();
    groups.forEach((x) => x.classList.remove("replay-hidden"));
    busy = false; phase = "trail"; spot = { kind: "ground" };
    nextMoveAt = performance.now() + 6000;
    pip.setSleepy(state === "idle");
  }

  // ================================================================ start
  async function start() {
    Pip.setDark(isDark());
    pip = Pip.create({ size: trailSize() }).mount();
    pip.onStep = (x, y, s) => Pip.step(x, y, s);
    const root = pip.svg.querySelector(".pip-root");
    root.addEventListener("click", onPipClick);
    root.addEventListener("dblclick", onPipDouble);
    pip.svg.setAttribute("tabindex", "0");
    pip.svg.setAttribute("role", "button");
    pip.svg.removeAttribute("aria-hidden");
    pip.svg.setAttribute("aria-label", "Pip the Token Fox. Enter for a token fact, C to chat.");
    pip.svg.addEventListener("keydown", (e) => {
      if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onPipClick(); }
      else if (e.key === "c" || e.key === "C") { e.preventDefault(); PipChat.open(); }
    });
    addEventListener("pointermove", (e) => {
      pointer.x = e.clientX; pointer.y = e.clientY; pointer.t = performance.now();
      const c = cardAt(e.clientX, e.clientY);
      if (c !== pointer.card) { pointer.card = c; pointer.since = pointer.t; }
    }, { passive: true });
    addEventListener("scroll", () => { pointer.card = pointer.x >= 0 ? cardAt(pointer.x, pointer.y) : null; }, { passive: true });
    poll();
    requestAnimationFrame(sparkles);
    requestAnimationFrame(follow);
    const wantIntro = S.meta && S.meta.portal ? S.meta.portal.intro_animation : true;
    if (wantIntro && !reduced()) await intro();
    else pip.place(innerWidth * 0.5, groundY(), trailSize());
    pip.setMood(state);
    pip.setSleepy(state === "idle");
    phase = "trail";
    nextMoveAt = performance.now() + 3500;
    trailLoop();
    brain();
    addEventListener("resize", () => {
      if (phase === "trail" && spot.kind === "ground" && !busy) pip.place(Math.min(pip.pos.x, innerWidth - 60), groundY(), trailSize());
    });
  }

  return {
    start, replay,
    cheer(msg) { if (pip && phase === "trail") pip.say(msg, 2600); },
    applySettings() { nextMoveAt = 0; },
    /** Pages and cards changed: re-check the card Pip stands on. */
    platformsChanged() {
      if (!pip || spot.kind !== "card") return;
      if (!anchor || !anchor.isConnected || !anchor.offsetParent) { setAnchor(null); spot = { kind: "lost" }; }
    },
  };
})();

