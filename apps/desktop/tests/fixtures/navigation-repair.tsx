import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import { Sidebar } from '../../src/features/chat/shell/fox-sidebar'
import { MemoizedWindowTitlebar } from '../../src/features/chat/shell/fox-window-titlebar'
import '../../src/styles/globals.css'
import '../../src/styles/themes.css'
import '../../src/styles/workbench.css'

const noop = () => {}
const items = Array.from({ length: 200 }, (_, i) => ({ id: `c-${i}`, title: `对话 ${i + 1}：很长的测试标题用于验证浮层文字不会横向溢出`, projectRoot: i < 160 ? `D:/项目-${i < 90 ? '01' : String(i - 88).padStart(2, '0')}` : null, createdAt: i, lastMessageAt: Date.now(), status: 'active' }))
const record = (action: string) => { (window as any).navigationFixture.action = action }
function Fixture() {
  const [collapsed, setCollapsed] = useState(true)
  ;(window as any).navigationFixture = { collapsed, toggle: () => setCollapsed(value => !value), action: (window as any).navigationFixture?.action }
  return <TooltipProvider><div className="fox-app fox-shell" style={{ height: '100vh', display: 'flex', flexDirection: 'column', '--fox-sidebar-width': '288px' } as any}>
    <MemoizedWindowTitlebar leftSidebarCollapsed={collapsed} onNewChat={() => record('new')} onOpenProject={() => record('project')} onSettings={() => record('settings')} onAbout={() => record('about')} onToggleSidebar={() => setCollapsed(value => !value)} onZoom={value => record(`zoom:${value}`)} />
    <div style={{ display: 'flex', flex: 1, minHeight: 0 }}>
      <Sidebar collapsed={collapsed} assistantMode="assistant" activeView="chat" newChatActive runtimeConversations={items as any} onModeChange={noop} onNewChat={noop} onNewProjectChat={noop} onAddProject={noop} onOpenConversation={record} onRenameConversation={noop} onPinConversation={noop} onArchiveConversation={noop} onUnarchiveConversation={noop} onTrashConversation={noop} onRestoreConversation={noop} onPurgeConversation={noop} onDeleteProject={noop} onNavigate={noop} onExitManagement={noop} onExpand={() => setCollapsed(false)} onTheme={noop} onSettings={noop} />
      <main style={{ flex: 1, minWidth: 0, margin: '8px 8px 8px 0', borderRadius: 16, padding: 32, background: 'var(--background)' }}><h1>导航交互验证</h1><p>71 个项目、一个项目内 90 个对话、40 个独立对话。</p></main>
    </div>
  </div></TooltipProvider>
}
document.documentElement.dataset.colorTheme = 'graphite'
createRoot(document.getElementById('root')!).render(<Fixture />)
