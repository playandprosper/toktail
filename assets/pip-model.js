// SPDX-License-Identifier: Apache-2.0
// Pip the Token Fox, built in three.js.
//
// Pip is modeled from primitives in the style of the mascot artwork:
// toon shading in Pip's palette, ink outlines (inverted hulls), spiky fur
// bursts on the chest, cheeks and ears, anime eyes with eyelids, navy socks,
// and a bushy skinned tail whose side is a live throughput chart.
//
// Every moving part is a named node, so the clips in pip-clips.js can drive
// it through three.js's AnimationMixer.
import * as THREE from 'three';
import { mergeVertices } from '../vendor/BufferGeometryUtils.js';

export const PALETTE = {
  orange: 0xfc8939, orangeDeep: 0xe3612f, cream: 0xfcebd3, peach: 0xf9caa7,
  navy: 0x0d0f38, sock: 0x22205e, lav: 0x8e5cf2, lavLight: 0xca9cf6,
  mint: 0xcce5e6, white: 0xfefeff,
};

/* ------------------------------------------------------------------ materials */

let GRADIENT = null;
function gradientMap() {
  if (GRADIENT) return GRADIENT;
  // A soft cel ramp: three tones with gentle, slightly blurred steps.
  const N = 64, data = new Uint8Array(N);
  const ss = (a, b, x) => { const t = Math.min(1, Math.max(0, (x - a) / (b - a))); return t * t * (3 - 2 * t); };
  for (let i = 0; i < N; i++) {
    const x = i / (N - 1);
    data[i] = Math.round(255 * (0.5 + 0.26 * ss(0.22, 0.4, x) + 0.16 * ss(0.55, 0.72, x) + 0.08 * ss(0.85, 0.97, x)));
  }
  GRADIENT = new THREE.DataTexture(data, N, 1, THREE.RedFormat);
  GRADIENT.minFilter = GRADIENT.magFilter = THREE.LinearFilter;
  GRADIENT.generateMipmaps = false;
  GRADIENT.needsUpdate = true;
  return GRADIENT;
}

// One rim-light patch shared by every toon material, so they share a program.
function addRim(shader) {
  shader.uniforms.uRim = { value: new THREE.Color(0xb48cff) };
  shader.uniforms.uRimAmt = { value: 0.5 };
  shader.fragmentShader = 'uniform vec3 uRim;\nuniform float uRimAmt;\n' + shader.fragmentShader.replace(
    '#include <opaque_fragment>',
    'float pipRim = pow( 1.0 - saturate( dot( normal, normalize( vViewPosition ) ) ), 2.4 );\n' +
    'outgoingLight += uRim * pipRim * uRimAmt;\n#include <opaque_fragment>'
  );
}

// Painted fur: soft darker strokes that flow down the surface, multiplied
// into the toon colour so the shading keeps its hard bands.
let FUR = null;
export function furTexture() {
  if (FUR) return FUR;
  const W = 512, H = 512;
  const c = document.createElement('canvas');
  c.width = W; c.height = H;
  const g = c.getContext('2d');
  g.fillStyle = '#ffffff'; g.fillRect(0, 0, W, H);
  let seed = 7;
  const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
  g.lineCap = 'round';
  for (let i = 0; i < 2600; i++) {
    const x = rnd() * W, y = rnd() * H, len = 8 + rnd() * 22, bend = (rnd() - 0.5) * 8;
    g.strokeStyle = `rgba(70,28,48,${0.03 + rnd() * 0.07})`;
    g.lineWidth = 1 + rnd() * 2.2;
    g.beginPath(); g.moveTo(x, y); g.quadraticCurveTo(x + bend, y + len / 2, x + bend * 0.4, y + len); g.stroke();
  }
  FUR = new THREE.CanvasTexture(c);
  FUR.wrapS = FUR.wrapT = THREE.RepeatWrapping;
  FUR.repeat.set(3, 2);
  FUR.colorSpace = THREE.SRGBColorSpace;
  FUR.anisotropy = 4;
  return FUR;
}

