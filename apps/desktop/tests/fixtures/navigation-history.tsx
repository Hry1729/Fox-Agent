import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import { Sidebar } from '../../src/features/chat/shell/fox-sidebar'
import { MemoizedWindowTitlebar } from '../../src/features/chat/shell/fox-window-titlebar'
import { useNavigationHistory } from '../../src/features/workspace/use-navigation-history'
import type { WorkspaceView } from '../../src/features/workspace/types'
import '../../src/styles/globals.css'
import '../../src/styles/themes.css'
import '../../src/styles/workbench.css'

const noop = () => {}
const items = [{ id: 'c-a', title: '对话 A', projectRoot: 'D:/项目 A', createdAt: 1, lastMessageAt: Date.now(), status: 'active' }, { id: 'c-b', title: '对话 B', projectRoot: null, createdAt: 2, lastMessageAt: Date.now(), status: 'active' }]
function Fixture() {
  const [collapsed, setCollapsed] = useState(true)
  const [route, setRoute] = useState<{ view: WorkspaceView; conversationId: string | null }>({ view: 'chat', conversationId: null })
  const [failure, setFailure] = useState('')
  const history = useNavigationHistory({ key: JSON.stringify(route), route }, async next => {
    await new Promise(resolve => setTimeout(resolve, 80))
    if ((window as any).failHistoryOnce) { (window as any).failHistoryOnce = false; throw new Error('页面已移除') }
    setRoute(next)
  }, error => setFailure(String(error)))
  const navigate = (view: WorkspaceView) => setRoute({ view, conversationId: null })
  ;(window as any).navigationFixture = { route, navigate, open: (id: string) => setRoute({ view: 'chat', conversationId: id }), failure, ...history }
  return <TooltipProvider><div className="fox-app fox-shell" style={{ height: '100vh', display: 'flex', flexDirection: 'column', '--fox-sidebar-width': '256px' } as any}>
    <MemoizedWindowTitlebar leftSidebarCollapsed={collapsed} onNewChat={() => navigate('chat')} onOpenProject={noop} onSettings={() => navigate('settings')} onAbout={() => navigate('settings-about')} onToggleSidebar={() => setCollapsed(value => !value)} onZoom={noop} canGoBack={history.canGoBack} canGoForward={history.canGoForward} onGoBack={history.back} onGoForward={history.forward} />
    <div style={{ display: 'flex', flex: 1, minHeight: 0 }}>
      <Sidebar collapsed={collapsed} assistantMode={route.view === 'local-knowledge-home' ? 'knowledge' : 'assistant'} activeView={route.view} newChatActive={!route.conversationId} activeConversationId={route.conversationId ?? undefined} runIndicators={new Map([['c-a', 'running'], ['c-b', 'completed']])} runtimeConversations={items as any} onModeChange={mode => navigate(mode === 'knowledge' ? 'local-knowledge-home' : 'chat')} onNewChat={() => navigate('chat')} onNewProjectChat={noop} onAddProject={noop} onOpenConversation={id => setRoute({ view: 'chat', conversationId: id ?? null })} onRenameConversation={noop} onPinConversation={noop} onArchiveConversation={noop} onUnarchiveConversation={noop} onTrashConversation={noop} onRestoreConversation={noop} onPurgeConversation={noop} onDeleteProject={noop} onNavigate={navigate} onExitManagement={() => navigate('chat')} onExpand={() => setCollapsed(false)} onTheme={noop} onSettings={() => navigate('settings')} />
      <main style={{ flex: 1, minWidth: 0, margin: '8px 8px 8px 0', borderRadius: 16, padding: 32, background: 'var(--background)' }}><h1>导航交互验证</h1><p data-testid="route">{route.view} · {route.conversationId ?? '新对话'}</p></main>
    </div>
  </div></TooltipProvider>
}
document.documentElement.dataset.colorTheme = 'graphite'
createRoot(document.getElementById('root')!).render(<Fixture />)
