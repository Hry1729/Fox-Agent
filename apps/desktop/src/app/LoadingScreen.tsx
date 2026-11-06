import { useLayoutEffect, useRef } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { StartupLayout, type WindowAction } from './StartupLayout'
import { BlueFoxAvatar } from '@/features/chat/components/BlueFoxAvatar'
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
  const windowAction = (action: WindowAction) => {
    try { void getCurrentWindow()[action]().catch(() => {}) } catch { /* No native window in browser previews. */ }
  }
  return <StartupLayout surfaceRef={surface} avatarRef={avatar} onWindowAction={windowAction}
    avatar={<BlueFoxAvatar state="thinking" />} />
}