const toonCache = new Map();
export function toon(color, opts = {}) {
  const key = color + JSON.stringify(opts);
  if (toonCache.has(key)) return toonCache.get(key);
  const m = new THREE.MeshToonMaterial({ color, gradientMap: gradientMap(), ...opts });
  m.onBeforeCompile = addRim;
  // Soft line art: each part is outlined in a deeper shade of its own colour.
  m.userData.ink = new THREE.Color(color).lerp(new THREE.Color(0x2a0f3a), 0.55).getHex();
  toonCache.set(key, m);
  return m;
}

const OUTLINES = new Map();
const outlineHull = (s) => {
  s.vertexShader = s.vertexShader.replace(
    '#include <begin_vertex>',
    'vec3 transformed = vec3( position ) + normalize( normal ) * 0.021;'
  );
};
function outlineMaterial(color = 0x3a1a3e) {
  if (OUTLINES.has(color)) return OUTLINES.get(color);
  const m = new THREE.MeshBasicMaterial({ color, side: THREE.BackSide });
  m.onBeforeCompile = outlineHull;
  OUTLINES.set(color, m);
  return m;
}

/* ------------------------------------------------------------------ geometry helpers */

// Averages normals of vertices that share a position, so seams don't split
// the shading or crack the outline.
function smoothNormals(geo) {
  geo.computeVertexNormals();
  const pos = geo.attributes.position, nor = geo.attributes.normal;
  const sums = new Map();
  const key = (i) => `${pos.getX(i).toFixed(4)},${pos.getY(i).toFixed(4)},${pos.getZ(i).toFixed(4)}`;
  for (let i = 0; i < pos.count; i++) {
    const k = key(i);
    const s = sums.get(k) || [0, 0, 0];
    s[0] += nor.getX(i); s[1] += nor.getY(i); s[2] += nor.getZ(i);
    sums.set(k, s);
  }
  for (let i = 0; i < pos.count; i++) {
    const s = sums.get(key(i));
    const l = Math.hypot(s[0], s[1], s[2]) || 1;
    nor.setXYZ(i, s[0] / l, s[1] / l, s[2] / l);
  }
  nor.needsUpdate = true;
  return geo;
}

function deformSphere(seg, fn) {
  const geo = new THREE.SphereGeometry(1, seg, Math.round(seg * 0.75));
  const p = geo.attributes.position, v = new THREE.Vector3();
  for (let i = 0; i < p.count; i++) {
    v.fromBufferAttribute(p, i);
    const o = fn(v.clone());
    p.setXYZ(i, o.x, o.y, o.z);
  }
  return smoothNormals(geo);
}

export function ellipsoid(rx, ry, rz, seg = 36, taper = 0) {
  // taper > 0 narrows the top (y+) and widens the bottom: a pear.
  return deformSphere(seg, (v) => {
    const t = 1 - taper * v.y;
    return new THREE.Vector3(v.x * rx * t, v.y * ry, v.z * rz * t);
  });
}

// A flattened fluff disk facing +z with spikes in its own plane, aimed at
// `dir` (radians, 0 = +x, PI/2 = up). This is the spiky fur in Pip's art.
export function burst(rx, ry, depth, { n = 8, amp = 0.35, sharp = 5, dir = -Math.PI / 2, spread = 1.4, seg = 56, phase = 0 } = {}) {
  return deformSphere(seg, (v) => {
    const az = Math.atan2(v.y, v.x);
    const aim = Math.pow(Math.max(0, Math.cos(az - dir)), spread);
    const rim = 1 - Math.abs(v.z);
    const w = (((n * (az - dir) + phase) / (Math.PI * 2)) % 1 + 1) % 1;
    const tri = 1 - Math.abs(w * 2 - 1);
    const s = amp * aim * rim * Math.pow(tri, sharp);
    return new THREE.Vector3(v.x * (1 + s) * rx, v.y * (1 + s) * ry, v.z * depth);
  });
}

function capsule(r, len, seg = 20) {
  const g = new THREE.CapsuleGeometry(r, len, 8, seg);
  g.translate(0, -len / 2, 0);
  return smoothNormals(g);
}

