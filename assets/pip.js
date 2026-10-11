// SPDX-License-Identifier: Apache-2.0
// Pip the Token Fox, as a living character.
//
// Pip is drawn here in vector parts (body, head, two ears, four two-bone
// legs and a spine-driven tail) in the style of the mascot artwork, and
// animated procedurally every frame:
//   - a gallop cycle where each paw is placed with inverse kinematics, so
//     feet plant on the ground instead of sliding,
//   - a tail that flows behind the body and carries the live tokens/sec line,
//   - blinking, ear twitches, breathing, a curled-up nap and a stampede mode.
// Every time a paw touches down it lights the ground in pure red, green or blue.
"use strict";

const Pip = (() => {
  const NS = "http://www.w3.org/2000/svg";
  const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)");

  // Palette taken from the mascot artwork.
  const C = {
    orange: "#fc8939", orangeDeep: "#e3612f", far: "#d9692c", cream: "#fcebd3", peach: "#f9caa7",
    navy: "#0d0f38", sock: "#23266a", sockFar: "#171a4e", lav: "#8e5cf2", lavLight: "#ca9cf6",
    mint: "#cce5e6", white: "#fefeff",
  };
  const OUT = 4.5;      // outline width, rig units
  const HEIGHT = 250;   // rig units from the ground to the ear tips; `size` in px maps to this

  // Rig geometry (rig units, Pip faces +x, ground at y = 0, y grows downward).
  const HIP = { x: -38, y: 4 }, SHOULDER = { x: 34, y: 2 };          // relative to the body origin
  const NECK = { x: 40, y: -16 };                                    // head pivot, relative to the body
  const TAIL_BASE = { x: -56, y: -10 };
  const LEGS = [ // drawn in this order: far legs behind the body, near legs in front
    { name: "hindFar", hind: true, near: false, phase: 0.0 },
    { name: "foreFar", hind: false, near: false, phase: 0.5 },
    { name: "hindNear", hind: true, near: true, phase: 0.08 },
    { name: "foreNear", hind: false, near: true, phase: 0.58 },
  ];
  const LEG_DIM = { hind: { l1: 32, l2: 38, w1: 15, w2: 10 }, fore: { l1: 31, l2: 36, w1: 13, w2: 9.5 } };
  const STRIDE = 64, STANCE = 0.46;
  const TAIL_SEG = 26, TAIL_N = 7, TAIL_SAMPLES = 42;

  // ------------------------------------------------------------ math
  const lerp = (a, b, t) => a + (b - a) * t;
  const clamp = (v, a, b) => Math.max(a, Math.min(b, v));
  const smooth = (e0, e1, x) => { const t = clamp((x - e0) / (e1 - e0), 0, 1); return t * t * (3 - 2 * t); };
  const rad = (d) => (d * Math.PI) / 180;
  const easeInOut = (t) => (t < 0.5 ? 2 * t * t : 1 - Math.pow(-2 * t + 2, 2) / 2);
  const f1 = (v) => v.toFixed(1);

  function el(tag, attrs, parent) {
    const e = document.createElementNS(NS, tag);
    for (const k in attrs) e.setAttribute(k, attrs[k]);
    if (parent) parent.append(e);
    return e;
  }

  // Catmull-Rom through points, sampled `n` times; returns [{x,y,tx,ty}].
  function spline(pts, n) {
    const out = [];
    const segs = pts.length - 1;
    for (let i = 0; i < n; i++) {
      const u = (i / (n - 1)) * segs;
      const k = Math.min(segs - 1, Math.floor(u)), t = u - k;
      const p0 = pts[Math.max(0, k - 1)], p1 = pts[k], p2 = pts[k + 1], p3 = pts[Math.min(segs, k + 2)];
      const t2 = t * t, t3 = t2 * t;
      const x = 0.5 * (2 * p1.x + (-p0.x + p2.x) * t + (2 * p0.x - 5 * p1.x + 4 * p2.x - p3.x) * t2 + (-p0.x + 3 * p1.x - 3 * p2.x + p3.x) * t3);
      const y = 0.5 * (2 * p1.y + (-p0.y + p2.y) * t + (2 * p0.y - 5 * p1.y + 4 * p2.y - p3.y) * t2 + (-p0.y + 3 * p1.y - 3 * p2.y + p3.y) * t3);
      const dx = 0.5 * ((-p0.x + p2.x) + 2 * (2 * p0.x - 5 * p1.x + 4 * p2.x - p3.x) * t + 3 * (-p0.x + 3 * p1.x - 3 * p2.x + p3.x) * t2);
      const dy = 0.5 * ((-p0.y + p2.y) + 2 * (2 * p0.y - 5 * p1.y + 4 * p2.y - p3.y) * t + 3 * (-p0.y + 3 * p1.y - 3 * p2.y + p3.y) * t2);
      const len = Math.hypot(dx, dy) || 1;
      out.push({ x, y, tx: dx / len, ty: dy / len });
    }
    return out;
  }

  // Two-bone IK. `back` bends the joint toward -x (hind hock), else toward +x (fore wrist).
  function ik(hip, foot, l1, l2, back) {
    let dx = foot.x - hip.x, dy = foot.y - hip.y;
    let d = Math.hypot(dx, dy);
    const max = l1 + l2 - 0.5, min = Math.abs(l1 - l2) + 4;
    const dd = clamp(d, min, max);
    if (d < 1e-3) { dx = 0; dy = 1; d = 1; }
    const a = Math.atan2(dy, dx);
    const cos = clamp((l1 * l1 + dd * dd - l2 * l2) / (2 * l1 * dd), -1, 1);
    const theta = Math.acos(cos);
    const a1 = back ? a + theta : a - theta;
    const knee = { x: hip.x + l1 * Math.cos(a1), y: hip.y + l1 * Math.sin(a1) };
    const end = { x: hip.x + (dx / d) * dd, y: hip.y + (dy / d) * dd };
    return { knee, end };
  }

  // ------------------------------------------------------------ drawing the rig
  const EAR_OUTER = "M -22 2 C -29 -40 -17 -92 4 -120 C 23 -92 31 -40 22 2 Z";
  const EAR_INNER = "M -13 -5 C -18 -40 -9 -82 4 -104 C 15 -82 19 -40 13 -5 Z";
  const EAR_STRIPES = "M -8 -18 L 0 -28 L -10 -38 L 2 -52 L -6 -62 L 5 -88 L 9 -62 L 2 -52 L 11 -40 L 3 -28 L 10 -16 Z";
  const EAR_FUR = "M -15 -2 L -9 -18 L -3 -8 L 4 -21 L 8 -8 L 15 -16 L 14 0 Z";
  const TORSO = "M 52 -8 C 56 -30 28 -36 0 -34 C -30 -32 -60 -28 -64 -6 C -67 14 -50 27 -26 27 L 22 25 C 44 23 50 8 52 -8 Z";
  const BELLY = "M -32 19 C -10 27 22 25 42 12 C 28 5 -6 7 -32 19 Z";
  const CHEST = "M 38 -22 L 58 -16 L 49 -9 L 61 -1 L 50 4 L 57 12 L 42 15 C 34 6 33 -10 38 -22 Z";
  const SPIKES = "M -54 -26 L -48 -46 L -38 -31 L -30 -54 L -18 -34 L -8 -58 L 2 -35 L 12 -55 L 20 -34 L 30 -48 L 34 -30 Z";
  const HEAD = "M -6 -34 C -8 -67 20 -81 44 -75 C 63 -70 73 -56 75 -43 C 83 -41 93 -37 99 -31 C 103 -27 101 -20 95 -18 C 85 -14 75 -10 67 -4 C 53 8 22 12 6 4 C -6 -4 -6 -20 -6 -34 Z";
  const MUZZLE = "M 52 -30 C 66 -35 84 -33 97 -27 C 93 -19 81 -13 67 -7 C 57 -4 46 -10 46 -19 C 46 -25 48 -29 52 -30 Z";
  const CHEEK = "M 4 -10 L -10 -2 L 0 1 L -8 12 L 6 8 L 4 19 L 18 9 C 12 4 7 -2 4 -10 Z";
  const TUFT = "M 14 -74 L 20 -90 L 26 -76 L 34 -88 L 36 -74 Z";

  function buildSvg() {
    const svg = el("svg", { class: "pip-svg", viewBox: "-260 -290 520 330", "aria-hidden": "true" });
    const root = el("g", { class: "pip-root" }, svg);
    const r = { svg, root };
    r.shadow = el("ellipse", { cx: 0, cy: 2, rx: 74, ry: 8, fill: C.navy, opacity: 0.16 }, root);

    // Tail: fur fill with outline, the data panel, its live line, the cream tip.
    r.tail = el("g", { class: "pip-tail" }, root);
    r.tailBody = el("path", { fill: C.orange, stroke: C.navy, "stroke-width": OUT, "stroke-linejoin": "round" }, r.tail);
    r.tailPanel = el("path", { fill: C.navy }, r.tail);
    r.tailSquares = [0.38, 0.47, 0.55, 0.62, 0.69].map((t, i) => ({ t, side: i % 2 ? 0.55 : -0.55, el: el("rect", { width: 7, height: 7, rx: 2, fill: C.mint, opacity: 0.85 }, r.tail) }));
    r.tailGlow = el("path", { fill: "none", stroke: C.lav, "stroke-width": 9, "stroke-linecap": "round", "stroke-linejoin": "round", opacity: 0.6 }, r.tail);
    r.tailMid = el("path", { fill: "none", stroke: C.lavLight, "stroke-width": 4.5, "stroke-linecap": "round", "stroke-linejoin": "round" }, r.tail);
    r.tailCore = el("path", { fill: "none", stroke: C.white, "stroke-width": 1.8, "stroke-linecap": "round", "stroke-linejoin": "round" }, r.tail);
    r.tailDot = el("circle", { r: 4.5, fill: C.white, stroke: C.lav, "stroke-width": 2.5 }, r.tail);
    r.tailTip = el("path", { fill: C.cream }, r.tail);

    r.legs = {};
    const drawLeg = (L, parent) => {
      const d = L.hind ? LEG_DIM.hind : LEG_DIM.fore;
      const furCol = L.near ? C.orange : C.far, sockCol = L.near ? C.sock : C.sockFar;
      const g = el("g", { class: "pip-leg" }, parent);
      const o1 = el("line", { stroke: C.navy, "stroke-width": d.w1 + OUT * 2, "stroke-linecap": "round" }, g);
      const o2 = el("line", { stroke: C.navy, "stroke-width": d.w2 + OUT * 2, "stroke-linecap": "round" }, g);
      const paw = el("ellipse", { rx: 10, ry: 6, fill: sockCol, stroke: C.navy, "stroke-width": OUT * 0.9 }, g);
      const s1 = el("line", { stroke: furCol, "stroke-width": d.w1, "stroke-linecap": "round" }, g);
      const s2 = el("line", { stroke: furCol, "stroke-width": d.w2, "stroke-linecap": "round" }, g);
      const s3 = el("line", { stroke: sockCol, "stroke-width": d.w2, "stroke-linecap": "round" }, g);
      const toe = el("path", { d: "M -6 -1 Q 0 -6 6 -1", fill: "none", stroke: C.lav, "stroke-width": 2.4, "stroke-linecap": "round" }, g);
      const haunch = L.hind ? el("ellipse", { rx: 19, ry: 25, fill: furCol, stroke: L.near ? "#d5592a" : "#b9531f", "stroke-width": 2.2 }, g) : null;
      r.legs[L.name] = { o1, o2, s1, s2, s3, paw, toe, haunch };
    };
    LEGS.filter((L) => !L.near).forEach((L) => drawLeg(L, root));

    // Body.
    r.body = el("g", { class: "pip-bodyg" }, root);
    r.spikes = el("path", { d: SPIKES, fill: C.orangeDeep, stroke: C.navy, "stroke-width": OUT, "stroke-linejoin": "round", opacity: 0 }, r.body);
    el("path", { d: TORSO, fill: C.orange, stroke: C.navy, "stroke-width": OUT, "stroke-linejoin": "round" }, r.body);
    el("path", { d: BELLY, fill: C.cream }, r.body);
    el("path", { d: CHEST, fill: C.cream, stroke: C.navy, "stroke-width": OUT * 0.8, "stroke-linejoin": "round" }, r.body);

    // Head with ears, eye, nose and fluff.
    r.head = el("g", { class: "pip-head" }, r.body);
    const ear = (far) => {
      const g = el("g", {}, r.head);
      const inner = el("g", {}, g);
      el("path", { d: EAR_OUTER, fill: far ? C.far : C.orange, stroke: C.navy, "stroke-width": OUT, "stroke-linejoin": "round" }, inner);
      el("path", { d: EAR_INNER, fill: C.lavLight }, inner);
      el("path", { d: EAR_STRIPES, fill: C.lav, opacity: far ? 0.8 : 1 }, inner);
      el("path", { d: EAR_FUR, fill: C.cream }, inner);
      return g;
    };
    r.earFar = ear(true);
    el("path", { d: HEAD, fill: C.orange, stroke: C.navy, "stroke-width": OUT, "stroke-linejoin": "round" }, r.head);
    el("path", { d: TUFT, fill: C.orange, stroke: C.navy, "stroke-width": OUT * 0.8, "stroke-linejoin": "round" }, r.head);
    el("path", { d: MUZZLE, fill: C.cream }, r.head);
    el("path", { d: CHEEK, fill: C.cream, stroke: C.navy, "stroke-width": OUT * 0.7, "stroke-linejoin": "round" }, r.head);
    r.eye = el("g", {}, r.head);
    el("ellipse", { cx: 52, cy: -44, rx: 12, ry: 15, fill: C.navy, transform: "rotate(-8 52 -44)" }, r.eye);
    el("ellipse", { cx: 53, cy: -39, rx: 8, ry: 8.5, fill: C.lav }, r.eye);
    el("circle", { cx: 54, cy: -41, r: 4.6, fill: C.navy }, r.eye);
    el("circle", { cx: 48.5, cy: -50, r: 4.4, fill: C.white }, r.eye);
    el("circle", { cx: 57, cy: -36.5, r: 2.1, fill: C.white }, r.eye);
    r.lash = el("path", { d: "M 38 -50 Q 52 -63 67 -53", fill: "none", stroke: C.navy, "stroke-width": 4.2, "stroke-linecap": "round" }, r.head);
    r.lid = el("path", { d: "M 39 -45 Q 52 -35 65 -46 M 40 -44 L 34 -40", fill: "none", stroke: C.navy, "stroke-width": 4.4, "stroke-linecap": "round", opacity: 0 }, r.head);
    el("ellipse", { cx: 97, cy: -25, rx: 5.6, ry: 4.3, fill: C.navy }, r.head);
    el("path", { d: "M 81 -14 Q 87 -9 93 -15", fill: "none", stroke: C.navy, "stroke-width": 3, "stroke-linecap": "round" }, r.head);
    r.blush = el("ellipse", { cx: 66, cy: -24, rx: 7, ry: 4, fill: "#ff8fa3", opacity: 0.35 }, r.head);
    r.earNear = ear(false);

    LEGS.filter((L) => L.near).forEach((L) => drawLeg(L, root));
    return r;
  }

  // ------------------------------------------------------------ the character
  const all = new Set();
  let ticking = false;

  class Fox {
    constructor(opts = {}) {
      this.r = buildSvg();
      this.svg = this.r.svg;
      this.size = opts.size || 120;
      this.x = innerWidth / 2; this.y = innerHeight - 22;
      this.target = null; this.speed = 0; this.v = 0;
      this.facing = 1; this.flip = 1;
      this.phase = 0; this.runW = 0; this.sleepW = 0; this.t = 0;
      this.mood = "idle"; this.wantSleep = false;
      this.blinkAt = 2; this.blink = 0; this.twitchAt = 3; this.twitch = 0;
      this.jump = null; this.prevPhases = LEGS.map(() => 0);
      this.leapT = null; this.air = 0; this.crouch = 0; this.land = 0;
      this.tailValues = new Array(40).fill(0); this.tailMax = 1;
      this.onStep = null;
      this.tail = []; // current tail angles (deg), smoothed
      // Overlays that must not mirror when Pip turns around.
      this.zzz = document.createElement("div"); this.zzz.className = "pip-zzz"; this.zzz.innerHTML = "<span>z</span><span>z</span><span>z</span>";
      this.bubble = document.createElement("div"); this.bubble.className = "pip-bubble"; this.bubble.setAttribute("aria-live", "polite");
      this.bubbleUntil = 0;
    }

    mount(parent = document.body) {
      parent.append(this.svg, this.zzz, this.bubble);
      all.add(this);
      if (!ticking) { ticking = true; last = performance.now(); requestAnimationFrame(tick); }
      this.render();
      return this;
    }
    unmount() { all.delete(this); this.svg.remove(); this.zzz.remove(); this.bubble.remove(); }

    get pos() { return { x: this.x, y: this.y, size: this.size }; }
    get scale() { return this.size / HEIGHT; }

    place(x, y, size) { this.x = x; this.y = y; if (size) this.size = size; this.cancel(); }

    /** Runs to (x, y) at `speed` px/s, resizing to `size` on the way. */
    moveTo(x, y, { speed = 160, size } = {}) {
      this.cancel();
      return new Promise((resolve) => {
        this.target = { x, y, speed, size: size || this.size, size0: this.size, d0: Math.hypot(x - this.x, y - this.y) || 1, resolve };
      });
    }
    cancel() {
      if (this.target) { const t = this.target; this.target = null; t.resolve(false); }
      if (this.leapT) { const l = this.leapT; this.leapT = null; this.air = 0; l.resolve(false); }
    }

    /** Jumps in an arc to (x, y): crouch, fly with paws tucked, land with a
     *  little squash and a flash of light under each paw. */
    leap(x, y, { height, dur, size } = {}) {
      this.cancel();
      const dx = x - this.x, dy = y - this.y;
      const dist = Math.hypot(dx, dy);
      const h = height ?? Math.max(40, Math.min(220, 50 + Math.max(0, -dy) * 0.6 + dist * 0.12));
      const d = dur ?? clamp(0.45 + dist / 1400 + h / 900, 0.5, 1.15);
      if (Math.abs(dx) > 2) this.facing = dx > 0 ? 1 : -1;
      return new Promise((resolve) => {
        this.leapT = { x0: this.x, y0: this.y, x1: x, y1: y, h, dur: d, t: -0.14, size0: this.size, size1: size || this.size, resolve };
      });
    }

    /** Moves Pip (and where Pip is headed) by a delta, e.g. when the page
     *  scrolls under the card Pip is standing on. */
    shift(dx, dy) {
      this.x += dx; this.y += dy;
      if (this.target) { this.target.x += dx; this.target.y += dy; }
      if (this.leapT) { this.leapT.x0 += dx; this.leapT.y0 += dy; this.leapT.x1 += dx; this.leapT.y1 += dy; }
    }

    setMood(m) { this.mood = m; this.svg.dataset.mood = m; }
    setSleepy(b, instant) { this.wantSleep = b; if (instant) this.sleepW = b ? 1 : 0; }
    setTail(values, max) { if (values && values.length) { this.tailValues = values.slice(-48); this.tailMax = Math.max(max || 0, ...this.tailValues, 1); } }
    hop() { if (!this.jump && this.sleepW < 0.5) this.jump = { t: 0, dur: 0.5, h: 46 }; }

    say(text, ms = 3200) {
      this.bubble.textContent = text;
      this.bubble.classList.add("on");
      this.bubbleUntil = ms ? performance.now() + ms : Infinity;
    }

    update(dt) {
      this.t += dt;
      const s = this.scale;
      // ---- leaping: a crouch, a parabola, a landing
      let moving = false;
      if (this.leapT) {
        const L = this.leapT;
        L.t += dt;
        if (L.t < 0) {
          // Wind-up: crouch before take-off.
          this.crouch = Math.min(1, this.crouch + dt * 9);
        } else {
          const u = Math.min(1, L.t / L.dur);
          this.crouch = Math.max(0, this.crouch - dt * 12);
          this.x = lerp(L.x0, L.x1, u);
          this.y = lerp(L.y0, L.y1, u) - L.h * 4 * u * (1 - u);
          this.size = lerp(L.size0, L.size1, u);
          this.air = Math.sin(Math.PI * u);
          this.airU = u;
          if (u >= 1) {
            this.x = L.x1; this.y = L.y1; this.air = 0; this.leapT = null; this.land = 1;
            this.emitAll();
            L.resolve(true);
          }
        }
      } else {
        this.crouch = Math.max(0, this.crouch - dt * 8);
      }
      this.land = Math.max(0, this.land - dt * 5);
      if (this.target) {
        const T = this.target;
        const dx = T.x - this.x, dy = T.y - this.y;
        const dist = Math.hypot(dx, dy);
        const accel = T.speed * 3;
        // Ease in, cruise, and brake so Pip lands exactly on the spot.
        const stopV = Math.sqrt(2 * accel * dist) + 12;
        this.v = Math.min(T.speed, this.v + accel * dt, stopV);
        if (dist <= Math.max(1.5, this.v * dt)) {
          this.x = T.x; this.y = T.y; this.size = T.size; this.v = 0;
          this.target = null; T.resolve(true);
        } else {
          const step = this.v * dt;
          this.x += (dx / dist) * step; this.y += (dy / dist) * step;
          this.size = lerp(T.size0, T.size, 1 - (dist - step) / T.d0);
          if (Math.abs(dx) > 2) this.facing = dx > 0 ? 1 : -1;
          moving = true;
        }
      } else {
        this.v = Math.max(0, this.v - 600 * dt);
      }
      const runTarget = moving ? clamp(this.v / (40 * s * 4), 0.25, 1) : 0;
      this.runW += (runTarget - this.runW) * Math.min(1, dt * 7);
      const sleepTarget = this.wantSleep && !moving && !this.jump && !this.leapT ? 1 : 0;
      this.sleepW += (sleepTarget - this.sleepW) * Math.min(1, dt * (sleepTarget ? 1.4 : 4));
      this.flip += (this.facing - this.flip) * Math.min(1, dt * 11);

      // ---- gait clock: the cycle length keeps planted paws from sliding
      const vUnits = this.v / Math.max(0.05, s);
      const stampede = this.mood === "stampede";
      const period = clamp(STRIDE / (STANCE * Math.max(1, vUnits)), stampede ? 0.2 : 0.26, 0.95);
      if (this.runW > 0.02) this.phase = (this.phase + dt / period) % 1;

      // ---- blink, twitch, jump
      this.blinkAt -= dt;
      if (this.blinkAt <= 0) { this.blink = 0.16; this.blinkAt = 2.5 + Math.random() * 4; }
      this.blink = Math.max(0, this.blink - dt);
      this.twitchAt -= dt;
      if (this.twitchAt <= 0) { this.twitch = 0.45; this.twitchAt = 2 + Math.random() * 5; }
      this.twitch = Math.max(0, this.twitch - dt);
      if (this.jump) {
        this.jump.t += dt;
        if (this.jump.t >= this.jump.dur) { this.jump = null; this.emitAll(); }
      }
      if (this.bubbleUntil && performance.now() > this.bubbleUntil) { this.bubble.classList.remove("on"); this.bubbleUntil = 0; }
      this.render();
    }

    emitAll() { if (this.onStep) for (const p of this.lastPaws || []) this.onStep(p.x, p.y, this.scale * 2.2); }

    render() {
      const r = this.r, s = this.scale, t = this.t;
      const run = this.runW, sleep = this.sleepW, stampede = this.mood === "stampede" ? 1 : 0;
      const ph = this.phase, tau = Math.PI * 2;

      // ---- body pose
      const bob = run * (stampede ? 9 : 6) * (0.5 - 0.5 * Math.cos(2 * tau * ph));
      const breathe = Math.sin(t * (sleep > 0.5 ? 1.6 : 2.4));
      const jumpY = this.jump ? -this.jump.h * Math.sin(Math.PI * (this.jump.t / this.jump.dur)) : 0;
      const air = this.air || 0;
      const squat = Math.max(this.crouch || 0, (this.land || 0) * 0.8);
      const bodyY = lerp(lerp(-72, -74 - bob, run), -38, sleep) + jumpY + squat * 16 - air * 6;
      const flightPitch = air > 0 ? lerp(-14, 12, this.airU || 0) * Math.min(1, air * 2) : 0;
      const pitch = run * -3.5 * Math.sin(tau * ph) + sleep * 2 + flightPitch;
      const bodyScaleY = 1 + breathe * (sleep > 0.5 ? 0.025 : 0.008);
      const cosP = Math.cos(rad(pitch)), sinP = Math.sin(rad(pitch));
      const toRoot = (p) => ({ x: p.x * cosP - p.y * bodyScaleY * sinP, y: p.x * sinP + p.y * bodyScaleY * cosP + bodyY });
      r.body.setAttribute("transform", `translate(0 ${f1(bodyY)}) rotate(${f1(pitch)}) scale(1 ${bodyScaleY.toFixed(3)})`);
      r.spikes.setAttribute("opacity", stampede);
      if (stampede) r.spikes.setAttribute("transform", `translate(0 ${f1(-2 * Math.abs(Math.sin(t * 30)))}) scale(1 ${(1 + 0.12 * Math.sin(t * 27)).toFixed(3)})`);

      // ---- head and ears
      const headRot = lerp(lerp(Math.sin(t * 0.7) * 3, -4 + 3 * Math.sin(2 * tau * ph + 1), run), 24, sleep);
      const headDrop = sleep * 20;
      r.head.setAttribute("transform", `translate(${NECK.x} ${f1(NECK.y + headDrop)}) rotate(${f1(headRot)})`);
      const tw = this.twitch > 0 ? Math.sin((this.twitch / 0.45) * Math.PI * 3) * 7 : 0;
      const earFlow = run * (-10 - 4 * Math.sin(2 * tau * ph));
      const nearEar = lerp(-18 + earFlow + tw, -62, sleep) + stampede * 14;
      const farEar = lerp(-3 + earFlow * 0.8 - tw * 0.5, -44, sleep) + stampede * 10;
      r.earNear.setAttribute("transform", `translate(12 -66) rotate(${f1(nearEar)})`);
      r.earFar.setAttribute("transform", `translate(36 -70) rotate(${f1(farEar)}) scale(0.93)`);

      // ---- eyes: open, blinking, or asleep
      const closed = sleep > 0.55;
      const lidOpen = closed ? 0.06 : this.blink > 0 ? 0.1 : 1;
      r.eye.setAttribute("transform", `translate(52 -44) scale(1 ${lidOpen}) translate(-52 44)`);
      r.lid.setAttribute("opacity", closed ? 1 : 0);
      r.lash.setAttribute("opacity", closed ? 0 : 1);

      // ---- legs, placed with IK
      const paws = [];
      LEGS.forEach((L, i) => {
        const d = L.hind ? LEG_DIM.hind : LEG_DIM.fore;
        const attach = L.hind ? HIP : SHOULDER;
        const joint = toRoot({ x: attach.x + (L.near ? 0 : 4), y: attach.y });
        const p = (ph + L.phase) % 1;
        let gx, gy;
        if (p < STANCE) { gx = STRIDE / 2 - STRIDE * (p / STANCE); gy = 0; }
        else { const u = (p - STANCE) / (1 - STANCE); gx = -STRIDE / 2 + STRIDE * easeInOut(u); gy = -(stampede ? 24 : 17) * Math.sin(Math.PI * u); }
        const restX = attach.x + (L.near ? 0 : 4);
        const stand = { x: restX + (L.hind ? 4 : -2), y: 0 };
        const gait = { x: restX + gx, y: gy };
        const nap = { x: restX + (L.hind ? 30 : 46), y: -2 };
        let foot = { x: lerp(stand.x, gait.x, run), y: lerp(stand.y, gait.y, run) };
        foot = { x: lerp(foot.x, nap.x, sleep), y: lerp(foot.y, nap.y, sleep) + jumpY * 0.55 };
        if (air > 0) {
          // Paws tucked in flight: hind legs trail, forelegs reach.
          foot = { x: lerp(foot.x, restX + (L.hind ? -26 : 30), air), y: lerp(foot.y, -34 + (L.hind ? 6 : 0), air) };
        }
        const k = ik(joint, foot, d.l1, d.l2, L.hind);
        const g = r.legs[L.name];
        // Navy socks on the lower half, like the artwork's dark paws.
        const sockTop = { x: lerp(k.knee.x, k.end.x, 0.5), y: lerp(k.knee.y, k.end.y, 0.5) };
        for (const [ln, a, b] of [[g.o1, joint, k.knee], [g.s1, joint, k.knee], [g.o2, k.knee, k.end], [g.s2, k.knee, k.end], [g.s3, sockTop, k.end]]) {
          ln.setAttribute("x1", f1(a.x)); ln.setAttribute("y1", f1(a.y)); ln.setAttribute("x2", f1(b.x)); ln.setAttribute("y2", f1(b.y));
        }
        const pawRot = run * (p < STANCE ? 0 : -25 * Math.sin(Math.PI * ((p - STANCE) / (1 - STANCE))));
        const pt = `translate(${f1(k.end.x + 5)} ${f1(k.end.y + 1)}) rotate(${f1(pawRot)})`;
        g.paw.setAttribute("transform", pt);
        g.toe.setAttribute("transform", pt);
        if (g.haunch) {
          const a = Math.atan2(k.knee.y - joint.y, k.knee.x - joint.x) * 180 / Math.PI - 90;
          g.haunch.setAttribute("transform", `translate(${f1(joint.x + (k.knee.x - joint.x) * 0.3)} ${f1(joint.y + (k.knee.y - joint.y) * 0.3 - 4)}) rotate(${f1(a)})`);
        }
        // Touchdown: the moment a paw starts its stance, it lights the ground.
        const prev = this.prevPhases[i];
        if (run > 0.55 && prev > p && !this.jump && !this.leapT) {
          const sx = this.x + (k.end.x + 5) * s * this.flip, sy = this.y;
          if (this.onStep) this.onStep(sx, sy, s * (stampede ? 2.4 : 2));
        }
        this.prevPhases[i] = p;
        paws.push({ x: this.x + (k.end.x + 5) * s * this.flip, y: this.y });
      });
      this.lastPaws = paws;

      // ---- tail: a spine that streams behind, sways, or curls round for a nap
      const base = toRoot(TAIL_BASE);
      const wave = (stampede ? 13 : 8) * run;
      let ang = 0, px = base.x, py = base.y;
      const pts = [{ x: px, y: py }];
      for (let i = 0; i < TAIL_N - 1; i++) {
        const standA = 206 + 14 * i + Math.sin(t * 1.3 - i * 0.5) * 4;
        const runA = 198 + 9 * i + wave * Math.sin(tau * ph - i * 0.75);
        const napA = 168 - 30 * i;
        let a = lerp(standA, runA, run);
        a = lerp(a, napA, sleep);
        ang = this.tail[i] === undefined ? a : this.tail[i] + (a - this.tail[i]) * 0.35;
        this.tail[i] = ang;
        px += Math.cos(rad(ang)) * TAIL_SEG; py += Math.sin(rad(ang)) * TAIL_SEG;
        py = Math.min(py, -5);
        pts.push({ x: px, y: py });
      }
      this.drawTail(pts, 1 + stampede * 0.2);

      // ---- shadow and placement on screen
      const lift = Math.max(Math.max(0, -jumpY) / 46, air);
      r.shadow.setAttribute("rx", f1(lerp(74, 84, sleep) * (1 - lift * 0.35)));
      r.shadow.setAttribute("opacity", (0.16 * (1 - lift * 0.8)).toFixed(3));
      r.root.setAttribute("transform", `scale(${this.flip.toFixed(3)} 1)`);
      const W = 520 * s, H = 330 * s;
      this.svg.style.width = W + "px"; this.svg.style.height = H + "px";
      this.svg.style.transform = `translate3d(${(this.x - 260 * s).toFixed(2)}px, ${(this.y - 290 * s).toFixed(2)}px, 0)`;
      // Overlays follow the head, unmirrored.
      const head = toRoot({ x: NECK.x + 20, y: NECK.y - 90 + headDrop });
      const hx = this.x + head.x * s * this.flip, hy = this.y + head.y * s;
      this.zzz.style.transform = `translate3d(${(hx + 10 * this.flip).toFixed(1)}px, ${(hy + 30 * s).toFixed(1)}px, 0)`;
      this.zzz.style.opacity = closed ? 1 : 0;
      this.bubble.style.transform = `translate3d(${hx.toFixed(1)}px, ${(hy - 6).toFixed(1)}px, 0) translate(-50%, -100%)`;
      if (this.bubble.classList.contains("on")) this.clampBubble(hx);
    }

    clampBubble(hx) {
      const w = this.bubble.offsetWidth;
      const left = hx - w / 2;
      const dx = left < 8 ? 8 - left : left + w > innerWidth - 8 ? innerWidth - 8 - (left + w) : 0;
      this.bubble.style.marginLeft = dx + "px";
    }

    drawTail(pts, puff) {
      const r = this.r;
      const S = spline(pts, TAIL_SAMPLES);
      const n = S.length;
      const width = (u) => {
        const w = u <= 0.78 ? 11 + 25 * Math.pow(Math.sin(Math.PI * u * 0.641), 0.8) : 36 * (1 - Math.pow((u - 0.78) / 0.22, 2) * 0.5);
        return w * puff;
      };
      let taper = false, j0t = 0, j1t = 0;
      const side = (j, sign, scaleW, jag) => {
        const u = j / (n - 1), p = S[j];
        let w = width(u) * scaleW * (taper ? 0.45 + 0.55 * smooth(j0t, j0t + 5, j) * smooth(j1t, j1t - 5, j) : 1);
        if (jag && u > 0.2 && j % 3 === 0) w += 2.2;
        return { x: p.x - p.ty * w * sign, y: p.y + p.tx * w * sign };
      };
      const outline = (j0, j1, scaleW, jag, cap, tp = false) => {
        taper = tp; j0t = j0; j1t = j1;
        let d = "";
        for (let j = j0; j <= j1; j++) { const q = side(j, 1, scaleW, jag); d += (j === j0 ? "M" : "L") + f1(q.x) + " " + f1(q.y); }
        const e = S[j1], we = width(j1 / (n - 1)) * scaleW;
        if (cap) {
          const a = side(j1, 1, scaleW, false), b = side(j1, -1, scaleW, false);
          d += `C${f1(a.x + e.tx * we * 1.2)} ${f1(a.y + e.ty * we * 1.2)} ${f1(b.x + e.tx * we * 1.2)} ${f1(b.y + e.ty * we * 1.2)} ${f1(b.x)} ${f1(b.y)}`;
        }
        for (let j = j1; j >= j0; j--) { const q = side(j, -1, scaleW, jag); d += "L" + f1(q.x) + " " + f1(q.y); }
        return d + "Z";
      };
      r.tailBody.setAttribute("d", outline(0, n - 1, 1, true, true));
      // Data panel: the navy band where the live line runs.
      const j0 = Math.round(0.3 * (n - 1)), j1 = Math.round(0.74 * (n - 1));
      r.tailPanel.setAttribute("d", outline(j0, j1, 0.74, false, false, true));
      taper = false;
      const jt = Math.round(0.77 * (n - 1));
      r.tailTip.setAttribute("d", outline(jt, n - 1, 0.86, true, true));
      // The live tokens/sec line, running along the tail.
      const vals = this.tailValues, m = vals.length, max = this.tailMax;
      let d = "", lastPt = null;
      const k0 = Math.round(0.33 * (n - 1)), k1 = Math.round(0.71 * (n - 1));
      for (let j = k0; j <= k1; j++) {
        const u = (j - k0) / (k1 - k0);
        const v = vals[Math.min(m - 1, Math.round(u * (m - 1)))] / max;
        const p = S[j], w = width(j / (n - 1)) * 0.72 * 0.62;
        const off = (0.5 - v) * 2 * w;
        const q = { x: p.x - p.ty * off, y: p.y + p.tx * off };
        d += (j === k0 ? "M" : "L") + f1(q.x) + " " + f1(q.y);
        lastPt = q;
      }
      for (const e of [r.tailGlow, r.tailMid, r.tailCore]) e.setAttribute("d", d);
      if (lastPt) { r.tailDot.setAttribute("cx", f1(lastPt.x)); r.tailDot.setAttribute("cy", f1(lastPt.y)); }
      for (const sq of r.tailSquares) {
        const j = Math.round(sq.t * (n - 1)), p = S[j], w = width(sq.t) * 0.72 * sq.side;
        sq.el.setAttribute("x", f1(p.x - p.ty * w - 3.5)); sq.el.setAttribute("y", f1(p.y + p.tx * w - 3.5));
      }
    }
  }

  let last = 0;
  function tick(now) {
    const dt = Math.min(0.05, Math.max(0, (now - last) / 1000));
    last = now;
    for (const f of all) f.update(dt);
    if (all.size) requestAnimationFrame(tick); else ticking = false;
  }

  // ------------------------------------------------------------ RGB lights
  // Each paw touchdown lights the ground in pure red, green or blue. Lights
  // add up where they overlap, like real light.
  const RGB = ["255,0,0", "0,255,0", "0,0,255"];
  let rgbIndex = 0;
  const lights = [];
  const particles = [];
  let canvas, ctx, dpr = 1, drawing = false;
  // On a light page colored light can't outshine the paper, so pools are
  // smaller and softer there; on a dark page they bloom fully.
  let dark = true;

  function ensureCanvas() {
    if (canvas) return;
    canvas = document.createElement("canvas");
    canvas.className = "pip-lights";
    canvas.setAttribute("aria-hidden", "true");
    document.body.append(canvas);
    ctx = canvas.getContext("2d");
    const size = () => { dpr = Math.min(2, devicePixelRatio || 1); canvas.width = innerWidth * dpr; canvas.height = innerHeight * dpr; };
    size(); addEventListener("resize", size);
  }

  function step(x, y, scale = 1, color) {
    ensureCanvas();
    lights.push({ x, y, s: scale, c: color || RGB[(rgbIndex = (rgbIndex + 1) % 3)], t0: performance.now() });
    if (lights.length > 70) lights.shift();
    kick();
  }

  function sparkle(x, y, n = 1, scale = 1) {
    ensureCanvas();
    const colors = [C.mint, C.lavLight, C.cream, C.lav];
    for (let i = 0; i < n; i++) {
      particles.push({ x: x + (Math.random() - 0.5) * 30 * scale, y, vx: (Math.random() - 0.5) * 24, vy: -(30 + Math.random() * 50) * scale,
        size: (3 + Math.random() * 5) * scale, c: colors[(Math.random() * colors.length) | 0], t0: performance.now(), life: 1000 + Math.random() * 700 });
    }
    if (particles.length > 160) particles.splice(0, particles.length - 160);
    kick();
  }

  function kick() { if (!drawing) { drawing = true; requestAnimationFrame(draw); } }

  const LIGHT_MS = 1900;
  function draw(now) {
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, innerWidth, innerHeight);
    ctx.globalCompositeOperation = "lighter";
    for (let i = lights.length - 1; i >= 0; i--) {
      const L = lights[i];
      const age = Math.max(0, (now - L.t0) / LIGHT_MS);
      if (age >= 1) { lights.splice(i, 1); continue; }
      const a = Math.min(1, age * 10) * Math.pow(1 - age, 1.7) * (dark ? 1 : 0.7);
      const R = (26 + 46 * Math.sqrt(age)) * L.s * (dark ? 1 : 0.72);
      ctx.save();
      ctx.translate(L.x, L.y);
      ctx.scale(1, 0.3);
      const g = ctx.createRadialGradient(0, 0, 0, 0, 0, R * 1.6);
      g.addColorStop(0, `rgba(${L.c},${0.95 * a})`);
      g.addColorStop(0.35, `rgba(${L.c},${0.42 * a})`);
      g.addColorStop(1, `rgba(${L.c},0)`);
      ctx.fillStyle = g;
      ctx.beginPath(); ctx.arc(0, 0, R * 1.6, 0, Math.PI * 2); ctx.fill();
      ctx.strokeStyle = `rgba(${L.c},${0.65 * Math.pow(1 - age, 2)})`;
      ctx.lineWidth = 4;
      ctx.beginPath(); ctx.arc(0, 0, R * (0.35 + age * 1.2), 0, Math.PI * 2); ctx.stroke();
      ctx.restore();
      if (!dark) continue;
      const aura = ctx.createRadialGradient(L.x, L.y - 14 * L.s, 0, L.x, L.y - 14 * L.s, R * 1.1);
      aura.addColorStop(0, `rgba(${L.c},${0.28 * a})`);
      aura.addColorStop(1, `rgba(${L.c},0)`);
      ctx.fillStyle = aura;
      ctx.beginPath(); ctx.arc(L.x, L.y - 14 * L.s, R * 1.1, 0, Math.PI * 2); ctx.fill();
    }
    ctx.globalCompositeOperation = "source-over";
    for (let i = particles.length - 1; i >= 0; i--) {
      const p = particles[i];
      const age = Math.max(0, (now - p.t0) / p.life);
      if (age >= 1) { particles.splice(i, 1); continue; }
      const tt = Math.max(0, now - p.t0) / 1000;
      const x = p.x + p.vx * tt + Math.sin(tt * 3 + p.size) * 6, y = p.y + p.vy * tt;
      ctx.globalAlpha = Math.min(1, (1 - age) * 1.4) * 0.9;
      ctx.fillStyle = p.c;
      ctx.beginPath();
      if (ctx.roundRect) ctx.roundRect(x - p.size / 2, y - p.size / 2, p.size, p.size, p.size / 4); else ctx.rect(x - p.size / 2, y - p.size / 2, p.size, p.size);
      ctx.fill();
    }
    ctx.globalAlpha = 1;
    if (lights.length || particles.length) requestAnimationFrame(draw);
    else { drawing = false; ctx.clearRect(0, 0, innerWidth, innerHeight); }
  }

  return {
    create: (opts) => new Fox(opts),
    step, sparkle, reduceMotion, HEIGHT,
    setDark(v) { dark = !!v; },
  };
})();
