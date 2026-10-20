// SPDX-License-Identifier: Apache-2.0
// Pip's poses and animation clips, for three.js's AnimationMixer.
//
// A pose names a rotation (Euler XYZ, radians), position or scale for any of
// Pip's nodes; anything it leaves out stays at the rest pose. A clip is a list
// of timed poses, turned into Quaternion/Vector keyframe tracks so the mixer
// can play, loop and cross-fade them.
import * as THREE from 'three';

const HIDDEN = [0.001, 0.001, 0.001];

// Sitting like the artwork: head cocked, ears wide.
export const SIT = {
  body: { p: [0, 0, 0], s: [1, 1, 1] },
  hips: { p: [0, 0.5, -0.08], r: [-0.7, 0, 0] },
  spine: { r: [-0.28, 0, 0] },
  chest: { s: [1, 1, 1] },
  neck: { r: [0.56, 0, 0] },
  head: { r: [0.36, 0.12, 0.26] },
  armL: { r: [0.98, 0, 0.05] }, armR: { r: [0.98, 0, -0.05] },
  foreL: { r: [0, 0, 0] }, foreR: { r: [0, 0, 0] },
  fpawL: { r: [0, 0, 0] }, fpawR: { r: [0, 0, 0] },
  thighL: { r: [-0.78, 0, 0.32] }, thighR: { r: [-0.78, 0, -0.32] },
  shinL: { r: [2.1, 0, 0] }, shinR: { r: [2.1, 0, 0] },
  hpawL: { r: [-0.7, 0, 0] }, hpawR: { r: [-0.7, 0, 0] },
  tailRoot: { r: [0.55, -1.25, -0.1] },
  tail0: { r: [0, 0, 0] }, tail1: { r: [0.1, 0, 0.12] }, tail2: { r: [0.05, 0, 0.1] }, tail3: { r: [0, 0, 0.05] }, tail4: { r: [0, 0, 0] }, tail5: { r: [0.1, 0, 0] }, tail6: { r: [0, 0, 0] },
  earL: { r: [-0.12, 0.3, -0.78] }, earR: { r: [-0.12, -0.3, 0.78] },
  lidL: { s: [1, 0.001, 1] }, lidR: { s: [1, 0.001, 1] },
  tongue: { s: HIDDEN },
};

export const STAND = {
  body: { p: [0, 0, 0], s: [1, 1, 1] },
  hips: { p: [0, 0.8, -0.3], r: [0, 0, 0] },
  spine: { r: [0, 0, 0] },
  chest: { s: [1, 1, 1] },
  neck: { r: [-0.05, 0, 0] },
  head: { r: [0.12, 0, 0] },
  armL: { r: [0, 0, 0] }, armR: { r: [0, 0, 0] },
  foreL: { r: [0, 0, 0] }, foreR: { r: [0, 0, 0] },
  fpawL: { r: [0, 0, 0] }, fpawR: { r: [0, 0, 0] },
  thighL: { r: [0, 0, 0] }, thighR: { r: [0, 0, 0] },
  shinL: { r: [0, 0, 0] }, shinR: { r: [0, 0, 0] },
  hpawL: { r: [0, 0, 0] }, hpawR: { r: [0, 0, 0] },
  tailRoot: { r: [-0.35, 0, 0] },
  tail0: { r: [0, 0, 0] }, tail1: { r: [0, 0, 0] }, tail2: { r: [0, 0, 0] }, tail3: { r: [0, 0, 0] }, tail4: { r: [0, 0, 0] }, tail5: { r: [0, 0, 0] }, tail6: { r: [0, 0, 0] },
  earL: { r: [-0.12, 0.3, -0.78] }, earR: { r: [-0.12, -0.3, 0.78] },
  lidL: { s: [1, 0.001, 1] }, lidR: { s: [1, 0.001, 1] },
  tongue: { s: HIDDEN },
};

const merge = (base, over) => {
  const out = {};
  for (const k of Object.keys(base)) out[k] = { ...base[k] };
  for (const [k, v] of Object.entries(over || {})) out[k] = { ...(out[k] || {}), ...v };
  return out;
};

