import { createRoot } from 'react-dom/client'
import { useState } from 'react'
import { BlueFoxAvatar } from '../../src/features/chat/components/BlueFoxAvatar'
import { FoxRunStatusBar } from '../../src/features/chat/components/FoxRunStatusBar'
import { BlueFoxEngine } from '../../src/features/chat/components/blue-fox/engine'
import type { FoxAnimation } from '../../src/features/chat/run-activity-presentation'
import '../../src/styles/globals.css'
import '../../src/features/chat/components/fox-run-status-bar.css'

const states: FoxAnimation[] = ['thinking', 'sending', 'receiving', 'loading', 'orbit', 'radar', 'progress', 'writing', 'searching', 'listening', 'happy', 'humming']
function Fixture() {
  const [revision, remount] = useState(0)
  const [detail, update] = useState<any>({ conversation: { id: 'c' }, lastRun: { id: 'r', status: 'running', startedAt: Date.now() }, messages: [], runtimeEvents: [], toolCalls: [], approvals: [] })
  Object.assign(window, { motionFixture: { remount: () => remount(v => v + 1), update, detail, BlueFoxEngine } })
  return <main style={{ padding: 32, color: 'var(--foreground)', background: 'var(--background)', minHeight: '100vh' }}>
    <h1>Fox 动效检查</h1><p>真实组件；状态由测试数据驱动。</p>
    <div style={{ display: 'grid', gridTemplateColumns: 'repeat(auto-fit,minmax(190px,1fr))', gap: 24, maxWidth: 900 }}>
      {states.map(state => <div key={state} data-case={state} style={{ display: 'flex', alignItems: 'center', gap: 8 }}><BlueFoxAvatar key={revision} state={state} progress={65} startedAt={Date.now() - 13000} /><span>{state}</span></div>)}
    </div>
    <div style={{ marginTop: 40, '--fox-chat-column-width': '100%' } as any}><FoxRunStatusBar detail={detail} conversationId="c" /></div>
    <svg id="engine-probe" style={{ width: 44, height: 44, marginTop: 40 }} />
  </main>
}
createRoot(document.getElementById('root')!).render(<Fixture />)
