import { useEffect, useState, type CSSProperties } from 'react'
import { Dotm3x3_12 } from '@/components/dotmatrix/dotm-3x3-12'
import { Dotm3x3_15 } from '@/components/dotmatrix/dotm-3x3-15'
import { usePrefersReducedMotion } from '@/components/dotmatrix/dotmatrix-hooks'
import './conversation-status-icon.css'

export type ConversationIconState = 'idle' | 'running' | 'waiting' | 'complete'

/** The approved nine-dot design, shared by every sidebar conversation row. */
export function ConversationStatusIcon({ state, size = 14 }: { state: ConversationIconState; size?: number }) {
  const reduced = usePrefersReducedMotion()
  const [visible, setVisible] = useState(() => typeof document === 'undefined' || !document.hidden)
  useEffect(() => {
    const update = () => setVisible(!document.hidden)
    document.addEventListener('visibilitychange', update)
    return () => document.removeEventListener('visibilitychange', update)
  }, [])
  if (state === 'idle') return null
  const animated = !reduced && visible
  const color = state === 'running' ? 'var(--fox-status-blue)' : state === 'waiting' ? 'var(--fox-status-yellow)' : 'var(--fox-status-green)'
  const style = { width: size, height: size } as CSSProperties
  return <span className={`fox-status-matrix is-${state}${animated ? ' is-animated' : ''}`} style={style} aria-hidden="true" data-icon-state={state}>
    {state !== 'complete' ? (() => {
      const Component = state === 'running' ? Dotm3x3_12 : Dotm3x3_15
      return <Component boxSize={size} dotSize={6} cellPadding={1} color={color} speed={state === 'running' ? 1.9 : .45} animated={animated} hoverAnimated={false} bloom={false} opacityBase={.18} opacityMid={.46} opacityPeak={1} />
    })() : <span className="fox-status-check" style={{ width: 20, height: 20, transform: `scale(${size / 20})`, color }}>
      {[[17,3],[3,9],[13,9],[8,15]].map(([x,y],index) => <span className="fox-status-check-dot" key={index} style={{ left: x - 3, top: y - 3 }} />)}
    </span>}
  </span>
}