// A cupped leaf (Pip's ear), base at the origin, tip up the +y axis.
function leafShape(w, h, jag = 0) {
  const s = new THREE.Shape();
  const steps = 40;
  const pts = [];
  for (let i = 0; i <= steps; i++) {
    const t = i / steps; // 0 base right -> 1 tip
    const y = t * h;
    const f = t < 0.26 ? 0.72 + 0.28 * Math.sin((t / 0.26) * Math.PI / 2) : Math.pow(1 - (t - 0.26) / 0.74, 1.12);
    let x = (w / 2) * f;
    if (jag && t > 0.12 && t < 0.92) x *= 1 - jag * (i % 2);
    pts.push([x, y]);
  }
  s.moveTo(0, 0);
  pts.forEach(([x, y]) => s.lineTo(x, y));
  for (let i = steps; i >= 0; i--) {
    const [x, y] = pts[i];
    s.lineTo(-x * (jag ? 1 : 0.92), y);
  }
  s.lineTo(0, 0);
  return s;
}

function earLayer(w, h, depth, cup, jag = 0) {
  let g = new THREE.ExtrudeGeometry(leafShape(w, h, jag), {
    depth, bevelEnabled: true, bevelThickness: depth * 0.6, bevelSize: Math.min(0.03, w * 0.06), bevelSegments: 3, curveSegments: 6,
  });
  g.deleteAttribute('normal'); g.deleteAttribute('uv');
  g = mergeVertices(g, 1e-4);
  const p = g.attributes.position;
  for (let i = 0; i < p.count; i++) {
    const x = p.getX(i), y = p.getY(i), z = p.getZ(i);
    const k = x / (w * 0.5);
    p.setZ(i, z + cup * k * k * (0.35 + 0.65 * Math.min(1, y / h * 1.6)) - cup * 0.15 * (y / h));
  }
  return smoothNormals(g);
}

// A dome with planar UVs: eyes, eyelids.
function dome(rx, ry, bulge, seg = 48) {
  const g = new THREE.CircleGeometry(1, seg, 0, Math.PI * 2);
  // add rings so the dome can curve
  const rings = 8, pos = [], uv = [], idx = [];
  pos.push(0, 0, bulge); uv.push(0.5, 0.5);
  for (let r = 1; r <= rings; r++) {
    const rr = r / rings;
    for (let a = 0; a < seg; a++) {
      const t = (a / seg) * Math.PI * 2;
      const x = Math.cos(t) * rr, y = Math.sin(t) * rr;
      pos.push(x * rx, y * ry, bulge * Math.sqrt(Math.max(0, 1 - rr * rr)));
      uv.push(0.5 + x * 0.5, 0.5 + y * 0.5);
    }
  }
  for (let a = 0; a < seg; a++) idx.push(0, 1 + a, 1 + ((a + 1) % seg));
  for (let r = 1; r < rings; r++) {
    const b0 = 1 + (r - 1) * seg, b1 = 1 + r * seg;
    for (let a = 0; a < seg; a++) {
      const a1 = (a + 1) % seg;
      idx.push(b0 + a, b1 + a, b1 + a1, b0 + a, b1 + a1, b0 + a1);
    }
  }
  g.dispose();
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  geo.setAttribute('uv', new THREE.Float32BufferAttribute(uv, 2));
  geo.setIndex(idx);
  geo.computeVertexNormals();
  return geo;
}

function tube(points, r, segs = 24) {
  const c = new THREE.CatmullRomCurve3(points.map((p) => new THREE.Vector3(...p)));
  return new THREE.TubeGeometry(c, segs, r, 6, false);
}

/* ------------------------------------------------------------------ textures */

function eyeTexture() {
  const c = document.createElement('canvas');
  c.width = c.height = 256;
  const g = c.getContext('2d');
  g.beginPath(); g.arc(128, 128, 128, 0, Math.PI * 2); g.fillStyle = '#0d0f38'; g.fill();
  g.save();
  g.beginPath(); g.arc(128, 128, 116, 0, Math.PI * 2); g.clip();
  const lg = g.createLinearGradient(0, 70, 0, 256);
  lg.addColorStop(0, 'rgba(142,92,242,0)');
  lg.addColorStop(0.42, '#5d36c9');
  lg.addColorStop(0.8, '#a77bf6');
  lg.addColorStop(1, '#d9c0ff');
  g.fillStyle = lg; g.fillRect(0, 0, 256, 256);
  g.beginPath(); g.ellipse(128, 118, 50, 62, 0, 0, Math.PI * 2); g.fillStyle = 'rgba(6,6,30,.72)'; g.fill();
  g.restore();
  g.fillStyle = '#ffffff';
  g.beginPath(); g.ellipse(166, 78, 34, 42, -0.5, 0, Math.PI * 2); g.fill();
  g.beginPath(); g.arc(112, 56, 10, 0, Math.PI * 2); g.fill();
  g.fillStyle = '#cce5e6';
  g.beginPath(); g.ellipse(86, 178, 17, 13, 0.4, 0, Math.PI * 2); g.fill();
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  t.anisotropy = 4;
  return t;
}

