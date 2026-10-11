// SPDX-License-Identifier: Apache-2.0
// Pip the Token Fox: the rig (a layered, clipped copy of the mascot art),
// the RGB footstep lights, the hop engine, the intro run and the trail.
"use strict";

const Pip = (() => {
  const VB = 1095;                  // the artwork's viewBox
  const FEET = { x: 480, y: 1012 }; // where Pip touches the ground, in art units
  const NS = "http://www.w3.org/2000/svg";
  const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)");

  // Regions of the artwork, in art units. Each part gets its own layer so it
  // can move on its own: tail wags, ears twitch, the rest breathes and hops.
  const TAIL = "665,330 1095,330 1095,1012 702,1012 694,940 690,820 676,690 668,520";
  const EAR_L = "8,178 150,182 322,358 350,412 292,452 232,562 138,566 36,474";
  const EAR_R = "585,22 652,58 702,300 692,470 612,470 522,402 436,346 454,250 538,86";
  // The dark data panel on the tail, where Pip draws live throughput.
  const PANEL = "702,800 690,760 720,700 760,640 800,600 860,560 900,500 932,430 962,408 1012,450 1052,520 1070,600 1062,700 1032,770 982,800 932,822 862,832 782,842 722,832";
  // Colors of the static chart printed on the tail; hidden inside the panel
  // so the live line replaces it.
  const CHART_INK = ["#8e5cf2", "#ca9cf6", "#fefeff", "#cce5e6", "#373975"];
  // Fur colors, redrawn above the live line so the fluff stays in front.
  const FUR = ["#fcebd3", "#fc8939", "#f9caa7", "#e3612f"];

  const pts = (s) => s.split(" ").map((p) => p.split(",").map(Number));
  const poly = (s) => "M" + pts(s).map((p) => p.join(" ")).join("L") + "Z";

  let ready = null;
  /** Loads the artwork once and builds the shared <defs> sprite. */
  function load() {
    if (ready) return ready;
    ready = fetch("/pip.svg").then((r) => r.text()).then((text) => {
      const doc = new DOMParser().parseFromString(text, "image/svg+xml");
      const paths = [...doc.querySelectorAll("path")];
      const sprite = document.createElementNS(NS, "svg");
      sprite.setAttribute("aria-hidden", "true");
      sprite.style.cssText = "position:absolute;width:0;height:0;overflow:hidden";
      const defs = document.createElementNS(NS, "defs");
      const art = document.createElementNS(NS, "g"); art.id = "pip-art";
      const plain = document.createElementNS(NS, "g"); plain.id = "pip-art-plain";
      const fur = document.createElementNS(NS, "g"); fur.id = "pip-art-fur";
      for (const p of paths) {
        const fill = (p.getAttribute("fill") || "").toLowerCase();
        art.append(document.importNode(p, true));
        if (!CHART_INK.includes(fill)) plain.append(document.importNode(p, true));
        if (FUR.includes(fill)) fur.append(document.importNode(p, true));
      }
      defs.append(art, plain, fur);
      const clip = (id, d, rule) => {
        const c = document.createElementNS(NS, "clipPath"); c.id = id;
        c.setAttribute("clipPathUnits", "userSpaceOnUse");
        const p = document.createElementNS(NS, "path"); p.setAttribute("d", d);
        if (rule) p.setAttribute("clip-rule", rule);
        c.append(p); defs.append(c);
      };
      clip("pip-c-body", `M0 0H${VB}V${VB}H0Z` + poly(TAIL) + poly(EAR_L) + poly(EAR_R), "evenodd");
      clip("pip-c-tail-outer", poly(TAIL) + poly(PANEL), "evenodd");
      clip("pip-c-panel", poly(PANEL));
      clip("pip-c-earl", poly(EAR_L));
      clip("pip-c-earr", poly(EAR_R));
      sprite.append(defs);
      document.body.prepend(sprite);
    });
    return ready;
  }

  function svgLayer(cls, inner) {
    const s = document.createElementNS(NS, "svg");
    s.setAttribute("viewBox", `0 0 ${VB} ${VB}`);
    s.setAttribute("class", "pip-layer " + cls);
    s.innerHTML = inner;
    return s;
  }

  // The data squares that float in the tail panel around the live line.
  const SQUARES = [[760, 720, 26], [845, 600, 22], [968, 560, 28], [1020, 700, 22], [905, 770, 30], [800, 805, 20], [1000, 470, 18]];

  /** Builds one Pip. Returns { el, setState, setFacing, setTail, say, ... }. */
  function create(opts = {}) {
    const el = document.createElement("div");
    el.className = "pip";
    el.setAttribute("role", "img");
    el.setAttribute("aria-label", "Pip the Token Fox");
    const body = document.createElement("div"); body.className = "pip-body";
    const glow = document.createElement("div"); glow.className = "pip-tail-glow";
    const tail = svgLayer("pip-tail",
      `<polygon points="${PANEL}" fill="#0d0f38"/>` +
      `<g clip-path="url(#pip-c-panel)"><use href="#pip-art-plain"/>` +
      SQUARES.map(([x, y, s]) => `<rect class="sq" x="${x}" y="${y}" width="${s}" height="${s}" rx="5" fill="#cce5e6" opacity=".7"/>`).join("") +
      `<path class="live-glow" fill="none" stroke="#8e5cf2" stroke-width="30" stroke-linecap="round" stroke-linejoin="round" opacity=".55"/>` +
      `<path class="live-mid" fill="none" stroke="#ca9cf6" stroke-width="14" stroke-linecap="round" stroke-linejoin="round"/>` +
      `<path class="live-core" fill="none" stroke="#fefeff" stroke-width="5" stroke-linecap="round" stroke-linejoin="round"/>` +
      `<circle class="live-dot" r="17" fill="#fefeff" stroke="#8e5cf2" stroke-width="8"/>` +
      `<use href="#pip-art-fur"/></g>` +
      `<g clip-path="url(#pip-c-tail-outer)"><use href="#pip-art"/></g>`);
    const main = svgLayer("pip-main",
      `<g clip-path="url(#pip-c-body)"><use href="#pip-art"/></g>` +
      // Eyelids, shown while Pip naps.
      `<g class="lids">` +
      `<ellipse cx="382" cy="530" rx="60" ry="54" fill="#fc8939"/><ellipse cx="527" cy="481" rx="41" ry="54" fill="#fc8939"/>` +
      `<path d="M326 532 Q382 574 438 528" fill="none" stroke="#0d0f38" stroke-width="10" stroke-linecap="round"/>` +
      `<path d="M490 488 Q527 518 566 478" fill="none" stroke="#0d0f38" stroke-width="10" stroke-linecap="round"/>` +
      `<path d="M330 536 L314 548 M490 492 L476 503" stroke="#0d0f38" stroke-width="8" stroke-linecap="round"/></g>`);
    const earL = svgLayer("pip-ear pip-ear-l", `<g clip-path="url(#pip-c-earl)"><use href="#pip-art"/></g>`);
    const earR = svgLayer("pip-ear pip-ear-r", `<g clip-path="url(#pip-c-earr)"><use href="#pip-art"/></g>`);
    const zzz = document.createElement("div"); zzz.className = "pip-zzz"; zzz.innerHTML = "<span>z</span><span>z</span><span>z</span>";
    const bubble = document.createElement("div"); bubble.className = "pip-bubble"; bubble.setAttribute("aria-live", "polite");
    body.append(glow, tail, main, earL, earR);
    const flip = document.createElement("div"); flip.className = "pip-flip";
    flip.append(body);
    el.append(flip, zzz, bubble);
    if (opts.size) el.style.setProperty("--pip-size", opts.size + "px");

    const livePaths = tail.querySelectorAll(".live-glow,.live-mid,.live-core");
    const dot = tail.querySelector(".live-dot");
    let state = "idle", bubbleTimer = 0;

    /** Draws a series (oldest first) along the tail panel. */
    function setTail(values, scaleMax) {
      const n = values.length;
      const max = Math.max(scaleMax || 0, ...values, 1);
      let d = "";
      let last = [0, 0];
      for (let i = 0; i < n; i++) {
        const t = n === 1 ? 1 : i / (n - 1);
        // The panel runs diagonally: low on the left, high on the right.
        const x = 722 + t * 318;
        const base = 808 - t * 150;
        const amp = 120 + t * 90;
        const y = base - (values[i] / max) * amp;
        d += (i ? "L" : "M") + x.toFixed(1) + " " + y.toFixed(1);
        last = [x, y];
      }
      livePaths.forEach((p) => p.setAttribute("d", d));
      dot.setAttribute("cx", last[0]); dot.setAttribute("cy", last[1]);
    }
    setTail(new Array(40).fill(0), 1);

    function setState(s) {
      if (s === state) return;
      el.classList.remove("is-idle", "is-processing", "is-stampede");
      el.classList.add("is-" + s);
      state = s;
    }
    setState("idle"); el.classList.add("is-idle");

    /** Keeps the speech bubble inside the window wherever Pip stands. */
    function clampBubble() {
      bubble.style.marginLeft = "0px";
      const r = bubble.getBoundingClientRect();
      let dx = 0;
      if (r.left < 8) dx = 8 - r.left;
      else if (r.right > innerWidth - 8) dx = innerWidth - 8 - r.right;
      if (dx) bubble.style.marginLeft = dx + "px";
    }

    function say(text, ms = 3200) {
      bubble.textContent = text;
      el.classList.add("is-talking");
      requestAnimationFrame(clampBubble);
      clearTimeout(bubbleTimer);
      if (ms) bubbleTimer = setTimeout(() => el.classList.remove("is-talking"), ms);
    }

    return {
      el, setTail, setState, say, clampBubble,
      get state() { return state; },
      setFacing(right) { el.classList.toggle("facing-right", !!right); },
      size() { return el.getBoundingClientRect().width || opts.size || 110; },
    };
  }

  // ------------------------------------------------------------ RGB lights
  // Each footstep lights the ground with pure red, green or blue light.
  // Lights add up where they overlap, like real light: red and green make
  // yellow, all three make white.
  const RGB = ["255,0,0", "0,255,0", "0,0,255"];
  let rgbIndex = 0;
  const lights = [];
  let canvas, ctx, dpr = 1, drawing = false;
  const particles = [];

  function ensureCanvas() {
    if (canvas) return;
    canvas = document.createElement("canvas");
    canvas.className = "pip-lights";
    canvas.setAttribute("aria-hidden", "true");
    document.body.append(canvas);
    ctx = canvas.getContext("2d");
    const size = () => {
      dpr = Math.min(2, devicePixelRatio || 1);
      canvas.width = innerWidth * dpr; canvas.height = innerHeight * dpr;
    };
    size(); addEventListener("resize", size);
  }

  function nextColor() { const c = RGB[rgbIndex]; rgbIndex = (rgbIndex + 1) % 3; return c; }

  /** A footstep light at viewport point (x, y). `scale` follows Pip's size. */
  function step(x, y, scale = 1, color) {
    ensureCanvas();
    lights.push({ x, y, s: scale, c: color || nextColor(), t0: performance.now() });
    if (lights.length > 60) lights.shift();
    kick();
  }

  /** Token squares that drift up from Pip's tail while it eats. */
  function sparkle(x, y, n = 1, scale = 1) {
    ensureCanvas();
    const colors = ["#cce5e6", "#ca9cf6", "#fcebd3", "#8e5cf2"];
    for (let i = 0; i < n; i++) {
      particles.push({
        x: x + (Math.random() - 0.5) * 30 * scale, y, vx: (Math.random() - 0.5) * 24,
        vy: -(30 + Math.random() * 50) * scale, size: (3 + Math.random() * 5) * scale,
        c: colors[(Math.random() * colors.length) | 0], t0: performance.now(), life: 1000 + Math.random() * 700,
      });
    }
    if (particles.length > 160) particles.splice(0, particles.length - 160);
    kick();
  }

  function kick() { if (!drawing) { drawing = true; requestAnimationFrame(draw); } }

  const LIGHT_MS = 2200;
  function draw(now) {
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, innerWidth, innerHeight);
    ctx.globalCompositeOperation = "lighter";
    for (let i = lights.length - 1; i >= 0; i--) {
      const L = lights[i];
      const age = Math.max(0, (now - L.t0) / LIGHT_MS);
      if (age >= 1) { lights.splice(i, 1); continue; }
      // Fast bloom, long soft fade.
      const rise = Math.min(1, age * 9);
      const fade = Math.pow(1 - age, 1.8);
      const a = rise * fade;
      const R = (60 + 90 * Math.sqrt(age)) * L.s;
      // Floor pool: a squashed radial light.
      ctx.save();
      ctx.translate(L.x, L.y);
      ctx.scale(1, 0.32);
      let g = ctx.createRadialGradient(0, 0, 0, 0, 0, R * 1.6);
      g.addColorStop(0, `rgba(${L.c},${0.95 * a})`);
      g.addColorStop(0.35, `rgba(${L.c},${0.45 * a})`);
      g.addColorStop(1, `rgba(${L.c},0)`);
      ctx.fillStyle = g;
      ctx.beginPath(); ctx.arc(0, 0, R * 1.6, 0, Math.PI * 2); ctx.fill();
      // Contact ripple.
      ctx.strokeStyle = `rgba(${L.c},${0.7 * fade * (1 - age)})`;
      ctx.lineWidth = 2.2 / 0.32 * 0.5;
      ctx.beginPath(); ctx.arc(0, 0, R * (0.4 + age * 1.3), 0, Math.PI * 2); ctx.stroke();
      ctx.restore();
      // Aura: the light reaching up around Pip.
      const aura = ctx.createRadialGradient(L.x, L.y - 30 * L.s, 0, L.x, L.y - 30 * L.s, R * 1.25);
      aura.addColorStop(0, `rgba(${L.c},${0.32 * a})`);
      aura.addColorStop(1, `rgba(${L.c},0)`);
      ctx.fillStyle = aura;
      ctx.beginPath(); ctx.arc(L.x, L.y - 30 * L.s, R * 1.25, 0, Math.PI * 2); ctx.fill();
    }
    ctx.globalCompositeOperation = "source-over";
    for (let i = particles.length - 1; i >= 0; i--) {
      const p = particles[i];
      const age = Math.max(0, (now - p.t0) / p.life);
      if (age >= 1) { particles.splice(i, 1); continue; }
      const t = Math.max(0, now - p.t0) / 1000;
      const x = p.x + p.vx * t + Math.sin(t * 3 + p.size) * 6;
      const y = p.y + p.vy * t;
      ctx.globalAlpha = Math.min(1, (1 - age) * 1.4) * 0.9;
      ctx.fillStyle = p.c;
      const s = p.size;
      ctx.beginPath(); ctx.roundRect ? ctx.roundRect(x - s / 2, y - s / 2, s, s, s / 4) : ctx.rect(x - s / 2, y - s / 2, s, s); ctx.fill();
    }
    ctx.globalAlpha = 1;
    if (lights.length || particles.length) requestAnimationFrame(draw);
    else { drawing = false; ctx.clearRect(0, 0, innerWidth, innerHeight); }
  }

  // ------------------------------------------------------------ hops
  const easeInOut = (t) => t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2;

  /**
   * Moves a Pip (positioned by its feet in viewport coordinates) through a
   * list of hops. Each hop: { x, y, size?, height?, ms? }. Calls onLand at
   * every landing. Returns a promise that resolves after the last landing.
   */
  function hopPath(pip, from, hops, onLand) {
    return new Promise((resolve) => {
      let i = 0, cur = { ...from }, t0 = performance.now(), cancelled = false;
      const run = (now) => {
        if (cancelled) return resolve(cur);
        const h = hops[i];
        const ms = h.ms || 420;
        const t = Math.min(1, (now - t0) / ms);
        const e = easeInOut(t);
        const x = cur.x + (h.x - cur.x) * e;
        const y = cur.y + (h.y - cur.y) * e;
        const size = cur.size + ((h.size || cur.size) - cur.size) * e;
        const height = (h.height ?? Math.min(140, Math.abs(h.x - cur.x) * 0.45 + 26)) * (size / 110);
        const lift = 4 * height * t * (1 - t);
        // Squash at take-off and landing, stretch in the air.
        const air = Math.sin(Math.PI * t);
        const sy = t < 0.12 ? 1 - (0.12 - t) * 1.1 : t > 0.88 ? 1 - (t - 0.88) * 1.1 : 1 + air * 0.06;
        const sx = 2 - sy;
        pip.setFacing(h.x > cur.x + 1);
        place(pip, x, y - lift, size, sx, sy, (h.x - cur.x) * 0.02 * air);
        if (t >= 1) {
          cur = { x: h.x, y: h.y, size: h.size || cur.size };
          onLand && onLand(cur, i);
          i++;
          if (i >= hops.length) return resolve(cur);
          t0 = now + (hops[i].pause || 0);
        }
        requestAnimationFrame(run);
      };
      requestAnimationFrame(run);
      pip.cancel = () => { cancelled = true; };
    });
  }

  /** Puts Pip's feet at (x, y) in the viewport, at `size` pixels tall. */
  function place(pip, x, y, size, sx = 1, sy = 1, tilt = 0) {
    const left = x - size * (FEET.x / VB);
    const top = y - size * (FEET.y / VB);
    pip.el.style.setProperty("--pip-size", size + "px");
    pip.el.style.transform = `translate3d(${left.toFixed(1)}px, ${top.toFixed(1)}px, 0)`;
    pip.el.querySelector(".pip-body").style.transform =
      `rotate(${tilt.toFixed(2)}deg) scale(${sx.toFixed(3)}, ${sy.toFixed(3)})`;
    pip.pos = { x, y, size };
    if (pip.el.classList.contains("is-talking")) pip.clampBubble();
  }

  return { load, create, step, sparkle, hopPath, place, reduceMotion, FEET, VB };
})();
