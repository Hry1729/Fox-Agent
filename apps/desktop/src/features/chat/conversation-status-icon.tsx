import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react'
import {
  Dotm3x3_11, GLYPH_FORM_COUNT, GLYPH_HOLD_RATIO, GLYPH_MORPH_RATIO, glyphMorphBeatMs, glyphMorphWindowMs,
} from '@/components/dotmatrix/dotm-3x3-11'
import { usePrefersReducedMotion } from '@/components/dotmatrix/dotmatrix-hooks'
import './conversation-status-icon.css'

export type ConversationIconState = 'idle' | 'running' | 'waiting' | 'complete'

/** The shape-change speed the blue glyph uses; green/yellow derive their beat from it. */
const GLYPH_SPEED = .65

/** The two states the green/yellow status ring alternates between. */
export const STATUS_RING_FORMS = 2

/**
 * The green/yellow breathing period in one shared tempo with the blue shape.
 *
 * One beat `T` is one shape change of the blue glyph, so the glyph's full cycle is
 * `GLYPH_FORM_COUNT * T` and the ring's is `STATUS_RING_FORMS * T`: the two run at
 * **7 : 2**, which is the only ratio that gives every form the same time. Both
 * colours take their time origin from the document timeline (the CSS animations
 * are pinned to it below), so a row mounted later — a new conversation, a state
 * change, a sidebar reopened after being hidden — lands on the same phase instead
 * of starting its own timer. The same period drives the approval (`waiting`) ring.
 */
export const STATUS_BREATHE_MS = STATUS_RING_FORMS * glyphMorphBeatMs(GLYPH_SPEED)

/** The running glyph's own cycle, exported so the 7:2 relationship is testable. */
export const GLYPH_CYCLE_MS = GLYPH_FORM_COUNT * glyphMorphBeatMs(GLYPH_SPEED)

/**
 * The ring's keyframe positions, derived from the glyph's own hold/morph split.
 *
 * Equal cycle ratios are not enough, and neither is a per-dot delay: the glyph
 * makes each dot *start later* while every dot still reaches the new shape at the
 * same deadline, so the whole group changes inside one morph window. A delay would
 * move both ends and spread the group over offset + window, so each of the five
 * stagger steps gets its own keyframe set built from these numbers, and the test
 * asserts the CSS against them.
 */
const RING_FORM_SHARE = 100 / STATUS_RING_FORMS
export const STATUS_RING_HOLD_PERCENT = GLYPH_HOLD_RATIO * RING_FORM_SHARE
export const STATUS_RING_MORPH_PERCENT = GLYPH_MORPH_RATIO * RING_FORM_SHARE
/** The ring's change window — the same duration the glyph spends morphing. */
export const STATUS_RING_MORPH_MS = glyphMorphWindowMs(GLYPH_SPEED)
/** Stagger steps the ring uses, matching the glyph's `(row + column) / 4`. */
export const STATUS_RING_STAGGER_STEPS = 5
/** Fraction of the cycle where a dot's change starts, for one stagger step. */
export function statusRingMorphStartPercent(stagger: number) {
  return STATUS_RING_HOLD_PERCENT + stagger * STATUS_RING_MORPH_PERCENT
}
/** Fraction where that change must be finished — the same for every dot. */
export function statusRingMorphDeadlinePercent() {
  return STATUS_RING_HOLD_PERCENT + STATUS_RING_MORPH_PERCENT
}

/** The approved nine-dot design, shared by every sidebar conversation row. */
export function ConversationStatusIcon({ state, size = 14 }: { state: ConversationIconState; size?: number }) {
  const reduced = usePrefersReducedMotion()
  const root = useRef<HTMLSpanElement>(null)
  const [visible, setVisible] = useState(() => typeof document === 'undefined' || !document.hidden)
  useEffect(() => {
    const update = () => setVisible(!document.hidden)
    document.addEventListener('visibilitychange', update)
    return () => document.removeEventListener('visibilitychange', update)
  }, [])
  const animated = !reduced && visible
  useLayoutEffect(() => {
    if (!animated || !root.current?.getAnimations) return
    // Every icon uses the document timeline origin, including late mounts and
    // icons resumed after a hidden page. There is no per-icon ticking timer.
    for (const animation of root.current.getAnimations({ subtree: true })) {
      if ('animationName' in animation) animation.startTime = 0
    }
  }, [animated, state, size])
  if (state === 'idle') return null
  const color = state === 'running' ? 'var(--fox-status-blue)' : state === 'waiting' ? 'var(--fox-status-yellow)' : 'var(--fox-status-green)'
  // The breathe is phase-locked to the shape change: one direction change per
  // beat, so the dim trough of every breathe coincides with a shape change.
  const style = {
    width: size,
    height: size,
    '--fox-status-breathe': `${STATUS_BREATHE_MS}ms`,
  } as CSSProperties & Record<'--fox-status-breathe', string>
  return <span ref={root} className={`fox-status-matrix is-${state}${animated ? ' is-animated' : ''}`} style={style} aria-hidden="true" data-icon-state={state}>
    {state === 'running' ? <Dotm3x3_11 boxSize={size} dotSize={6} cellPadding={1} color={color} speed={GLYPH_SPEED} synchronized animated={animated} hoverAnimated={false} bloom={false} opacityBase={.18} opacityMid={.46} opacityPeak={1} floorOpacity={0} />
      : <span className="fox-status-ring-grid" style={{ gap: size * .05, color }}>
        {Array.from({ length: 9 }, (_, index) => {
          // The glyph staggers its dots by row+column: a later dot starts its
          // change later and every dot finishes at the same deadline. Each stagger
          // step therefore has its own keyframe set (a delay would shift the end
          // too), selected by class so the browser keeps the animation off the main
          // thread.
          const staggerStep = (Math.floor(index / 3) + (index % 3))
          return <span
            className={`fox-status-ring-dot is-stagger-${staggerStep}${index === 4 ? ' is-center' : ''}`}
            key={index}
          />
        })}
      </span>}
  </span>
}
