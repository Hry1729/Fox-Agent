import { useLayoutEffect, useState, type RefObject } from 'react'
import { useReducedMotion } from 'motion/react'
import { Highlight, HighlightItem } from '@/components/animate-ui/primitives/effects/highlight'

/** A measured proxy uses the registry's spring without replacing any navigation DOM. */
export function SidebarHighlight({ container, active }: { container: RefObject<HTMLElement | null>; active: HTMLElement | null }) {
  const reduced = useReducedMotion()
  const [bounds, setBounds] = useState<{ top: number; left: number; width: number; height: number } | null>(null)
  useLayoutEffect(() => {
    if (!active || !container.current) { setBounds(null); return }
    let frame: number
    const measure = () => {
      if (!active.isConnected || !container.current) { setBounds(null); return }
      const parent = container.current.getBoundingClientRect(), rect = active.getBoundingClientRect()
      const next = { top: rect.top - parent.top, left: rect.left - parent.left, width: rect.width, height: rect.height }
      setBounds(previous => previous && Object.keys(next).every(key => previous[key as keyof typeof next] === next[key as keyof typeof next]) ? previous : next)
      frame = requestAnimationFrame(measure)
    }
    measure()
    return () => cancelAnimationFrame(frame)
  }, [active, container])
  return <div aria-hidden="true" style={{ position: 'absolute', inset: 0, zIndex: 0, pointerEvents: 'none' }}>
    <Highlight mode="parent" controlledItems value={bounds ? 'active-row' : null} click={false} forceUpdateBounds className="fox-sidebar-moving-highlight" transition={reduced ? { duration: 0 } : { type: 'spring', stiffness: 350, damping: 35 }} exitDelay={0}>
      {/* Keep the proxy mounted: a null value lets HighlightItem clear the parent's bounds. */}
      <HighlightItem asChild value="active-row"><span style={{ position: 'absolute', ...(bounds ?? { top: 0, left: 0, width: 0, height: 0 }) }} /></HighlightItem>
    </Highlight>
  </div>
}