export function glowTexture() {
  const c = document.createElement('canvas');
  c.width = c.height = 128;
  const g = c.getContext('2d');
  const r = g.createRadialGradient(64, 64, 0, 64, 64, 64);
  r.addColorStop(0, 'rgba(255,255,255,1)');
  r.addColorStop(0.25, 'rgba(255,255,255,.55)');
  r.addColorStop(1, 'rgba(255,255,255,0)');
  g.fillStyle = r; g.fillRect(0, 0, 128, 128);
  const t = new THREE.CanvasTexture(c);
  t.colorSpace = THREE.SRGBColorSpace;
  return t;
}

// The live chart on Pip's tail. draw() scrolls it; speed follows Pip's mood.
function chartScreen() {
  const W = 512, H = 256;
  const c = document.createElement('canvas');
  c.width = W; c.height = H;
  const g = c.getContext('2d');
  const tex = new THREE.CanvasTexture(c);
  tex.colorSpace = THREE.SRGBColorSpace;
  const squares = Array.from({ length: 9 }, (_, i) => ({ x: (i * 97) % W, y: 40 + ((i * 53) % 170), s: 10 + (i % 3) * 5, ph: i * 1.7 }));
  let offset = 0;
  const DEMO = [0.62, 0.42, 0.55, 0.3, 0.5, 0.22, 0.4, 0.58, 0.35, 0.18, 0.46, 0.28];
  let pts = DEMO;
  /** Live values for the tail screen (0..1 each, oldest first); null = the demo line. */
  function setValues(values) {
    pts = values && values.length >= 4 ? values.map((v) => Math.max(0, Math.min(1, 1 - v))) : DEMO;
  }
  function draw(t, dt, speed) {
    offset = (offset + dt * 70 * speed) % (W / 6 * pts.length);
    g.clearRect(0, 0, W, H);
    // dark screen with a soft edge
    const bg = g.createRadialGradient(W / 2, H / 2, 10, W / 2, H / 2, W * 0.55);
    bg.addColorStop(0, '#191b62'); bg.addColorStop(0.7, '#0d0f38'); bg.addColorStop(1, 'rgba(13,15,56,0)');
    g.fillStyle = bg; g.fillRect(0, 0, W, H);
    g.strokeStyle = 'rgba(202,156,246,.12)'; g.lineWidth = 2;
    for (let y = 48; y < H; y += 48) { g.beginPath(); g.moveTo(30, y); g.lineTo(W - 30, y); g.stroke(); }
    for (const q of squares) {
      g.globalAlpha = 0.35 + 0.45 * (0.5 + 0.5 * Math.sin(t * 2.2 + q.ph));
      g.fillStyle = q.ph % 2 > 1 ? '#fcebd3' : '#cce5e6';
      g.fillRect((q.x - offset * 0.4 + W) % W, q.y, q.s, q.s);
    }
    g.globalAlpha = 1;
    const step = W / 6;
    const path = [];
    for (let i = -1; i < pts.length + 7; i++) {
      const x = i * step - offset;
      path.push([x, H * (0.18 + 0.64 * pts[((i % pts.length) + pts.length) % pts.length])]);
    }
    const line = (w, col, blur) => {
      g.lineJoin = 'round'; g.lineCap = 'round'; g.lineWidth = w; g.strokeStyle = col;
      g.shadowColor = '#8e5cf2'; g.shadowBlur = blur;
      g.beginPath(); path.forEach(([x, y], i) => (i ? g.lineTo(x, y) : g.moveTo(x, y))); g.stroke();
    };
    line(22, 'rgba(142,92,242,.45)', 24);
    line(10, '#ca9cf6', 12);
    line(4, '#ffffff', 0);
    g.shadowBlur = 10; g.shadowColor = '#ffffff'; g.fillStyle = '#ffffff';
    for (const [x, y] of path) { if (x > -10 && x < W + 10) { g.beginPath(); g.arc(x, y, 9, 0, Math.PI * 2); g.fill(); } }
    g.shadowBlur = 0;
    tex.needsUpdate = true;
  }
  draw(0, 0, 1);
  return { texture: tex, draw, setValues };
}

