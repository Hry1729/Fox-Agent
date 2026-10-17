import { useEffect, useRef, useState, type PointerEvent } from 'react'
import { AnimatePresence, animate, motion, useMotionValue, useReducedMotion } from 'motion/react'
import type { ExpertLanyardScene } from './expert-lanyard-scene'
import { createExpertCardBackPainter } from './expert-card-emboss'
import './expert-lanyard.css'

function ExpertCardBack({ active }: { active: boolean }) {
  const ref = useRef<HTMLCanvasElement>(null)
  const painter = useRef<ReturnType<typeof createExpertCardBackPainter> | null>(null)
  useEffect(() => {
    const canvas = ref.current
    if (!canvas) return
    painter.current = createExpertCardBackPainter(canvas, canvas)
    return () => { painter.current?.dispose(); painter.current = null }
  }, [])
  useEffect(() => { painter.current?.setActive(active) }, [active])
  return <canvas className="fox-expert-card-emboss" ref={ref} width={1024} height={1372} />
}

type ExpertLanyardProps = { expertId: string; name: string; frontImage: string; description?: string; autoCollapseKey?: number }

export function ExpertLanyard(props: ExpertLanyardProps) {
  const [reduceMotion, setReduceMotion] = useState(() => typeof window !== 'undefined' && window.matchMedia('(prefers-reduced-motion: reduce)').matches)
  useEffect(() => {
    const query = window.matchMedia('(prefers-reduced-motion: reduce)')
    const sync = () => setReduceMotion(query.matches)
    sync()
    query.addEventListener('change', sync)
    return () => query.removeEventListener('change', sync)
  }, [])
  return <AnimatePresence mode="wait" initial={false}>
    <motion.div key={props.expertId} className="fox-expert-lanyard-transition"
      initial={{ y: reduceMotion ? 0 : -220 }} animate={{ y: 0 }}
      exit={{ y: reduceMotion ? 0 : -220, transition: { duration: reduceMotion ? 0 : .24, ease: [.4, 0, 1, 1] } }}
      transition={reduceMotion ? { duration: 0 } : { type: 'spring', stiffness: 290, damping: 20, mass: .8 }}>
      <ExpertLanyardCard {...props} />
    </motion.div>
  </AnimatePresence>
}

