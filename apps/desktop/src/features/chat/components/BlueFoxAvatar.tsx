import { useEffect, useId, useRef, useState } from 'react'
import type { FoxAnimation } from '../run-activity-presentation'
import type { BlueFoxEngine } from './blue-fox/engine'
import base from './blue-fox/fox-base-blue.png'
import idleEyes from './blue-fox/idle-eyes.svg'

export function BlueFoxAvatar({ state, progress, animate = true, startedAt }: { state: FoxAnimation; progress?: number; animate?: boolean; startedAt?: number | null }) {
  const fallbackFilter = `fox-avatar-fallback-${useId().replace(/:/g, '')}`
  const svg = useRef<SVGSVGElement>(null)
  const engine = useRef<BlueFoxEngine | null>(null)
  const currentState = useRef(state)
  currentState.current = state
  const currentProgress = useRef(progress)
  currentProgress.current = progress
  const [reduced, setReduced] = useState(() => matchMedia('(prefers-reduced-motion: reduce)').matches)
  const [ready, setReady] = useState(false)
  const motionEnabled = useRef(animate && !reduced)
  motionEnabled.current = animate && !reduced
  const currentStartedAt = useRef(startedAt)
  currentStartedAt.current = startedAt
  useEffect(() => {
    const media = matchMedia('(prefers-reduced-motion: reduce)')
    const update = () => setReduced(media.matches)
    media.addEventListener('change', update)
    return () => media.removeEventListener('change', update)
  }, [])
  useEffect(() => {
    if (!svg.current) return
    let disposed = false
    const target = svg.current
    const visibility = () => engine.current?.setPaused(document.hidden || !motionEnabled.current)
    document.addEventListener('visibilitychange', visibility)
    void import('./blue-fox/engine').then(({ BlueFoxEngine }) => {
      if (disposed) return
      target.replaceChildren()
      engine.current = new BlueFoxEngine(target, { followPointer: false, autoTricks: false, reduceMotion: !motionEnabled.current, initialState: currentState.current, thinkingStartedAt: currentStartedAt.current ?? undefined })
      engine.current.setProgress(currentProgress.current ?? 0)
      engine.current.setMotionEnabled(motionEnabled.current)
      visibility()
      setReady(true)
    }).catch(() => { if (!disposed) setReady(false) })
    return () => {
      disposed = true
      document.removeEventListener('visibilitychange', visibility)
      engine.current?.destroy()
      engine.current = null
    }
  }, [])
  useEffect(() => {
    engine.current?.setState(state)
    engine.current?.setMotionEnabled(animate && !reduced)
  }, [state, reduced, animate])
  useEffect(() => { engine.current?.setProgress(progress ?? 0) }, [progress])
  useEffect(() => { if (engine.current) engine.current.thinkingStartedAt = startedAt ?? undefined }, [startedAt])
  const thinkingFace = state === 'thinking' && (reduced || Math.floor(Math.max(0, Date.now() - (startedAt ?? Date.now())) / 6000) % 4 < 2)
  return <span className="fox-blue-avatar" aria-hidden="true" data-animation={state} data-animated={ready && animate && !reduced}>
    <svg data-avatar-fallback="true" viewBox="0 0 720 720" style={{ display: ready ? 'none' : undefined }}>
      <defs><filter id={fallbackFilter} colorInterpolationFilters="sRGB">
        <feColorMatrix in="SourceGraphic" type="matrix" values="0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 -5 0 5 0 -1" result="disk" />
        <feComposite in="disk" in2="SourceAlpha" operator="in" result="backdrop-mask" />
        <feFlood floodColor="var(--fox-avatar-color)" result="theme" />
        <feComposite in="theme" in2="backdrop-mask" operator="in" result="backdrop" />
        <feComposite in="SourceGraphic" in2="backdrop-mask" operator="out" result="fox" />
        <feComposite in="fox" in2="backdrop" operator="arithmetic" k2="1" k3="1" />
      </filter></defs>
      {thinkingFace || ['idle', 'happy', 'listening', 'searching'].includes(state) ? <>
        <image href={base} width="720" height="720" filter={`url(#${fallbackFilter})`} />
        <image href={idleEyes} width="720" height="720" />
      </> : <g fill="var(--fox-avatar-color)"><circle cx="200" cy="360" r="65" opacity=".65" /><circle cx="360" cy="360" r="65" /><circle cx="520" cy="360" r="65" opacity=".65" /></g>}
    </svg>
    <svg data-avatar-engine="true" ref={svg} viewBox="0 0 720 720" style={{ display: !ready ? 'none' : undefined }} />
  </span>
}