/* ------------------------------------------------------------------ the tail */

const TAIL_BONES = 7;
function tailCurve() {
  return new THREE.CatmullRomCurve3([
    new THREE.Vector3(0, 0, 0),
    new THREE.Vector3(0, -0.05, -0.36),
    new THREE.Vector3(0.02, 0.2, -0.76),
    new THREE.Vector3(0.05, 0.64, -0.93),
    new THREE.Vector3(0.06, 1.08, -0.78),
    new THREE.Vector3(0.05, 1.33, -0.46),
    new THREE.Vector3(0.03, 1.4, -0.12),
  ], false, 'centripetal');
}
const tailRadius = (u) => (0.11 + 0.44 * Math.pow(Math.sin(Math.PI * Math.min(1, Math.pow(u, 0.8))), 0.8)) * (1 - 0.1 * u) + 0.015;
const PANEL = { u0: 0.22, u1: 0.7, a: -Math.PI / 2 - 1.05, half: 1.15 };
function inPanel(u, a) {
  if (u < PANEL.u0 || u > PANEL.u1) return 0;
  let d = Math.abs(((a - PANEL.a + Math.PI * 3) % (Math.PI * 2)) - Math.PI);
  return d < PANEL.half ? 1 : 0;
}

function tailGeometry({ panel = false } = {}) {
  const curve = tailCurve();
  const L = 90, R = panel ? 24 : 40;
  const frames = curve.computeFrenetFrames(L, false);
  const pos = [], col = [], uv = [], skinI = [], skinW = [], idx = [];
  const orange = new THREE.Color(PALETTE.orange), deep = new THREE.Color(PALETTE.orangeDeep), cream = new THREE.Color(PALETTE.cream);
  const tmp = new THREE.Color();
  const aRange = panel ? [PANEL.a - PANEL.half, PANEL.a + PANEL.half] : [0, Math.PI * 2];
  const uRange = panel ? [PANEL.u0, PANEL.u1] : [0, 1];
  for (let i = 0; i <= L; i++) {
    const u = uRange[0] + (uRange[1] - uRange[0]) * (i / L);
    const P = curve.getPointAt(u);
    const fi = Math.round(u * L);
    const N = frames.normals[fi], B = frames.binormals[fi];
    const tipClose = u > 0.97 ? Math.max(0.05, 1 - (u - 0.97) / 0.03) : 1;
    for (let j = 0; j <= R; j++) {
      const a = aRange[0] + (aRange[1] - aRange[0]) * (j / R);
      let r = tailRadius(u) * tipClose;
      if (panel) {
        r *= 1.02;
      } else if (!inPanel(u, a)) {
        const ramp = Math.min(1, u / 0.12);
        const spikes = Math.pow(0.5 + 0.5 * Math.cos(9 * a + u * 7), 5) * (0.55 + 0.45 * Math.pow(0.5 + 0.5 * Math.sin(u * 46 + a * 2), 3));
        r *= 1 + 0.26 * spikes * ramp;
      }
      const d = new THREE.Vector3().addScaledVector(N, Math.cos(a)).addScaledVector(B, Math.sin(a));
      const p = P.clone().addScaledVector(d, r);
      pos.push(p.x, p.y, p.z);
      // colours: cream tip with a jagged edge, darker underside, orange elsewhere
      const tipEdge = 0.76 + 0.035 * Math.sin(9 * a);
      if (u > tipEdge) tmp.copy(cream);
      else if (u < 0.2 + 0.03 * Math.sin(7 * a) && Math.cos(a) < -0.2) tmp.copy(cream);
      else tmp.copy(orange).lerp(deep, Math.max(0, -Math.sin(a)) * 0.45);
      col.push(tmp.r, tmp.g, tmp.b);
      uv.push((u - uRange[0]) / (uRange[1] - uRange[0]), (j / R));
      const f = u * (TAIL_BONES - 1);
      const b0 = Math.min(TAIL_BONES - 2, Math.floor(f));
      const w = f - b0;
      skinI.push(b0, b0 + 1, 0, 0);
      skinW.push(1 - w, w, 0, 0);
    }
  }
  for (let i = 0; i < L; i++) {
    for (let j = 0; j < R; j++) {
      const a = i * (R + 1) + j, b = a + R + 1;
      idx.push(a, a + 1, b, b, a + 1, b + 1);
    }
  }
  const geo = new THREE.BufferGeometry();
  geo.setAttribute('position', new THREE.Float32BufferAttribute(pos, 3));
  geo.setAttribute('color', new THREE.Float32BufferAttribute(col, 3));
  geo.setAttribute('uv', new THREE.Float32BufferAttribute(uv, 2));
  geo.setAttribute('skinIndex', new THREE.Uint16BufferAttribute(skinI, 4));
  geo.setAttribute('skinWeight', new THREE.Float32BufferAttribute(skinW, 4));
  geo.setIndex(idx);
  smoothNormals(geo);
  return geo;
}

