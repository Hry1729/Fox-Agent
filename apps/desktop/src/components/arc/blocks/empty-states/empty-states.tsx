// Source: https://uiarc.dev/r/empty-states.json
// Fox adaptation: controlled scene, real copy/actions, compact illustration. Registry geometry and spring engine retained.
"use client";
import { useEffect, useId, useRef, type ReactNode, type PointerEvent } from "react";
import { animate, useReducedMotion } from "motion/react";
import { motionTokens } from "../../lib/motion-tokens";
import { FRAMES, KINDS, VIEW, shapePath, type Frame, type SceneId } from "./empty-states-art";
import styles from "./empty-states.module.css";

export interface EmptyStatesProps {
  title: string;
  description?: ReactNode;
  icon?: ReactNode;
  action?: ReactNode;
  variant?: "empty" | "search" | "error";
  className?: string;
}

/** Shapes lead, details trail: back tiles, body, front tiles, then lines and the mark. Totals stay under 0.4s. */
const DELAY = [.05, .03, 0, .04, .07, .1, .06, .09, .11, .14];
const NUDGE = [.03, .015, 0, .02, .035, .05, .02, .04, .05, .07];
const DRIFT = 2.5;


const enter = [...motionTokens.ease.enter] as [number, number, number, number];
const standard = [...motionTokens.ease.standard] as [number, number, number, number];

/** Motion's visualDuration and bounce, turned into stiffness and damping for the vector springs below. */
function springOf({ visualDuration, bounce }: { visualDuration: number; bounce: number }) {
  const root = (2 * Math.PI) / (visualDuration * 1.2);
  return { k: root * root, c: 2 * Math.min(1, Math.max(.05, 1 - bounce)) * root };
}
const MORPH = springOf(motionTokens.spring.morph), SMOOTH = springOf(motionTokens.spring.smooth), SWAY = { k: 90, c: 19 };

type Channel = { pos: Float64Array; vel: Float64Array; target: Float64Array; queue: { at: number; values: number[] }[]; k: number; c: number; still: boolean };
const channel = (values: number[], spring: { k: number; c: number }): Channel =>
  ({ pos: Float64Array.from(values), vel: new Float64Array(values.length), target: Float64Array.from(values), queue: [], ...spring, still: true });

/** Integrates one spring vector. A new target keeps the current velocity, so shapes retarget mid-flight without a hitch. */
function advance(ch: Channel, now: number, dt: number) {
  while (ch.queue.length && ch.queue[0]!.at <= now) {
    const next = ch.queue.shift();
    if (next) { ch.target.set(next.values); ch.still = false; }
  }
  if (!ch.still) {
    const steps = Math.max(1, Math.ceil(dt * 240)), h = dt / steps, { pos, vel, target, k, c } = ch;
    for (let s = 0; s < steps; s++) for (let i = 0; i < pos.length; i++) { vel[i]! += (-k * (pos[i]! - target[i]!) - c * vel[i]!) * h; pos[i]! += vel[i]! * h; }
    let settled = true;
    for (let i = 0; i < pos.length; i++) if (Math.abs(pos[i]! - target[i]!) > .004 || Math.abs(vel[i]!) > .02) { settled = false; break; }
    if (settled) { pos.set(target); vel.fill(0); ch.still = true; }
  }
  return !ch.still || ch.queue.length > 0;
}

type Nodes = { paths: SVGPathElement[]; ghosts: SVGPathElement[]; main: SVGGElement; ghost: SVGGElement };

/** One animation frame loop drives every shape through refs. It sleeps once everything has settled. */
function createEngine(nodes: Nodes, start: Frame) {
  const geo = start.prims.map(p => channel(p.geo, MORPH));
  const fx = start.prims.map(p => channel(p.fx, SMOOTH));
  const sway = channel([0, 0], SWAY);
  const written = start.prims.map((p, i) => ({ ...shapePath(KINDS[i]!, p.geo, p.fx[1], 0, 0), o: String(p.fx[0]) }));
  let frame = start, raf = 0, last = 0, parallax = false;

  const paint = () => {
    const ox = sway.pos[0]! * DRIFT, oy = sway.pos[1]! * DRIFT;
    nodes.paths.forEach((node, i) => {
      const g = geo[i]!.pos, f = fx[i]!.pos, w = written[i]!;
      const { d, dash } = shapePath(KINDS[i]!, g, f[1]!, ox, oy);
      const o = String(Math.round(Math.min(1, Math.max(0, f[0]!)) * 1000) / 1000);
      if (d !== w.d) { node.setAttribute("d", d); w.d = d; }
      if (o !== w.o) { node.setAttribute("opacity", o); w.o = o; }
      if (dash !== w.dash) { node.setAttribute("stroke-dasharray", dash); w.dash = dash; }
    });
  };
  const tick = (time: number) => {
    raf = 0;
    const now = time / 1000, dt = last ? Math.min(.034, Math.max(0, now - last)) : 1 / 60;
    last = now;
    let busy = advance(sway, now, dt);
    for (let i = 0; i < geo.length; i++) busy = advance(geo[i]!, now, dt) || busy;
    for (let i = 0; i < fx.length; i++) busy = advance(fx[i]!, now, dt) || busy;
    paint();
    if (busy) raf = requestAnimationFrame(tick); else last = 0;
  };
  const kick = () => { if (!raf) raf = requestAnimationFrame(tick); };

  const pointer = (x: number, y: number) => {
    sway.queue = [];
    sway.target.set(parallax ? [x, y] : [0, 0]);
    sway.still = false;
    kick();
  };

  return {
    pointer,
    go(next: Frame, sceneChanged: boolean, reduced: boolean) {
      if (reduced) {
        if (raf) cancelAnimationFrame(raf);
        raf = 0; last = 0;
        sway.queue = []; sway.pos.fill(0); sway.target.fill(0); sway.vel.fill(0); sway.still = true;
        // Crossfade without travel: freeze the old drawing in a ghost layer, snap to the new one, and fade between them.
        nodes.paths.forEach((node, i) => {
          const ghost = nodes.ghosts[i]!;
          ghost.setAttribute("d", node.getAttribute("d") ?? "");
          ghost.setAttribute("opacity", node.getAttribute("opacity") ?? "1");
          ghost.setAttribute("stroke-dasharray", node.getAttribute("stroke-dasharray") ?? "none");
          ghost.dataset.tone = frame.prims[i]!.tone;
        });
        next.prims.forEach((p, i) => {
          for (const [ch, values] of [[geo[i]!, p.geo], [fx[i]!, p.fx]] as const) { ch.queue = []; ch.target.set(values); ch.pos.set(values); ch.vel.fill(0); ch.still = true; }
        });
        frame = next;
        paint();
        animate(nodes.ghost, { opacity: [1, 0] }, { duration: motionTokens.duration.standard, ease: standard });
        animate(nodes.main, { opacity: [0, 1] }, { duration: motionTokens.duration.standard, ease: standard });
        return;
      }
      const now = performance.now() / 1000, delays = sceneChanged ? DELAY : NUDGE;
      next.prims.forEach((p, i) => {
        const delay = delays[i]!, f = fx[i]!;
        geo[i]!.queue = [{ at: now + delay, values: p.geo }];
        if (sceneChanged && p.redraw) f.queue = [{ at: now, values: [p.fx[0], 0] }, { at: now + delay + .18, values: p.fx }];
        else f.queue = [{ at: now + delay + (p.fx[1] > f.target[1]! + .01 ? .12 : 0), values: p.fx }];
      });
      frame = next;
      kick();
    },
    setParallax(on: boolean) { parallax = on; if (!on) pointer(0, 0); },
    destroy() { if (raf) cancelAnimationFrame(raf); raf = 0; },
  };
}

