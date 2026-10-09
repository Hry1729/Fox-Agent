"use client";

// Glyph Pulse from Dot Matrix. See LICENSE and NOTICE.md in this folder.
import { useMemo } from "react";
import { DotMatrix3Base, rowMajorIndex3 } from "./dotmatrix-core";
import { useCyclePhase, useDotMatrixPhases, usePrefersReducedMotion } from "./dotmatrix-hooks";
import type { DotAnimationResolver, DotMatrixCommonProps } from "./dotmatrix-core";

export type Dotm3x3_11Props = DotMatrixCommonProps & {
  synchronized?: boolean;
  /**
   * Opacity a fully extinguished dot settles at. The glyph's own dim base
   * (`BASE_OPACITY`) is right for a loader that should keep a faint matrix
   * footprint; a caller that must show *no* residual background passes 0. The
   * default keeps every other page's rendering unchanged.
   */
  floorOpacity?: number;
};

const BASE_OPACITY = 0.06;
const PEAK_OPACITY = 0.88;
const CYCLE_MS_BASE = 2700;
/** Share of one beat each shape is held before it starts becoming the next. */
export const GLYPH_HOLD_RATIO = 0.52;
/** Share of one beat actually spent morphing into the next shape. */
export const GLYPH_MORPH_RATIO = 0.34;
const SMOOTH_TRANSITION = "opacity 120ms linear";

/**
 * One morph beat: how long the glyph holds one shape before it starts becoming
 * the next. Callers that must stay in step with the shape change (the sidebar's
 * green/yellow status ring) derive their own period from this same number
 * instead of guessing at a duration of their own.
 */
export function glyphMorphBeatMs(speed = 1): number {
  const safeSpeed = speed > 0 ? speed : 1;
  return CYCLE_MS_BASE / safeSpeed / GLYPH_PATTERNS.length;
}

/**
 * How long the glyph actually spends changing shape inside one beat.
 *
 * A caller that alternates between its own states (the sidebar's status ring) must
 * spend the same time changing, otherwise its change reads as faster than the
 * glyph even when both keep the same time per form.
 */
export function glyphMorphWindowMs(speed = 1): number {
  return glyphMorphBeatMs(speed) * GLYPH_MORPH_RATIO;
}

function smoothstep(value: number): number {
  const t = Math.min(1, Math.max(0, value));
  return t * t * (3 - 2 * t);
}

function patternWeight(pattern: ReadonlySet<number>, index: number): number {
  return pattern.has(index) ? 1 : 0;
}

/** Original motifs: corners, cross, full, center, ring, X, rails. */
const GLYPH_PATTERNS: readonly ReadonlySet<number>[] = [
  new Set([rowMajorIndex3(0, 0), rowMajorIndex3(0, 2), rowMajorIndex3(2, 0), rowMajorIndex3(2, 2)]),
  new Set([
    rowMajorIndex3(0, 1), rowMajorIndex3(1, 0), rowMajorIndex3(1, 1),
    rowMajorIndex3(1, 2), rowMajorIndex3(2, 1)
  ]),
  new Set(Array.from({ length: 9 }, (_, index) => index)),
  new Set([rowMajorIndex3(1, 1)]),
  new Set([
    rowMajorIndex3(0, 0), rowMajorIndex3(0, 1), rowMajorIndex3(0, 2),
    rowMajorIndex3(1, 0), rowMajorIndex3(1, 2),
    rowMajorIndex3(2, 0), rowMajorIndex3(2, 1), rowMajorIndex3(2, 2)
  ]),
  new Set([
    rowMajorIndex3(0, 0), rowMajorIndex3(0, 2), rowMajorIndex3(1, 1),
    rowMajorIndex3(2, 0), rowMajorIndex3(2, 2)
  ]),
  new Set([
    rowMajorIndex3(0, 1), rowMajorIndex3(1, 0),
    rowMajorIndex3(1, 2), rowMajorIndex3(2, 1)
  ])
];

/** How many shapes one glyph cycle paints. A caller that alternates between its
 *  own count of states derives a comparable period from {@link glyphMorphBeatMs}
 *  times that count, so both keep the same time per form. */