function buildTail(chart) {
  const curve = tailCurve();
  const bones = [];
  let prev = new THREE.Vector3();
  for (let k = 0; k < TAIL_BONES; k++) {
    const b = new THREE.Bone();
    b.name = 'tail' + k;
    const p = curve.getPointAt(k / (TAIL_BONES - 1));
    b.position.copy(p).sub(prev);
    prev = p;
    if (k) bones[k - 1].add(b);
    bones.push(b);
  }
  const skeleton = new THREE.Skeleton(bones);
  const fur = new THREE.SkinnedMesh(tailGeometry(), toon(0xffffff, { vertexColors: true, map: furTexture() }));
  fur.add(bones[0]);
  fur.bind(skeleton);
  const hull = new THREE.SkinnedMesh(fur.geometry, outlineMaterial(0x7a3020));
  hull.bind(skeleton, fur.bindMatrix);
  fur.add(hull);
  const screen = new THREE.SkinnedMesh(tailGeometry({ panel: true }), new THREE.MeshBasicMaterial({ map: chart.texture, transparent: true, depthWrite: true }));
  screen.bind(skeleton, fur.bindMatrix);
  fur.add(screen);
  fur.frustumCulled = hull.frustumCulled = screen.frustumCulled = false;
  return fur;
}

/* ------------------------------------------------------------------ assembling Pip */

function part(geo, mat, parent, { pos, rot, outline = true, name } = {}) {
  const m = new THREE.Mesh(geo, mat);
  if (pos) m.position.set(...pos);
  if (rot) m.rotation.set(...rot);
  if (name) m.name = name;
  if (outline) m.add(new THREE.Mesh(geo, outlineMaterial(mat.userData.ink)));
  parent.add(m);
  return m;
}

function node(name, parent, pos = [0, 0, 0], rot = [0, 0, 0]) {
  const g = new THREE.Group();
  g.name = name;
  g.position.set(...pos);
  g.rotation.set(...rot);
  parent.add(g);
  return g;
}