// Lying on the line: belly down, front paws out, head up and alert.
export const LIE = merge(STAND, {
  hips: { p: [0, 0.4, -0.3], r: [0.04, 0, 0] }, spine: { r: [0.06, 0, 0] },
  neck: { r: [0.05, 0, 0] }, head: { r: [0.1, 0.1, 0.24] },
  armL: { r: [-1.45, 0, 0.12] }, armR: { r: [-1.45, 0, -0.12] }, foreL: { r: [0.12, 0, 0] }, foreR: { r: [0.12, 0, 0] }, fpawL: { r: [1.35, 0, 0] }, fpawR: { r: [1.35, 0, 0] },
  thighL: { r: [-1.3, 0, 0.38] }, thighR: { r: [-1.3, 0, -0.38] }, shinL: { r: [2.4, 0, 0] }, shinR: { r: [2.4, 0, 0] }, hpawL: { r: [-1.0, 0, 0] }, hpawR: { r: [-1.0, 0, 0] },
  tailRoot: { r: [0.85, -1.35, 0.15] }, tail1: { r: [0.35, 0.2, 0] }, tail2: { r: [0.3, 0.2, 0] }, tail3: { r: [0.2, 0.15, 0] },
});

export function applyPose(nodes, pose) {
  for (const [name, t] of Object.entries(pose)) {
    const n = nodes[name];
    if (!n) continue;
    if (t.r) n.rotation.set(t.r[0], t.r[1], t.r[2]);
    if (t.p) n.position.set(t.p[0], t.p[1], t.p[2]);
    if (t.s) n.scale.set(t.s[0], t.s[1], t.s[2]);
  }
}

// keys: [{ t, pose }] where every pose is a full pose (built with merge).
function clip(name, duration, keys) {
  const names = new Set();
  keys.forEach((k) => Object.keys(k.pose).forEach((n) => names.add(n)));
  const tracks = [];
  const e = new THREE.Euler(), q = new THREE.Quaternion();
  const times = keys.map((k) => k.t);
  for (const n of names) {
    const has = (f) => keys.some((k) => k.pose[n] && k.pose[n][f]);
    if (has('r')) {
      const vals = [];
      let prev = null;
      keys.forEach((k) => {
        const r = (k.pose[n] && k.pose[n].r) || [0, 0, 0];
        q.setFromEuler(e.set(r[0], r[1], r[2]));
        if (prev && prev.dot(q) < 0) q.set(-q.x, -q.y, -q.z, -q.w);
        prev = q.clone();
        vals.push(q.x, q.y, q.z, q.w);
      });
      tracks.push(new THREE.QuaternionKeyframeTrack(n + '.quaternion', times, vals));
    }
    if (has('p')) tracks.push(new THREE.VectorKeyframeTrack(n + '.position', times, keys.flatMap((k) => (k.pose[n] && k.pose[n].p) || [0, 0, 0])));
    if (has('s')) tracks.push(new THREE.VectorKeyframeTrack(n + '.scale', times, keys.flatMap((k) => (k.pose[n] && k.pose[n].s) || [1, 1, 1])));
  }
  return new THREE.AnimationClip(name, duration, tracks);
}

// sample a pose function f(t, phase) into a looping or one-shot clip
function sampled(name, D, steps, f) {
  const keys = [];
  for (let i = 0; i <= steps; i++) {
    const t = (i / steps) * D;
    keys.push({ t, pose: f(t, (t / D) * Math.PI * 2) });
  }
  return clip(name, D, keys);
}

const BLINK = { lidL: { s: [1, 1, 1] }, lidR: { s: [1, 1, 1] } };
const SQUINT = (k) => ({ lidL: { s: [1, k, 1] }, lidR: { s: [1, k, 1] } });
const TONGUE = (k, len = 1) => ({ tongue: { s: [k, k, k * len] } });
const tailWave = (amp, phase, base = SIT) => {
  const o = {};
  for (let k = 1; k <= 6; k++) {
    const b = base['tail' + k].r;
    o['tail' + k] = { r: [b[0], b[1] + amp * Math.sin(phase - k * 0.55) * (0.5 + k * 0.12), b[2] + amp * 0.35 * Math.cos(phase - k * 0.55)] };
  }
  return o;
};
const ears = (x, z) => ({ earL: { r: [x, 0.3, -z] }, earR: { r: [x, -0.3, z] } });

