import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react'
import { Dotm3x3_11 } from '@/components/dotmatrix/dotm-3x3-11'
import { usePrefersReducedMotion } from '@/components/dotmatrix/dotmatrix-hooks'
import './conversation-status-icon.css'

export type ConversationIconState = 'idle' | 'running' | 'waiting' | 'complete'

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
  const style = { width: size, height: size } as CSSProperties
  return <span ref={root} className={`fox-status-matrix is-${state}${animated ? ' is-animated' : ''}`} style={style} aria-hidden="true" data-icon-state={state}>
    {state === 'running' ? <Dotm3x3_11 boxSize={size} dotSize={6} cellPadding={1} color={color} speed={.65} synchronized animated={animated} hoverAnimated={false} bloom={false} opacityBase={.18} opacityMid={.46} opacityPeak={1} />
      : <span className="fox-status-ring-grid" style={{ gap: size * .05, color }}>
        {Array.from({ length: 9 }, (_, index) => <span className={`fox-status-ring-dot${index === 4 ? ' is-center' : ''}`} key={index} />)}
      </span>}
  </span>
}
