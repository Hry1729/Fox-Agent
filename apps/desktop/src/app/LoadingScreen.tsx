import { useLayoutEffect, useRef } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { Minus, Square, X } from 'lucide-react'
import { BlueFoxAvatar } from '@/features/chat/components/BlueFoxAvatar'
import { FOX_ASSISTANT_AVATAR } from '@/features/chat/components/FoxAssistantAvatar'
import './loading-screen.css'

export function LoadingScreen() {
  const surface = useRef<HTMLDivElement>(null)
  const avatar = useRef<HTMLSpanElement>(null)
  useLayoutEffect(() => {
    const place = () => {
      if (!surface.current || !avatar.current) return
      const body = surface.current.getBoundingClientRect(), mark = avatar.current.getBoundingClientRect()
      surface.current.style.setProperty('--fox-loading-ox', `${mark.left + mark.width / 2 - body.left}px`)
      surface.current.style.setProperty('--fox-loading-oy', `${mark.top + mark.height / 2 - body.top}px`)
    }
    const observer = new ResizeObserver(place)
    if (surface.current) observer.observe(surface.current)
    if (avatar.current) observer.observe(avatar.current)
    void document.fonts.ready.then(place)
    return () => observer.disconnect()
  }, [])
  const windowAction = (action: 'minimize' | 'toggleMaximize' | 'close') => {
    try { void getCurrentWindow()[action]().catch(() => {}) } catch { /* No native window in browser previews. */ }
  }
  return <div className="fox-startup-screen">
    <header className="fox-startup-titlebar" data-tauri-drag-region><span data-tauri-drag-region><img src={FOX_ASSISTANT_AVATAR} alt="" />Fox</span><div><button aria-label="最小化" onClick={() => windowAction('minimize')}><Minus /></button><button aria-label="最大化或还原" onClick={() => windowAction('toggleMaximize')}><Square /></button><button aria-label="关闭" onClick={() => windowAction('close')}><X /></button></div></header>
    <div className="fox-startup-body" ref={surface}><section className="fox-startup-hero" role="status" aria-label="正在准备工作区">
      <h1 className="fox-startup-brand" aria-label="Fox Agent"><span>F</span><span ref={avatar} className="fox-startup-avatar"><BlueFoxAvatar state="thinking" /></span><span>x Agent</span></h1>
      <p>正在准备工作区</p><div className="fox-startup-progress" aria-hidden="true"><span /></div>
    </section></div>
  </div>
}