export function buildClips() {
  const clips = {};

  // Sitting: breathing, tail swaying, an ear twitch, two blinks, a head that
  // tips from side to side like the artwork.
  clips.sit = sampled('sit', 4.8, 24, (t, ph) => {
    let p = merge(SIT, tailWave(0.16, ph));
    p = merge(p, { chest: { s: [1 + 0.02 * Math.sin(ph * 2), 1 + 0.035 * Math.sin(ph * 2), 1] } });
    p = merge(p, { head: { r: [0.36 + 0.04 * Math.sin(ph), 0.12 + 0.08 * Math.sin(ph * 0.5), 0.26 + 0.07 * Math.sin(ph)] } });
    if (Math.abs(t - 3.0) < 0.21) p = merge(p, { earR: { r: [-0.12, -0.3, 0.98] } });
    if (Math.abs(t - 1.4) < 0.11 || Math.abs(t - 4.2) < 0.11) p = merge(p, BLINK);
    return p;
  });

  // Trot: diagonal leg pairs, a bob, a wagging tail.
  const gait = (name, D, stride, bob, extra) => sampled(name, D, 16, (t, ph) => {
    const a = Math.sin(ph), b = Math.sin(ph + Math.PI);
    let p = merge(STAND, {
      body: { p: [0, bob * Math.abs(Math.sin(ph)), 0], s: [1, 1, 1] },
      hips: { p: [0, 0.8, -0.3], r: [0.04 * Math.sin(ph * 2), 0, 0] },
      armL: { r: [stride * a, 0, 0] }, foreL: { r: [Math.max(0, -0.9 * Math.cos(ph)), 0, 0] },
      armR: { r: [stride * b, 0, 0] }, foreR: { r: [Math.max(0, 0.9 * Math.cos(ph)), 0, 0] },
      thighL: { r: [stride * 0.9 * b, 0, 0] }, shinL: { r: [0.2 - Math.max(0, 0.7 * Math.cos(ph)), 0, 0] },
      thighR: { r: [stride * 0.9 * a, 0, 0] }, shinR: { r: [0.2 - Math.max(0, -0.7 * Math.cos(ph)), 0, 0] },
      head: { r: [0.12 + 0.05 * Math.sin(ph * 2), 0, 0] },
      ...ears(-0.22, 0.78),
    });
    p = merge(p, tailWave(0.25, ph, STAND));
    return extra ? merge(p, extra(t, ph)) : p;
  });
  clips.trot = gait('trot', 0.5, 0.55, 0.05);
  // Zoomies: a stretched-out gallop, ears back, tongue flapping.
  clips.run = gait('run', 0.3, 0.95, 0.12, (t, ph) => ({
    hips: { p: [0, 0.78, -0.3], r: [0.14 * Math.sin(ph), 0, 0] },
    spine: { r: [-0.1 * Math.sin(ph), 0, 0] },
    ...ears(0.5, 0.95),
    tailRoot: { r: [0.15, 0, 0] },
    ...TONGUE(1, 1.2),
  }));

  // Hop: crouch, launch with paws tucked, stretch, land and squash.
  {
    const crouch = merge(SIT, { body: { p: [0, -0.06, 0], s: [1.06, 0.88, 1.06] }, hips: { p: [0, 0.42, -0.1], r: [-0.5, 0, 0] }, ...ears(0.15, 0.6) });
    const fly = merge(STAND, {
      body: { p: [0, 0, 0], s: [0.94, 1.1, 0.94] }, hips: { p: [0, 0.8, -0.3], r: [-0.35, 0, 0] },
      armL: { r: [-0.9, 0, 0] }, armR: { r: [-0.9, 0, 0] }, foreL: { r: [1.2, 0, 0] }, foreR: { r: [1.2, 0, 0] },
      thighL: { r: [0.9, 0, 0.1] }, thighR: { r: [0.9, 0, -0.1] }, shinL: { r: [-0.6, 0, 0] }, shinR: { r: [-0.6, 0, 0] },
      tailRoot: { r: [0.1, 0, 0] }, ...ears(0.35, 0.55), head: { r: [-0.1, 0, 0] },
    });
    const land = merge(SIT, { body: { p: [0, -0.05, 0], s: [1.1, 0.85, 1.1] }, ...ears(-0.3, 0.6) });
    clips.hop = clip('hop', 0.9, [
      { t: 0, pose: merge(SIT, {}) }, { t: 0.14, pose: crouch }, { t: 0.32, pose: fly }, { t: 0.62, pose: fly },
      { t: 0.76, pose: land }, { t: 0.9, pose: merge(SIT, {}) },
    ]);
  }

  // Sleep: curled up on the line, head on paws, tail wrapped round.
  {
    const SLEEP = merge(LIE, {
      hips: { p: [0, 0.42, -0.3], r: [0.05, 0, 0] },
      neck: { r: [0.55, 0.25, 0] }, head: { r: [0.35, 0.2, 0.25] },
      tailRoot: { r: [0.9, -1.6, 0.2] }, tail1: { r: [0.6, 0.3, 0] }, tail2: { r: [0.5, 0.3, 0] }, tail3: { r: [0.4, 0.25, 0] }, tail4: { r: [0.2, 0.2, 0] },
      ...ears(-0.6, 0.9), ...BLINK,
    });
    clips.sleep = sampled('sleep', 3.6, 12, (t, ph) => merge(SLEEP, {
      chest: { s: [1 + 0.035 * Math.sin(ph), 1 + 0.05 * Math.sin(ph), 1] }, body: { p: [0, 0.01 * Math.sin(ph), 0], s: [1, 1, 1] },
    }));
  }

  // Loafing on the line: lying down, alert, tail swishing, looking about.
  clips.loaf = sampled('loaf', 5.2, 26, (t, ph) => {
    let p = merge(LIE, {
      chest: { s: [1 + 0.02 * Math.sin(ph * 2), 1 + 0.03 * Math.sin(ph * 2), 1] },
      head: { r: [0.1 + 0.05 * Math.sin(ph), 0.1 + 0.35 * Math.sin(ph), 0.24 + 0.06 * Math.sin(ph * 2)] },
      tail4: { r: [0.1, 0.4 * Math.sin(ph * 2), 0] }, tail5: { r: [0.1, 0.5 * Math.sin(ph * 2 - 0.6), 0] }, tail6: { r: [0, 0.5 * Math.sin(ph * 2 - 1.2), 0] },
    });
    if (Math.abs(t - 2.2) < 0.11) p = merge(p, BLINK);
    if (Math.abs(t - 3.9) < 0.2) p = merge(p, { earL: { r: [-0.12, 0.3, -0.98] } });
    return p;
  });

  // Batting at the panel like a cat: lying on the edge, a paw comes up and
  // swipes down over the edge, twice.
  {
    const watch = merge(LIE, { neck: { r: [0.35, 0, 0] }, head: { r: [0.42, -0.1, 0.15] }, ...ears(-0.4, 0.62) });
    const up = (x) => merge(watch, { armL: { r: [-2.7, 0, 0.3] }, foreL: { r: [-0.7, 0, 0] }, fpawL: { r: [0.4, 0, 0] }, tail5: { r: [0.3, 0.5, 0] }, tail6: { r: [0.2, 0.6, 0] }, body: { p: [0, 0, 0], s: [1, 1, 1], r: [0, 0, -x] } });
    const swipe = merge(watch, { armL: { r: [-0.35, 0, 0.12] }, foreL: { r: [0.3, 0, 0] }, fpawL: { r: [0.9, 0, 0] }, head: { r: [0.55, 0.1, 0.1] }, tail5: { r: [0.3, -0.5, 0] }, tail6: { r: [0.2, -0.6, 0] } });
    clips.bat = clip('bat', 1.9, [
      { t: 0, pose: merge(LIE, {}) }, { t: 0.25, pose: watch }, { t: 0.5, pose: up(0.04) }, { t: 0.66, pose: swipe },
      { t: 0.86, pose: up(0.03) }, { t: 1.0, pose: swipe }, { t: 1.12, pose: merge(swipe, { fpawL: { r: [1.2, 0, 0] } }) },
      { t: 1.5, pose: watch }, { t: 1.9, pose: merge(LIE, {}) },
    ]);
  }

  // Shaking out the mane: head and ears whip side to side, eyes shut.
  clips.shake = sampled('shake', 1.15, 28, (t) => {
    const a = t < 1.0 ? 0.55 * Math.pow(1 - t / 1.0, 1.2) * Math.min(1, t / 0.08) : 0;
    const w = Math.PI * 2 * 6.5 * t;
    return merge(SIT, {
      spine: { r: [-0.28, 0, 0.18 * a * Math.sin(w - 0.3)] },
      neck: { r: [0.56, 0, a * Math.sin(w)] },
      head: { r: [0.36, 0.12, 0.26 + 0.9 * a * Math.sin(w - 0.6)] },
      earL: { r: [-0.12, 0.3, -0.78 + 1.3 * a * Math.sin(w - 1.4)] }, earR: { r: [-0.12, -0.3, 0.78 + 1.3 * a * Math.sin(w - 1.4)] },
      ...tailWave(0.6 * a, w * 0.5),
      ...(t > 0.06 && t < 1.0 ? BLINK : {}),
    });
  });

  // Licking a paw: right paw up to the mouth, four licks, paw down.
  {
    const raised = (lick, k) => merge(SIT, {
      armL: { r: [-1.45, -0.1, -0.5] }, foreL: { r: [-1.55, 0, 0] }, fpawL: { r: [-0.6, 0, -0.2] },
      neck: { r: [0.62, 0, 0] }, head: { r: [0.6 + 0.14 * lick, 0.45, 0.1] },
      ...SQUINT(0.55), ...TONGUE(k, 1 + 0.4 * lick), ...ears(-0.25, 0.7),
    });
    const keys = [{ t: 0, pose: merge(SIT, {}) }, { t: 0.45, pose: raised(0, 0.001) }];
    for (let i = 0; i < 4; i++) {
      const t0 = 0.6 + i * 0.42;
      keys.push({ t: t0, pose: raised(0, 0.6) }, { t: t0 + 0.18, pose: raised(1, 1) }, { t: t0 + 0.36, pose: raised(0, 0.3) });
    }
    keys.push({ t: 2.45, pose: raised(0, 0.001) }, { t: 2.9, pose: merge(SIT, BLINK) }, { t: 3.1, pose: merge(SIT, {}) });
    clips.lick = clip('lick', 3.1, keys);
  }

  // Play bow and yawn: up, front down and bum up, a big yawn, back to sitting.
  {
    const bow = (yawn) => merge(STAND, {
      hips: { p: [0, 0.98, -0.3], r: [0.62, 0, 0] }, spine: { r: [0.32, 0, 0] }, neck: { r: [-0.35, 0, 0] }, head: { r: [-0.45 - 0.2 * yawn, 0, 0.15] },
      armL: { r: [-1.55, 0, 0.1] }, armR: { r: [-1.55, 0, -0.1] }, foreL: { r: [0.1, 0, 0] }, foreR: { r: [0.1, 0, 0] }, fpawL: { r: [0.6, 0, 0] }, fpawR: { r: [0.6, 0, 0] },
      thighL: { r: [-0.62, 0, 0] }, thighR: { r: [-0.62, 0, 0] }, hpawL: { r: [0.3, 0, 0] }, hpawR: { r: [0.3, 0, 0] },
      tailRoot: { r: [-1.05, 0, 0] }, tail2: { r: [-0.2, 0, 0] }, tail4: { r: [0.3, 0, 0] },
      ...SQUINT(0.4 + 0.6 * yawn), ...TONGUE(0.001 + 0.8 * yawn, 0.8), ...ears(0.2, 0.9),
    });
    clips.stretch = clip('stretch', 2.6, [
      { t: 0, pose: merge(SIT, {}) }, { t: 0.4, pose: merge(STAND, {}) }, { t: 0.85, pose: bow(0) }, { t: 1.25, pose: bow(1) },
      { t: 1.65, pose: bow(1) }, { t: 1.9, pose: bow(0) }, { t: 2.2, pose: merge(STAND, {}) }, { t: 2.6, pose: merge(SIT, {}) },
    ]);
  }

  // Fox mousing: a low crouch with the bum wiggling, then a high pounce that
  // comes down front paws first.
  const CROUCH = (wig) => merge(STAND, {
    hips: { p: [0, 0.52, -0.3], r: [0.26, 0, 0.13 * wig] }, spine: { r: [0.16, 0, -0.06 * wig] },
    armL: { r: [-0.45, 0, 0] }, armR: { r: [-0.45, 0, 0] }, foreL: { r: [0.85, 0, 0] }, foreR: { r: [0.85, 0, 0] }, fpawL: { r: [-0.25, 0, 0] }, fpawR: { r: [-0.25, 0, 0] },
    thighL: { r: [0.5, 0, 0] }, thighR: { r: [0.5, 0, 0] }, shinL: { r: [-0.9, 0, 0] }, shinR: { r: [-0.9, 0, 0] }, hpawL: { r: [0.4, 0, 0] }, hpawR: { r: [0.4, 0, 0] },
    neck: { r: [-0.2, 0, 0] }, head: { r: [-0.1, 0, 0.1] }, ...ears(-0.42, 0.55),
    tailRoot: { r: [0.15, 0.3 * wig, 0] }, tail4: { r: [0, 0.4 * wig, 0] }, tail6: { r: [0, 0.6 * wig, 0] },
  });
  clips.crouch = sampled('crouch', 0.44, 12, (t, ph) => CROUCH(Math.sin(ph)));
  clips.pounce = clip('pounce', 0.9, [
    { t: 0, pose: CROUCH(0) },
    { t: 0.18, pose: merge(STAND, { hips: { p: [0, 0.82, -0.3], r: [-0.65, 0, 0] }, armL: { r: [-1.7, 0, 0] }, armR: { r: [-1.7, 0, 0] }, thighL: { r: [0.9, 0, 0] }, thighR: { r: [0.9, 0, 0] }, shinL: { r: [-0.4, 0, 0] }, shinR: { r: [-0.4, 0, 0] }, ...ears(0.2, 0.6), tailRoot: { r: [0.3, 0, 0] } }) },
    { t: 0.45, pose: merge(STAND, { hips: { p: [0, 0.82, -0.3], r: [0.95, 0, 0] }, armL: { r: [-0.6, 0, 0] }, armR: { r: [-0.6, 0, 0] }, foreL: { r: [0.2, 0, 0] }, foreR: { r: [0.2, 0, 0] }, neck: { r: [0.3, 0, 0] }, head: { r: [0.35, 0, 0] }, tailRoot: { r: [-1.2, 0, 0] }, thighL: { r: [0.5, 0, 0] }, thighR: { r: [0.5, 0, 0] }, ...ears(-0.4, 0.5) }) },
    { t: 0.62, pose: merge(STAND, { body: { p: [0, -0.04, 0], s: [1.08, 0.88, 1.08] }, hips: { p: [0, 0.74, -0.3], r: [0.4, 0, 0] }, armL: { r: [0.25, 0, 0] }, armR: { r: [0.25, 0, 0] }, head: { r: [0.4, 0, 0] }, tailRoot: { r: [-0.9, 0, 0] } }) },
    { t: 0.9, pose: merge(STAND, { ...SQUINT(0.5) }) },
  ]);

  // Panting after zoomies: tongue out, fast breathing, happy squint.
  clips.pant = sampled('pant', 0.9, 12, (t, ph) => merge(SIT, {
    chest: { s: [1 + 0.03 * Math.sin(ph * 3), 1 + 0.05 * Math.sin(ph * 3), 1] },
    head: { r: [0.22 + 0.04 * Math.sin(ph * 3), 0.12, 0.26] },
    ...SQUINT(0.45), ...TONGUE(1, 1.25 + 0.15 * Math.sin(ph * 3)),
    ...tailWave(0.3, ph * 2),
  }));

  // Stampede in place: bouncing, ears up, tail going wild.
  clips.excited = sampled('excited', 0.4, 10, (t, ph) => merge(merge(SIT, {
    body: { p: [0.015 * Math.sin(ph * 3), 0.06 * Math.abs(Math.sin(ph)), 0], s: [1, 1 + 0.03 * Math.abs(Math.sin(ph)), 1] },
    ...ears(0.15, 0.6),
    head: { r: [0.3, 0.1 * Math.sin(ph), 0.26 + 0.12 * Math.sin(ph)] },
    armL: { r: [0.98 - 0.25 * Math.max(0, Math.sin(ph)), 0, 0.05] }, armR: { r: [0.98 - 0.25 * Math.max(0, -Math.sin(ph)), 0, -0.05] },
  }), tailWave(0.5, ph * 2)));

  // Startled by a click: ears pinned, a jump straight up (the page lifts Pip), a squashy landing.
  {
    const pinned = merge(SIT, { body: { p: [0, -0.05, 0], s: [1.1, 0.86, 1.1] }, ...ears(0.55, 0.95), head: { r: [0.2, 0, 0] } });
    const up = merge(SIT, {
      body: { p: [0, 0, 0], s: [0.92, 1.12, 0.92] }, ...ears(0.6, 0.9), head: { r: [0.1, 0, 0] },
      armL: { r: [0.3, 0, 0.3] }, armR: { r: [0.3, 0, -0.3] }, foreL: { r: [0.9, 0, 0] }, foreR: { r: [0.9, 0, 0] },
      tailRoot: { r: [0.9, -1.1, -0.1] },
    });
    const land = merge(SIT, { body: { p: [0, -0.04, 0], s: [1.1, 0.88, 1.1] }, ...ears(-0.3, 0.6) });
    clips.startle = clip('startle', 0.8, [
      { t: 0, pose: merge(SIT, {}) }, { t: 0.08, pose: pinned }, { t: 0.24, pose: up }, { t: 0.46, pose: up },
      { t: 0.58, pose: land }, { t: 0.8, pose: merge(SIT, {}) },
    ]);
  }

  // An ear flick: mildly annoyed.
  clips.flick = clip('flick', 0.5, [
    { t: 0, pose: merge(SIT, {}) },
    { t: 0.1, pose: merge(SIT, { earL: { r: [-0.12, 0.3, -1.3] }, head: { r: [0.36, 0.12, 0.4] } }) },
    { t: 0.22, pose: merge(SIT, { earL: { r: [-0.12, 0.3, -0.6] } }) },
    { t: 0.34, pose: merge(SIT, { earL: { r: [-0.12, 0.3, -1.2] } }) },
    { t: 0.5, pose: merge(SIT, {}) },
  ]);

  // Sulking: sitting up straight with its back turned, ears half down, tail twitching.
  clips.sulk = sampled('sulk', 4, 16, (t, ph) => merge(SIT, {
    head: { r: [0.5, 0, 0] }, ...ears(-0.35, 0.5),
    chest: { s: [1 + 0.02 * Math.sin(ph * 2), 1 + 0.03 * Math.sin(ph * 2), 1] },
    ...tailWave(0.12, ph * 1.5),
  }));

  // Poked: a happy squint and a little bounce.
  {
    const happy = (y, sq) => merge(SIT, { body: { p: [0, y, 0], s: [1 + sq, 1 - sq, 1 + sq] }, ...SQUINT(0.6), head: { r: [0.3, 0.12, 0.4] }, ...ears(0.1, 0.95) });
    clips.happy = clip('happy', 0.9, [
      { t: 0, pose: merge(SIT, {}) }, { t: 0.12, pose: happy(-0.03, 0.08) }, { t: 0.35, pose: merge(happy(0.28, -0.05), tailWave(0.45, 1)) },
      { t: 0.55, pose: merge(happy(0, 0.1), tailWave(0.45, 3)) }, { t: 0.72, pose: merge(happy(0, 0), tailWave(0.3, 5)) }, { t: 0.9, pose: merge(SIT, {}) },
    ]);
  }
  return clips;
}

// Clips that play once; the rest loop.
export const ONE_SHOT = new Set(['hop', 'happy', 'bat', 'shake', 'lick', 'stretch', 'pounce', 'startle', 'flick']);

