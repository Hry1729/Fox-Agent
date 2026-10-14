import { useEffect, useRef, useState } from 'react'
import type { ExpertLanyardScene } from './expert-lanyard-scene'
import './expert-lanyard.css'

/** Native button gives click, Enter and Space the same flip. Three stays off the
 * chat's eager bundle and renders only on setup, resize, image load and flipping. */
export function ExpertLanyard({ expertId, name, frontImage }: { expertId: string; name: string; frontImage: string }) {
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
        scene.current = createExpertLanyardScene(host.current, frontImage, unavailable)
        if (!frontRef.current) scene.current.flip(false)
        setReady(true)
      } catch { unavailable() }
    }).catch(unavailable)
    return () => { cancelled = true; scene.current?.dispose(); scene.current = null }
  }, [expertId, frontImage])
  const flip = () => {
    const next = !frontRef.current
    frontRef.current = next; setFront(next); scene.current?.flip(next)
  }
  return <button type="button" className="fox-expert-lanyard" aria-label={`${name}专家挂卡，${front ? '正面，点击查看背面' : '背面，点击查看头像'}`} aria-pressed={front} title={`${name} · 点击翻转`} onClick={flip}>
    <span className="fox-expert-lanyard-scene" ref={host} data-ready={ready} />
    {!ready && <span className="fox-expert-lanyard-fallback" aria-hidden="true">
      <span className="fox-expert-lanyard-strap" /><span className="fox-expert-lanyard-strap is-right" /><span className="fox-expert-lanyard-clasp" />
      <span className={`fox-expert-lanyard-body ${front ? 'is-front' : ''}`}><span className="fox-expert-lanyard-face is-front"><span className="fox-expert-lanyard-slot" /><img src={frontImage} alt="" draggable={false} /></span><span className="fox-expert-lanyard-face is-back"><span className="fox-expert-lanyard-slot" /></span></span>
    </span>}
  </button>
}
