// SPDX-License-Identifier: Apache-2.0
// Pip in three dimensions: the same fox as toktail.dev, rendered with
// three.js on one transparent WebGL layer over the portal.
//
// The SVG fox in pip.js stays the brain: it decides where Pip is, how fast
// it runs, when it sleeps, hops or leaps, and where its speech bubble goes.
// This module is the skin. Each frame it reads that state and poses the 3D
// model to match: trot and run gaits while moving, the hop clip for jumps
// and leaps, sleep when napping, sitting otherwise; the live tokens/sec
// chart plays on the screen in Pip's tail. The SVG itself turns invisible
// but stays in the page as the click and keyboard target.
//
// Without WebGL, or with reduced motion, nothing here runs and the SVG fox
// shows as before.
import * as THREE from 'three';
import { buildPip, glowTexture } from './pip-model.js';
import { buildClips, ONE_SHOT, SIT, applyPose } from './pip-clips.js';

const FACE_VIEWER = -0.32;
const DT_MAX = 1 / 20;

function start() {
  if (typeof Pip === 'undefined' || typeof Pip.setSkin !== 'function') return;
  if (matchMedia('(prefers-reduced-motion: reduce)').matches) return;
  let renderer;
  try {
    renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, powerPreference: 'high-performance' });
  } catch (e) {
    return;
  }
  if (!renderer.getContext()) return;
  const canvas = renderer.domElement;
  canvas.className = 'pip-3d';
  canvas.setAttribute('aria-hidden', 'true');
  document.body.appendChild(canvas);
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.75));
  renderer.setSize(window.innerWidth, window.innerHeight);

  const scene = new THREE.Scene();
  const camera = new THREE.PerspectiveCamera(18, window.innerWidth / window.innerHeight, 1, 600);
  camera.position.set(0, 14, 80);
  camera.lookAt(0, 0, 0);
  scene.add(new THREE.HemisphereLight(0xc4b6ff, 0x2a1650, 1.8));
  const key = new THREE.DirectionalLight(0xfff1e0, 2.3);
  key.position.set(-3, 5, 6);
  scene.add(key);
  const rim = new THREE.DirectionalLight(0xa880ff, 1.8);
  rim.position.set(4, 3, -5);
  scene.add(rim);

  const glowTex = glowTexture();
  const discGeo = new THREE.PlaneGeometry(2.4, 2.4);
  discGeo.rotateX(-Math.PI / 2);

  /* ---------- screen <-> world, on the z = 0 plane */
  const ray = new THREE.Raycaster();
  const plane = new THREE.Plane(new THREE.Vector3(0, 0, 1), 0);
  const ndc = new THREE.Vector2();
  function toWorld(sx, sy, out) {
    ndc.set((sx / window.innerWidth) * 2 - 1, -(sy / window.innerHeight) * 2 + 1);
    ray.setFromCamera(ndc, camera);
    ray.ray.intersectPlane(plane, out);
    return out;
  }
  function worldPerPx(p) {
    const d = camera.position.distanceTo(p);
    return (2 * d * Math.tan(THREE.MathUtils.degToRad(camera.fov / 2))) / window.innerHeight;
  }
  const wrap = (a) => a - Math.PI * 2 * Math.round(a / (Math.PI * 2));

  /* ---------- one skin per fox */
  const skins = new Set();
  const tmp = new THREE.Vector3(), tmp2 = new THREE.Vector3();
  const clock = new THREE.Clock();

  class Skin {
    constructor(fox) {
      this.fox = fox;
      const pip = buildPip();
      this.pip = pip;
      this.root = pip.root;
      this.nodes = pip.nodes;
      scene.add(this.root);
      applyPose(this.nodes, SIT);
      this.root.updateMatrixWorld(true);
      const box = new THREE.Box3().setFromObject(this.root);
      const paws = new THREE.Box3();
      ['fpawL', 'fpawR', 'hpawL', 'hpawR'].forEach((n) => paws.expandByObject(this.nodes[n]));
      this.footY = paws.min.y;
      this.modelH = box.max.y - this.footY;
      this.mixer = new THREE.AnimationMixer(this.root);
      this.clips = buildClips();
      this.actions = {};
      for (const [name, clip] of Object.entries(this.clips)) {
        const a = this.mixer.clipAction(clip);
        if (ONE_SHOT.has(name)) { a.setLoop(THREE.LoopOnce, 1); a.clampWhenFinished = true; }
        this.actions[name] = a;
      }
      this.current = null;
      this.oneShotUntil = 0;
      this.yaw = FACE_VIEWER;
      this.wasAir = false; this.wasJump = false;
      this.chartAcc = 0;
      this.nodes.gaze.rotation.order = 'YXZ';
      this.look = { x: 0, y: 0 };
      this.shadow = new THREE.Mesh(discGeo, new THREE.MeshBasicMaterial({ color: 0x05031a, map: glowTex, transparent: true, opacity: 0.5, depthWrite: false }));
      this.shadow.renderOrder = -2;
      scene.add(this.shadow);
      this.play('sit', 0);
      // The SVG stays for clicks and focus; the pixels come from here.
      fox.svg.style.opacity = '0';
      fox.svg.style.pointerEvents = 'auto';
    }
    dispose() {
      scene.remove(this.root, this.shadow);
      this.fox.svg.style.opacity = '';
      this.fox.svg.style.pointerEvents = '';
    }
    play(name, fade = 0.25, timeScale = 1) {
      const next = this.actions[name];
      if (!next) return;
      if (this.current === next && !ONE_SHOT.has(name)) { next.setEffectiveTimeScale(timeScale); return; }
      next.reset().setEffectiveTimeScale(timeScale).setEffectiveWeight(1).fadeIn(fade).play();
      if (this.current && this.current !== next) this.current.fadeOut(fade);
      this.current = next;
      if (ONE_SHOT.has(name)) this.oneShotUntil = performance.now() + (this.clips[name].duration / timeScale) * 1000;
    }
    /** A one-shot flourish: happy, excited, shake, lick, stretch, startle, flick… */
    emote(name) {
      if (!this.actions[name]) return false;
      this.play(name, 0.12, 1);
      return true;
    }
    frame(dt, time) {
      const f = this.fox;
      // ---- which clip
      const run = f.runW || 0, sleep = f.sleepW || 0, stampede = f.mood === 'stampede';
      const leaping = !!f.leapT, jumping = !!f.jump;
      if (leaping && !this.wasAir) this.play('hop', 0.08, 0.9 / Math.max(0.35, (f.leapT.dur || 0.7) + 0.14));
      else if (jumping && !this.wasJump) this.play('hop', 0.08, 0.9 / (f.jump.dur || 0.5));
      else if (!leaping && !jumping && performance.now() > this.oneShotUntil) {
        if (sleep > 0.5) this.play('sleep', 0.6);
        else if (run > 0.08) {
          const vUnits = (f.v || 0) / Math.max(20, f.size || 120);
          if (stampede || vUnits > 3.2) this.play('run', 0.2, 0.9 + Math.min(1.1, vUnits * 0.22));
          else this.play('trot', 0.2, 0.8 + Math.min(1.4, vUnits * 0.45));
        } else this.play('sit', 0.35);
      }
      this.wasAir = leaping; this.wasJump = jumping;

      // ---- where, how big, which way
      toWorld(f.x, f.y, tmp);
      const scale = ((f.size || 120) * worldPerPx(tmp)) / this.modelH;
      const lift = Math.max(0, -(f.jump ? f.jump.h * Math.sin(Math.PI * (f.jump.t / f.jump.dur)) : 0));
      // Leaps already move f.y along the arc; hops don't, so lift the model.
      this.root.scale.setScalar(scale);
      this.root.position.set(tmp.x, tmp.y - this.footY * scale + lift * worldPerPx(tmp), tmp.z);
      let want = run > 0.08 || leaping ? (f.facing >= 0 ? 1.15 : -1.15) : FACE_VIEWER;
      if (sleep > 0.5) want = f.facing >= 0 ? 0.55 : -0.55;
      this.yaw += wrap(want - this.yaw) * Math.min(1, dt * 6);
      this.root.rotation.y = this.yaw;

      // ---- shadow on the ground
      const ground = f.leapT ? { x: f.x, y: f.leapT.y0 + (f.leapT.x1 === f.leapT.x0 ? 0 : (f.x - f.leapT.x0) / (f.leapT.x1 - f.leapT.x0)) * (f.leapT.y1 - f.leapT.y0) } : { x: f.x, y: f.y };
      toWorld(ground.x, ground.y, tmp2);
      const up = Math.max(0, (this.root.position.y - (tmp2.y - this.footY * scale)) / (this.modelH * scale));
      this.shadow.position.set(tmp2.x, tmp2.y + 0.01 * scale, tmp2.z);
      this.shadow.scale.setScalar(scale * (0.85 - Math.min(0.5, up * 0.4)) * (sleep > 0.5 ? 1.15 : 1));
      this.shadow.material.opacity = 0.45 * (1 - Math.min(0.8, up));

      this.mixer.update(dt);

      // ---- gaze: a touch toward the viewer while sitting, ahead while moving
      const tx = run > 0.08 ? 0 : THREE.MathUtils.clamp(-wrap(this.yaw - FACE_VIEWER) * 0.6, -1, 1);
      const ty = sleep > 0.5 ? 0 : -0.05;
      this.look.x += (tx - this.look.x) * Math.min(1, dt * 6);
      this.look.y += (ty - this.look.y) * Math.min(1, dt * 6);
      this.nodes.gaze.rotation.set(this.look.y, this.look.x, 0);

      // ---- the tail screen shows the live tokens/sec
      this.chartAcc += dt;
      if (this.chartAcc > 1 / 30) {
        const vals = f.tailValues && f.tailValues.length ? f.tailValues.slice(-24).map((v) => v / Math.max(1, f.tailMax || 1)) : null;
        this.pip.chart.setValues(vals);
        this.pip.chart.draw(time, this.chartAcc, stampede ? 3 : sleep > 0.5 ? 0.2 : 1 + run);
        this.chartAcc = 0;
      }
    }
  }

  function frame() {
    const dt = Math.min(DT_MAX, clock.getDelta());
    const time = clock.elapsedTime;
    if (document.hidden) { requestAnimationFrame(frame); return; }
    for (const s of skins) s.frame(dt, time);
    renderer.render(scene, camera);
    requestAnimationFrame(frame);
  }
  function onResize() {
    camera.aspect = window.innerWidth / window.innerHeight;
    camera.updateProjectionMatrix();
    renderer.setSize(window.innerWidth, window.innerHeight);
  }
  window.addEventListener('resize', onResize);

  /** A still of Pip sitting, for the small icons that used the SVG. */
  function snapshot(px = 96) {
    try {
      const r = new THREE.WebGLRenderer({ antialias: true, alpha: true });
      r.setPixelRatio(2);
      r.setSize(px, px);
      const sc = new THREE.Scene();
      sc.add(new THREE.HemisphereLight(0xc4b6ff, 0x2a1650, 1.8));
      const k = new THREE.DirectionalLight(0xfff1e0, 2.3); k.position.set(-3, 5, 6); sc.add(k);
      const rm = new THREE.DirectionalLight(0xa880ff, 1.8); rm.position.set(4, 3, -5); sc.add(rm);
      const pip = buildPip();
      applyPose(pip.nodes, SIT);
      pip.root.rotation.y = FACE_VIEWER;
      pip.root.updateMatrixWorld(true);
      const box = new THREE.Box3().setFromObject(pip.root);
      const c = box.getCenter(new THREE.Vector3()), size = box.getSize(new THREE.Vector3());
      const cam = new THREE.PerspectiveCamera(28, 1, 0.1, 100);
      const dist = (Math.max(size.x, size.y) * 1.15) / (2 * Math.tan(THREE.MathUtils.degToRad(14)));
      cam.position.set(c.x, c.y + size.y * 0.05, c.z + dist);
      cam.lookAt(c.x, c.y, c.z);
      sc.add(pip.root);
      r.render(sc, cam);
      const url = r.domElement.toDataURL('image/png');
      r.dispose();
      return url;
    } catch (e) {
      return null;
    }
  }

  Pip.setSkin({
    attach(fox) { const s = new Skin(fox); skins.add(s); fox.skin3d = s; return s; },
    detach(fox) { if (fox.skin3d) { fox.skin3d.dispose(); skins.delete(fox.skin3d); fox.skin3d = null; } },
    emote(fox, name) { return fox.skin3d ? fox.skin3d.emote(name) : false; },
    snapshot,
  });
  requestAnimationFrame(frame);
  // Small icons: the same fox, as a picture.
  const url = snapshot();
  if (url) {
    for (const img of document.querySelectorAll('img[src="/pip.svg"]')) { img.src = url; img.classList.add('pip-3d-icon'); }
    document.documentElement.style.setProperty('--pip-icon', `url("${url}")`);
    window.__pipIcon = url;
  }
}

if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', start);
else start();