export function EmptyStates({ title, description, icon, action, variant = "empty", className }: EmptyStatesProps) {
  const uid = useId();
  const reduced = !!useReducedMotion();
  const scene: SceneId = variant === "search" ? "search" : variant === "error" ? "offline" : "inbox";
  const frame = FRAMES[scene].idle;
  const initialFrame = useRef(frame);
  const paths = useRef<(SVGPathElement | null)[]>([]);
  const ghosts = useRef<(SVGPathElement | null)[]>([]);
  const mainLayer = useRef<SVGGElement>(null);
  const ghostLayer = useRef<SVGGElement>(null);
  const engine = useRef<ReturnType<typeof createEngine> | null>(null);
  const shown = useRef(scene);

  useEffect(() => {
    const main = mainLayer.current, ghost = ghostLayer.current;
    const nodes = paths.current.filter(Boolean) as SVGPathElement[], ghostNodes = ghosts.current.filter(Boolean) as SVGPathElement[];
    if (!main || !ghost || nodes.length !== KINDS.length || ghostNodes.length !== KINDS.length) return;
    const instance = createEngine({ paths: nodes, ghosts: ghostNodes, main, ghost }, initialFrame.current);
    engine.current = instance;
    return () => { instance.destroy(); engine.current = null; };
  }, []);
  useEffect(() => {
    engine.current?.setParallax(!reduced && window.matchMedia("(hover: hover) and (pointer: fine)").matches);
  }, [reduced]);
  useEffect(() => {
    if (shown.current === scene) return;
    shown.current = scene;
    engine.current?.go(FRAMES[scene].idle, true, reduced);
  }, [scene, reduced]);
  function onPointerMove(event: PointerEvent<HTMLDivElement>) {
    if (event.pointerType !== "mouse" || reduced) return;
    const rect = event.currentTarget.getBoundingClientRect();
    if (!rect.width || !rect.height) return;
    engine.current?.pointer(((event.clientX - rect.left) / rect.width - .5) * 2, ((event.clientY - rect.top) / rect.height - .5) * 2);
  }
  return <section className={[styles.root, className].filter(Boolean).join(" ")} data-variant={variant} aria-labelledby={`${uid}-title`}>
    <div className={styles.panel}>
      <div className={styles.stage} aria-hidden="true" onPointerMove={onPointerMove} onPointerLeave={() => engine.current?.pointer(0, 0)}>
        <svg className={styles.art} viewBox={`0 0 ${VIEW.width} ${VIEW.height}`}>
          <g ref={ghostLayer} opacity={0}>{KINDS.map((kind, i) => <path key={i} ref={node => { ghosts.current[i] = node; }} className={styles.shape} data-kind={kind} d="" />)}</g>
          <g ref={mainLayer}>{initialFrame.current.prims.map((p, i) => {
            const path = shapePath(KINDS[i]!, p.geo, p.fx[1], 0, 0);
            return <path key={i} ref={node => { paths.current[i] = node; }} className={styles.shape} data-kind={KINDS[i]} data-tone={frame.prims[i]!.tone} d={path.d} opacity={p.fx[0]} strokeDasharray={path.dash} />;
          })}</g>
        </svg>
      </div>
      <div className={styles.copy}>
        {icon && <span className={styles.customIcon} aria-hidden="true">{icon}</span>}
        <h3 id={`${uid}-title`} className={styles.title}>{title}</h3>
        {description != null && <div className={styles.line}>{description}</div>}
        {action != null && <div className={styles.action}>{action}</div>}
      </div>
    </div>
  </section>;
}
export default EmptyStates;
