import { useLayoutEffect, useRef, useState, type RefObject } from 'react'
import { motion, useReducedMotion } from 'motion/react'

export function SidebarHighlight({ container, active }: { container: RefObject<HTMLElement | null>; active: HTMLElement | null }) {
  const reduced = useReducedMotion()
  const previousActive = useRef<HTMLElement | null>(null)
  const [entering, setEntering] = useState(true)
  const [visible, setVisible] = useState(false)
  const [bounds, setBounds] = useState<{ top: number; left: number; width: number; height: number } | null>(null)
  useLayoutEffect(() => {
    setEntering(previousActive.current === null)
    previousActive.current = active
    if (!active || !container.current) { setVisible(false); return }
    setVisible(true)
    let frame: number
    const measure = () => {
      if (!active.isConnected || !container.current) { previousActive.current = null; setVisible(false); return }
      const parent = container.current.getBoundingClientRect(), rect = active.getBoundingClientRect()
      const next = { top: rect.top - parent.top, left: rect.left - parent.left, width: rect.width, height: rect.height }
      setBounds(previous => previous && Object.keys(next).every(key => previous[key as keyof typeof next] === next[key as keyof typeof next]) ? previous : next)
      frame = requestAnimationFrame(measure)
    }
    measure()
    return () => cancelAnimationFrame(frame)
  }, [active, container])
  if (!bounds) return null
  // Preserve the exit rectangle; fresh entries fade at the target without travelling.
  return <motion.div aria-hidden="true" className="fox-sidebar-moving-highlight"
    style={{ position: 'absolute', zIndex: 0, pointerEvents: 'none' }}
    initial={{ ...bounds, opacity: 0 }} animate={{ ...bounds, opacity: visible ? 1 : 0 }}
    transition={{ ...(reduced || entering ? { duration: 0 } : { type: 'spring', stiffness: 350, damping: 35 }), opacity: { duration: reduced ? 0 : 0.15 } }} />
}
