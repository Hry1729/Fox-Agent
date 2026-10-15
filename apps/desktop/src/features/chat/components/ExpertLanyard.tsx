import { useEffect, useRef, useState } from 'react'
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

/** Native button gives click, Enter and Space the same flip. Three stays off the
 * chat's eager bundle; the silver sweep runs only while the back is visible. */
export function ExpertLanyard({ expertId, name, frontImage, description = '随时为你提供协助' }: { expertId: string; name: string; frontImage: string; description?: string }) {
  const host = useRef<HTMLSpanElement>(null)
  const scene = useRef<ExpertLanyardScene | null>(null)
  const frontRef = useRef(true)
  const [front, setFront] = useState(true)
  const [ready, setReady] = useState(false)
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
  return <button type="button" className="fox-expert-lanyard" aria-label={`${name}专家挂卡，${front ? '正面，点击查看背面' : '背面，点击查看头像'}`} aria-pressed={front} title={`${name} · ${description} · 点击翻转`} onClick={flip}>
    <span className="fox-expert-lanyard-scene" ref={host} data-ready={ready} />
    {!ready && <span className="fox-expert-lanyard-fallback" aria-hidden="true">
      <span className="fox-expert-lanyard-strap" /><span className="fox-expert-lanyard-strap is-right" /><span className="fox-expert-lanyard-clasp" />
      <span className={`fox-expert-lanyard-body ${front ? 'is-front' : ''}`}><span className="fox-expert-lanyard-face is-front"><span className="fox-expert-lanyard-slot" /><img src={frontImage} alt="" draggable={false} /><strong>{name}</strong><small>Fox Agent</small></span><span className="fox-expert-lanyard-face is-back"><ExpertCardBack active={!front} /><span className="fox-expert-lanyard-slot" /></span></span>
    </span>}
  </button>
}
