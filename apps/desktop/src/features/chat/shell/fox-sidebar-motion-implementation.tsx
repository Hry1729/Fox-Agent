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
    let frame = 0
    const measure = () => {
      if (!active.isConnected || !container.current) { previousActive.current = null; setVisible(false); return }
      const parent = container.current.getBoundingClientRect(), rect = active.getBoundingClientRect()
      const next = { top: rect.top - parent.top, left: rect.left - parent.left, width: rect.width, height: rect.height }
      setBounds(previous => previous && Object.keys(next).every(key => previous[key as keyof typeof next] === next[key as keyof typeof next]) ? previous : next)
    }
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure) }
    const observer = new ResizeObserver(schedule)
    observer.observe(active)
    observer.observe(container.current)
    const parent = container.current
    parent.addEventListener('scroll', schedule, true)
    const mutations = new MutationObserver(schedule)
    mutations.observe(parent, { childList: true, subtree: true })
    measure()
    return () => { cancelAnimationFrame(frame); observer.disconnect(); mutations.disconnect(); parent.removeEventListener('scroll', schedule, true) }
  }, [active, container])
  if (!bounds) return null
  const position = { x: bounds.left, y: bounds.top, width: bounds.width, height: bounds.height }
  // Preserve the exit rectangle; fresh entries fade at the target without travelling.
  return <motion.div aria-hidden="true" className="fox-sidebar-moving-highlight"
    style={{ position: 'absolute', top: 0, left: 0, zIndex: 0, pointerEvents: 'none' }}
    initial={{ ...position, opacity: 0 }} animate={{ ...position, opacity: visible ? 1 : 0 }}
    transition={{ ...(reduced || entering ? { duration: 0 } : { type: 'spring', stiffness: 350, damping: 35 }), opacity: { duration: reduced ? 0 : 0.15 } }} />
}