function ExpertLanyardCard({ expertId, name, frontImage, description = '随时为你提供协助', autoCollapseKey = 0 }: ExpertLanyardProps) {
  const host = useRef<HTMLSpanElement>(null)
  const scene = useRef<ExpertLanyardScene | null>(null)
  const frontRef = useRef(true)
  const [front, setFront] = useState(true)
  const [ready, setReady] = useState(false)
  const [collapsed, setCollapsed] = useState(false)
  const [dragging, setDragging] = useState(false)
  const collapsedRef = useRef(false)
  const hovering = useRef(false)
  const gesture = useRef<{ id: number; start: number; origin: number; distance: number } | null>(null)
  const suppressClick = useRef(false)
  const reduceMotion = useReducedMotion()
  const y = useMotionValue(0)
  const settle = (hidden: boolean, peek = false) => {
    const target = hidden ? peek ? -110 : -134 : 0
    y.stop()
    if (reduceMotion) y.set(target)
    else animate(y, target, { type: 'spring', stiffness: 380, damping: 25, mass: .8 })
  }
  const setHidden = (hidden: boolean) => {
    collapsedRef.current = hidden
    setCollapsed(hidden)
    // A release always tucks the card fully; a later pointer entry reveals the edge.
    hovering.current = false
    settle(hidden)
  }
  const endDrag = (event: PointerEvent<HTMLButtonElement>, cancelled = false) => {
    const current = gesture.current
    if (!current || current.id !== event.pointerId) return
    gesture.current = null
    setDragging(false)
    if (event.currentTarget.hasPointerCapture?.(event.pointerId)) event.currentTarget.releasePointerCapture(event.pointerId)
    if (!cancelled && current.distance >= 22) setHidden(!collapsedRef.current)
    else settle(collapsedRef.current, hovering.current)
  }
  useEffect(() => {
    collapsedRef.current = false
    hovering.current = false
    gesture.current = null
    setCollapsed(false)
    setDragging(false)
    y.stop(); y.set(0)
    return () => { y.stop() }
  }, [expertId, y])
  useEffect(() => {
    gesture.current = null
    setDragging(false)
    setHidden(autoCollapseKey > 0)
  }, [autoCollapseKey, expertId])
  useEffect(() => {
    let cancelled = false
    frontRef.current = true; setFront(true); setReady(false)
    const unavailable = () => {
      if (cancelled) return
      scene.current?.dispose(); scene.current = null; setReady(false)
    }
    void import('./expert-lanyard-scene').then(({ createExpertLanyardScene }) => {
      if (cancelled || !host.current) return
      try {
        scene.current = createExpertLanyardScene(host.current, { frontImage, name }, unavailable)
        if (!frontRef.current) scene.current.flip(false)
        setReady(true)
      } catch { unavailable() }
    }).catch(unavailable)
    return () => { cancelled = true; scene.current?.dispose(); scene.current = null }
  }, [expertId, frontImage, name])
  const flip = () => {
    const next = !frontRef.current
    frontRef.current = next; setFront(next); scene.current?.flip(next)
  }
  return <motion.button type="button" className="fox-expert-lanyard" style={{ y }} data-collapsed={collapsed} data-dragging={dragging}
    aria-label={`${name}专家挂卡，${collapsed ? '已收起，向下拉或按回车展开' : '已展开，向下拉收起，点击翻面'}`} aria-expanded={!collapsed}
    title={`${name} · ${description} · ${collapsed ? '向下拉展开' : '向下拉收起，点击翻面'}；方向键控制展开和收起`}
    onPointerEnter={() => { hovering.current = true; if (!gesture.current && collapsedRef.current) settle(true, true) }}
    onPointerLeave={() => { hovering.current = false; if (!gesture.current) settle(collapsedRef.current) }}
    onPointerDown={(event) => {
      if (event.button !== 0) return
      suppressClick.current = false
      y.stop()
      gesture.current = { id: event.pointerId, start: event.clientY, origin: y.get(), distance: 0 }
      event.currentTarget.setPointerCapture?.(event.pointerId)
      setDragging(true)
    }}
    onPointerMove={(event) => {
      const current = gesture.current
      if (!current || current.id !== event.pointerId) return
      current.distance = Math.max(0, event.clientY - current.start)
      if (Math.abs(event.clientY - current.start) > 5) suppressClick.current = true
      y.set(current.origin + Math.min(72, current.distance * .65))
    }}
    onPointerUp={(event) => endDrag(event)} onPointerCancel={(event) => endDrag(event, true)} onLostPointerCapture={(event) => endDrag(event, true)}
    onKeyDown={(event) => {
      if (event.key === 'ArrowUp' || event.key === 'ArrowDown' || event.key === 'Escape') {
        event.preventDefault(); event.stopPropagation()
        setHidden(event.key !== 'ArrowDown')
      }
    }}
    onClick={() => {
      if (suppressClick.current) { suppressClick.current = false; return }
      if (collapsedRef.current) setHidden(false)
      else flip()
    }}>
    <span className="fox-expert-lanyard-extension" aria-hidden="true" />
    <span className="fox-expert-lanyard-scene" ref={host} data-ready={ready} />
    {!ready && <span className="fox-expert-lanyard-fallback" aria-hidden="true">
      <span className="fox-expert-lanyard-strap" /><span className="fox-expert-lanyard-clasp" />
      <span className={`fox-expert-lanyard-body ${front ? 'is-front' : ''}`}><span className="fox-expert-lanyard-face is-front"><span className="fox-expert-lanyard-slot" /><img src={frontImage} alt="" draggable={false} /><strong>{name}</strong><small>Fox Agent</small></span><span className="fox-expert-lanyard-face is-back"><ExpertCardBack active={!front} /><span className="fox-expert-lanyard-slot" /></span></span>
    </span>}
  </motion.button>
}