export const GLYPH_FORM_COUNT = GLYPH_PATTERNS.length;

/**
 * Progress of one dot through its shape change, on the glyph's own rule.
 *
 * A dot with a larger stagger *starts later* and every dot reaches the new shape
 * at the same deadline (`HOLD + MORPH` of the beat): the offset shifts the start,
 * never the end. Callers that change a whole group of dots must share this rule —
 * a CSS animation plus a per-dot delay shifts both ends, so the group would spread
 * over start-offset + window instead of one window.
 */
export function glyphMorphProgress(segmentPhase: number, stagger: number): number {
  const morphStart = GLYPH_HOLD_RATIO;
  const morphEnd = GLYPH_HOLD_RATIO + GLYPH_MORPH_RATIO;
  if (segmentPhase < morphStart + stagger * GLYPH_MORPH_RATIO) return 0;
  if (segmentPhase >= morphEnd) return 1;
  const localSpan = morphEnd - morphStart;
  const localPhase = (segmentPhase - morphStart - stagger * GLYPH_MORPH_RATIO)
    / (localSpan * (1 - stagger * 0.85));
  return smoothstep(localPhase);
}

export function Dotm3x3_11({
  speed = 1.25,
  pattern = "full",
  dotShape = "circle",
  animated = true,
  hoverAnimated = false,
  synchronized = false,
  floorOpacity,
  ...rest
}: Dotm3x3_11Props) {
  const reducedMotion = usePrefersReducedMotion();
  const { phase: matrixPhase, onMouseEnter, onMouseLeave } = useDotMatrixPhases({
    animated: Boolean(animated && !reducedMotion),
    hoverAnimated: Boolean(hoverAnimated && !reducedMotion),
    speed
  });
  const cyclePhase = useCyclePhase({
    active: !reducedMotion && matrixPhase !== "idle",
    cycleMsBase: CYCLE_MS_BASE,
    speed,
    synchronized
  });

  const animationResolver = useMemo<DotAnimationResolver>(() => {
    const patternCount = GLYPH_PATTERNS.length;
    const scaledPhase = cyclePhase * patternCount;
    const patternIndex = Math.floor(scaledPhase) % patternCount;
    const nextPatternIndex = (patternIndex + 1) % patternCount;
    const segmentPhase = scaledPhase - Math.floor(scaledPhase);
    const currentPattern = GLYPH_PATTERNS[patternIndex]!;
    const nextPattern = GLYPH_PATTERNS[nextPatternIndex]!;
    // A dot that the shape has left behind settles at the floor: 0 means the
    // nine-dot block leaves no pale background once its shape has moved on.
    const floor = Math.min(1, Math.max(0, floorOpacity ?? BASE_OPACITY));

    return ({ isActive, index, row, col, reducedMotion: rm, phase }) => {
      if (!isActive) return { className: "dmx-inactive" };
      const stagger = (row + col) / 4;
      const morphT = glyphMorphProgress(segmentPhase, stagger);
      let weight = patternWeight(currentPattern, index) * (1 - morphT)
        + patternWeight(nextPattern, index) * morphT;
      if (segmentPhase < GLYPH_HOLD_RATIO && weight > 0.01) {
        const breathe = 0.78 + 0.22 * Math.sin((segmentPhase / GLYPH_HOLD_RATIO) * Math.PI);
        weight *= breathe;
      }
      const opacity = floor + weight * (PEAK_OPACITY - floor);
      if (rm || phase === "idle") return { style: { opacity } };
      return { style: { opacity, transition: SMOOTH_TRANSITION } };
    };
  }, [cyclePhase, floorOpacity]);

  return <DotMatrix3Base
    {...rest}
    speed={speed}
    pattern={pattern}
    dotShape={dotShape}
    animated={animated}
    phase={matrixPhase}
    onMouseEnter={onMouseEnter}
    onMouseLeave={onMouseLeave}
    reducedMotion={reducedMotion}
    animationResolver={animationResolver}
  />;
}
