import { BlueFoxFallback } from '../features/chat/components/blue-fox/BlueFoxFallback'
import type { ReactNode, Ref } from 'react'
import { Minus, Square, X } from 'lucide-react'

export type WindowAction = 'minimize' | 'toggleMaximize' | 'close'

type Props = {
  avatar?: ReactNode
  surfaceRef?: Ref<HTMLDivElement>
  avatarRef?: Ref<HTMLSpanElement>
  onWindowAction?: (action: WindowAction) => void
}

export function StartupLayout({ avatar, surfaceRef, avatarRef, onWindowAction }: Props) {
  return <div className="fox-startup-screen">
    <header className="fox-startup-titlebar" data-tauri-drag-region="">
      <span data-tauri-drag-region=""><img src="/app-logo.png" alt="" />Fox</span>
      <div>
        <button aria-label="最小化" disabled={!onWindowAction} onClick={() => onWindowAction?.('minimize')}><Minus /></button>
        <button aria-label="最大化或还原" disabled={!onWindowAction} onClick={() => onWindowAction?.('toggleMaximize')}><Square /></button>
        <button aria-label="关闭" disabled={!onWindowAction} onClick={() => onWindowAction?.('close')}><X /></button>
      </div>
    </header>
    <div className="fox-startup-body" ref={surfaceRef}>
      <section className="fox-startup-hero" role="status" aria-label="正在准备工作区">
        <h1 className="fox-startup-brand" aria-label="Fox Agent">
          <span>F</span><span ref={avatarRef} className="fox-startup-avatar">{avatar ?? <BlueFoxFallback
            base="/src/features/chat/components/blue-fox/fox-base-blue.png" eyes="/src/features/chat/components/blue-fox/idle-eyes.svg"
            fallbackFilter="fox-startup-face-color" showFace />}</span><span>x Agent</span>
        </h1>
        <p>正在准备工作区</p>
        <div className="fox-startup-progress" aria-hidden="true"><span /></div>
      </section>
    </div>
  </div>
}