export function buildPip() {
  const P = PALETTE;
  const fur = furTexture();
  const orange = toon(P.orange, { map: fur }), cream = toon(P.cream, { map: fur }), navy = toon(P.sock), lav = toon(P.lav), lavL = toon(P.lavLight);
  const earOrange = toon(P.orange), earCream = toon(P.cream);
  const ink = new THREE.MeshBasicMaterial({ color: P.navy });
  const white = new THREE.MeshBasicMaterial({ color: 0xffffff });
  const chart = chartScreen();

  const root = new THREE.Group();
  root.name = 'pip';
  const body = node('body', root); // bob/squash target for clips
  const hips = node('hips', body, [0, 0.8, -0.3]);
  part(ellipsoid(0.46, 0.44, 0.48, 40), orange, hips, { pos: [0, 0, -0.04] });
  part(burst(0.2, 0.2, 0.14, { n: 5, amp: 0.55, dir: -2.4, spread: 1.2 }), orange, hips, { pos: [0.33, -0.08, -0.12], rot: [0, 1.35, 0], outline: false });
  part(burst(0.2, 0.2, 0.14, { n: 5, amp: 0.55, dir: -0.74, spread: 1.2 }), orange, hips, { pos: [-0.33, -0.08, -0.12], rot: [0, -1.35, 0], outline: false });

  const spine = node('spine', hips, [0, 0.06, 0.12]);
  const chest = node('chest', spine);
  part(ellipsoid(0.38, 0.42, 0.52, 40), orange, chest, { pos: [0, 0.02, 0.42] });
  part(ellipsoid(0.22, 0.16, 0.34, 32), cream, chest, { pos: [0, -0.28, 0.3], outline: false });

  // neck, chest fluff and head
  const neck = node('neck', spine, [0, 0.22, 0.78]);
  part(burst(0.4, 0.34, 0.26, { n: 10, amp: 0.34, sharp: 1.6, dir: -Math.PI / 2, spread: 0.8, seg: 72 }), cream, neck, { pos: [0, -0.1, 0.14] });
  // `gaze` is where Pip looks (set by the page each frame); `head` is what the clips animate.
  const gaze = node('gaze', neck, [0, 0.42, 0.14]);
  const head = node('head', gaze);
  part(deformSphere(48, (v) => {
    const cheek = 1 + 0.12 * Math.max(0, -v.y + 0.2);
    return new THREE.Vector3(v.x * 0.52 * cheek, v.y * 0.47, v.z * 0.48);
  }), orange, head, { pos: [0, 0.27, 0.02] });
  // cheek fluff, cream inside with orange tufts outside
  part(burst(0.3, 0.22, 0.18, { n: 6, amp: 0.6, sharp: 1.5, dir: -0.5, spread: 1.3 }), orange, head, { pos: [0.36, 0.1, 0.05], rot: [0, 0.6, 0] });
  part(burst(0.3, 0.22, 0.18, { n: 6, amp: 0.6, sharp: 1.5, dir: Math.PI + 0.5, spread: 1.3 }), orange, head, { pos: [-0.36, 0.1, 0.05], rot: [0, -0.6, 0] });
  part(burst(0.25, 0.17, 0.15, { n: 5, amp: 0.5, sharp: 1.5, dir: -0.7, spread: 1.3, phase: 1 }), cream, head, { pos: [0.29, 0.05, 0.2], rot: [0, 0.45, 0], outline: false });
  part(burst(0.25, 0.17, 0.15, { n: 5, amp: 0.5, sharp: 1.5, dir: Math.PI + 0.7, spread: 1.3, phase: 1 }), cream, head, { pos: [-0.29, 0.05, 0.2], rot: [0, -0.45, 0], outline: false });
  // forehead tuft
  part(burst(0.16, 0.13, 0.12, { n: 5, amp: 0.5, sharp: 4, dir: Math.PI / 2, spread: 2 }), orange, head, { pos: [0, 0.66, 0.16], rot: [-0.5, 0, 0], outline: false });
  // muzzle, nose, mouth
  part(ellipsoid(0.24, 0.16, 0.2, 32), cream, head, { pos: [0, 0.14, 0.38] });
  part(ellipsoid(0.075, 0.052, 0.06, 20), toon(P.navy), head, { pos: [0, 0.22, 0.575], outline: false });
  part(new THREE.SphereGeometry(0.018, 10, 8), white, head, { pos: [0.02, 0.245, 0.62], outline: false });
  part(tube([[-0.11, 0.115, 0.5], [-0.06, 0.085, 0.53], [0, 0.11, 0.55], [0.06, 0.085, 0.53], [0.11, 0.115, 0.5]], 0.013), ink, head, { outline: false });
  part(tube([[0, 0.17, 0.57], [0, 0.11, 0.55]], 0.011, 4), ink, head, { outline: false });
  // tongue, for grooming and panting (hidden until a clip scales it up)
  const tongue = node('tongue', head, [0, 0.085, 0.53], [0.35, 0, 0]);
  tongue.scale.set(0.001, 0.001, 0.001);
  part(ellipsoid(0.06, 0.022, 0.1, 20), toon(0xff86ad), tongue, { pos: [0, 0, 0.07] });

  // eyes with eyelids
  const eyeTex = eyeTexture();
  const eyeMat = new THREE.MeshBasicMaterial({ map: eyeTex });
  const lidMat = toon(P.orange);
  const eyeGeo = dome(0.13, 0.17, 0.05);
  const lidGeo = dome(0.145, 0.186, 0.075);
  lidGeo.translate(0, -0.186, 0);
  [[1, 'L'], [-1, 'R']].forEach(([s, n]) => {
    const socket = node('eye' + n, head, [0.19 * s, 0.29, 0.425], [0, 0.42 * s, 0]);
    socket.rotateZ(-0.08 * s);
    part(eyeGeo, eyeMat, socket, { outline: false });
    const lid = node('lid' + n, socket, [0, 0.186, 0.004]);
    lid.scale.set(1, 0.001, 1);
    part(lidGeo, lidMat, lid, { outline: false });
    part(tube([[-0.135, -0.19, 0.06], [0, -0.25, 0.085], [0.135, -0.19, 0.06]], 0.015, 12), ink, lid, { outline: false });
    // lashes and brow
    part(tube([[-0.11 * s, 0.15, 0.04], [0.02 * s, 0.19, 0.05], [0.14 * s, 0.15, 0.03], [0.19 * s, 0.19, 0.0]], 0.017, 16), ink, socket, { outline: false });
    part(tube([[-0.08 * s, 0.28, 0.02], [0.03 * s, 0.31, 0.02], [0.13 * s, 0.29, 0.0]], 0.013, 12), ink, socket, { outline: false });
  });

  // ears: orange back, cream fringe, lavender middle, cream tufts at the base
  const earOuter = earLayer(0.74, 1.3, 0.05, 0.2);
  const earFringe = earLayer(0.6, 1.1, 0.03, 0.19, 0.22);
  const earInner = earLayer(0.42, 0.88, 0.03, 0.16);
  [[1, 'L'], [-1, 'R']].forEach(([s, n]) => {
    const ear = node('ear' + n, head, [0.24 * s, 0.56, -0.02], [-0.12, 0.3 * s, -0.78 * s]);
    part(earOuter, earOrange, ear);
    part(earFringe, earCream, ear, { pos: [0, 0.07, 0.05], outline: false });
    part(earInner, lav, ear, { pos: [0, 0.16, 0.085], outline: false });
    part(earInner, lavL, ear, { pos: [0, 0.36, 0.1], outline: false }).scale.set(0.55, 0.55, 1);
  });

  // front legs: orange shoulder, navy sock, paw with toes
  const toes = (paw) => [-0.055, 0, 0.055].forEach((x) => part(new THREE.SphereGeometry(0.045, 14, 10), navy, paw, { pos: [x, 0.0, 0.1], outline: false }));
  [[1, 'L'], [-1, 'R']].forEach(([s, n]) => {
    const arm = node('arm' + n, spine, [0.2 * s, -0.16, 0.66]);
    part(capsule(0.12, 0.2), orange, arm);
    const fore = node('fore' + n, arm, [0, -0.34, 0]);
    part(capsule(0.095, 0.24), navy, fore);
    const paw = node('fpaw' + n, fore, [0, -0.32, 0]);
    part(ellipsoid(0.11, 0.065, 0.14, 24), navy, paw, { pos: [0, 0, 0.04] });
    toes(paw);
  });
  // hind legs: orange thigh, navy shin, paw
  [[1, 'L'], [-1, 'R']].forEach(([s, n]) => {
    const thigh = node('thigh' + n, hips, [0.22 * s, -0.04, -0.02]);
    part(ellipsoid(0.18, 0.3, 0.24, 32), orange, thigh, { pos: [0.02 * s, -0.13, 0.03] });
    const shin = node('shin' + n, thigh, [0, -0.36, 0.02]);
    part(capsule(0.09, 0.24), navy, shin);
    const paw = node('hpaw' + n, shin, [0, -0.34, 0]);
    part(ellipsoid(0.11, 0.065, 0.15, 24), navy, paw, { pos: [0, 0, 0.05] });
    toes(paw);
  });

  // tail
  const tailRoot = node('tailRoot', hips, [0, 0.0, -0.4]);
  const tail = buildTail(chart);
  tailRoot.add(tail);

  root.traverse((o) => { if (o.isMesh) o.frustumCulled = false; });
  const nodes = {};
  root.traverse((o) => { if (o.name) nodes[o.name] = o; });
  return { root, nodes, chart };
}

