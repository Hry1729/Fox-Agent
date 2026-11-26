// Browser-only acceptance scene. Production sidebar and companion components;
// local records replace the Host so no model, remote service or database is used.
import { useState } from 'react'
import { mockIPC } from '@tauri-apps/api/mocks'
import { createRoot } from 'react-dom/client'
import type { ContactsContextValue } from '../../src/features/companions/context'
import type { Companion } from '../../src/features/companions/api'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import { isCompanionWorkspaceView, type NavigateWorkspace, type WorkspaceView } from '../../src/features/workspace/types'
import type { AppNotificationRecord, GlobalSearchRecord } from '../../src/features/conversations/model/types'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'
import '../../src/styles/workspace-pages.css'
import '../../src/styles/areas/sidebar.css'

const params = new URLSearchParams(window.location.search)
const prototype = params.has('prototype')
if (prototype) Object.assign(globalThis, { isTauri: true })
const theme = params.get('theme') ?? 'graphite'
const dark = params.get('mode') === 'dark'
document.documentElement.dataset.colorTheme = theme
document.documentElement.classList.toggle('dark', dark)
document.documentElement.style.colorScheme = dark ? 'dark' : 'light'

const companion = (id: string, displayName: string, patch: Partial<Companion> = {}): Companion => ({
  id, conversationId: `chat-${id}`, templateAgentId: null, displayName, avatarRef: '🦊', duty: `${displayName}的职责`,
  personaOverride: null, workspaceDir: `D:/acceptance/${id}`, workspaceState: 'ready', workspaceError: null,
  isDefault: false, pinnedAt: null, workShortcutOrder: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  reminderMode: 'follow_global', lastReadDeliveryOrder: null, manualUnreadFromOrder: null, manualUnreadAt: null,
  readDeliveryOrder: 0, pendingActionCount: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
  ...patch,
})
let records = [
  companion('default', '小狐', { isDefault: true, avatarRef: '🦊', duty: '你的默认工作伙伴' }),
  companion('writer', '周报伙伴', { avatarRef: '周', duty: '整理进展，写出清楚的周报', pinnedAt: 40, workShortcutOrder: 1, activeRunStatus: 'running' }),
  companion('research', '资料伙伴', { avatarRef: '📚', duty: '查找、核对并归纳资料', pinnedAt: 30, workShortcutOrder: 2, unreadCount: 3 }),
  companion('table', '表格伙伴', { avatarRef: '表', duty: '清洗数据并生成表格', pinnedAt: 20, workShortcutOrder: 3, pendingActionCount: 1 }),
  companion('planner', '计划伙伴', { avatarRef: '计', duty: '拆解目标并跟踪计划', pinnedAt: 10 }),
  companion('review', '审校伙伴', { avatarRef: '审', duty: '检查文字、事实和格式', latestFailureSummary: '模型服务超时',
    lastMessageAt: Date.now() - 60_000, latestBubbleId: 'review-message', latestDeliveryOrder: 10 }),
  companion('meeting', '会议伙伴', { avatarRef: '会', duty: '整理会议记录与行动项', unreadCount: 1,
    lastMessageAt: Date.now() - 120_000, latestBubbleId: 'meeting-message', latestDeliveryOrder: 10 }),
  companion('archive', '归档伙伴', { avatarRef: '档', archivedAt: new Date(2026, 10, 3).getTime() }),
]
if (params.has('overflow')) records = [...records, ...Array.from({ length: 16 }, (_, index) => companion(`extra-${index + 1}`, `候选伙伴 ${index + 1}`))]
const persistedOrderKey = 'fox.sidebar.acceptance.pinned-order'
if (params.has('persistence')) {
  const order = JSON.parse(sessionStorage.getItem(persistedOrderKey) ?? '[]') as string[]
  records = records.map(item => order.includes(item.id) ? { ...item, pinnedOrder: order.indexOf(item.id) + 1 } : item)
}
const noop = () => {}
const now = Date.now()
let notices: AppNotificationRecord[] = [
  { id: 'notice-writer', mergeKey: 'fixture-writer', kind: 'completed', severity: 'normal', title: '周报整理完成',
    body: '已按项目和负责人归纳进度，并列出尚待确认的事项与下周的跟进安排。', sourceType: 'companion', sourceId: 'writer',
    workspaceView: 'companions', entityId: 'writer', progress: null, status: 'completed', action: {}, readAt: null, createdAt: now, updatedAt: now },
  { id: 'notice-table', mergeKey: 'fixture-table', kind: 'approval', severity: 'high', title: '需要确认文件写入',
    body: '伙伴请求将整理后的项目进度保存到共享目录。请先核对计划写入的文件和变更内容。', sourceType: 'companion', sourceId: 'table',
    workspaceView: 'companions', entityId: 'table', progress: null, status: 'pending', action: {}, readAt: null, createdAt: now - 1, updatedAt: now - 1 },
]
const searchItems: GlobalSearchRecord[] = [
  { id: 'search-research', kind: 'companion', title: '资料伙伴', subtitle: '查找、核对并归纳资料', workspaceView: 'companions', entityId: 'research', documentId: null, updatedAt: 3 },
  { id: 'search-writer', kind: 'companion', title: '周报伙伴', subtitle: '整理进展，写出清楚的周报', workspaceView: 'companions', entityId: 'writer', documentId: null, updatedAt: 2 },
  { id: 'search-chat', kind: 'conversation', title: '资料整理对话', subtitle: '普通对话', workspaceView: 'chat', entityId: 'chat-research', documentId: null, updatedAt: 1 },
]
mockIPC((command, args) => {
  const request = (args as { request: { companionId: string; ids: string[]; displayName: string; avatarRef: string } }).request
  if (prototype) {
    if (command === 'app_notifications_list') return { ok: true, data: notices }
    if (command === 'app_notifications_mark_all_read') { notices = notices.map(item => ({ ...item, readAt: now })); return { ok: true, data: notices.length } }
    if (command === 'app_notifications_clear_read') { notices = notices.filter(item => !item.readAt); return { ok: true, data: 1 } }
    if (command === 'notification_preferences_get') return { ok: true, data: { badge: true, sound: false } }
    if (command === 'conversation_load') return { ok: true, data: { conversation: { id: (request as unknown as { conversationId: string }).conversationId, permissionMode: 'ask' }, approvals: [], toolCalls: [], runtimeEvents: [], runs: [] } }
    if (command === 'companion_task_list') return { ok: true, data: request.companionId === 'writer' ? [{
      task: { id: 'weekly', name: '周五进展周报', conversationId: 'chat-writer', objective: '汇总本周项目进展。' },
      schedule: { id: 'weekly-schedule', scheduleKind: 'weekly', weekdays: [5], localTime: '17:00', timezone: 'Asia/Shanghai', enabled: true, nextDueAt: now + 86_400_000 },
    }] : [] }
    if (command === 'companion_task_history') return { ok: true, data: [{ status: 'completed' }] }
  }
  if (command === 'agents_list') return { ok:true, data:['汇报材料专家','会议纪要专家','制度与流程撰写专家','技术文档专家','数据分析专家','前端开发专家','后端开发专家','软件架构专家','代码审查专家','工业物联网方案专家','UI 设计专家','用户体验专家','演示与信息图专家','品牌与视觉规范专家','调研与资料核验专家','产品需求专家','项目计划与推进专家','提示词优化专家','学习规划专家','翻译与术语专家'].map((name,index) => ({id:`expert-${index}`,name,description:`${name}，帮助整理资料、核对内容并完成交付。`,available:true,runtimeType:'pi',agentKind:'expert',resources:{knowledges:[],tools:[],mcps:[],skills:[]},packageManifest:{}})) }
  if (command === 'companion_next_default_name') return { ok: true, data: 'Fox' }
  if (command === 'companion_create_blank') {
    const item = companion('new', request.displayName, { avatarRef: request.avatarRef })
    records = [...records, item]
    return { ok: true, data: item }
  }
  if (command === 'companion_update_profile') {
    records = records.map(item => item.id === request.companionId ? { ...item, displayName: request.displayName } : item)
  }
  if (command === 'companion_set_pinned_order') {
    records = records.map(item => request.ids.includes(item.id) ? { ...item, pinnedOrder: request.ids.indexOf(item.id) + 1 } : item)
    if (params.has('persistence')) sessionStorage.setItem(persistedOrderKey, JSON.stringify(request.ids))
    return { ok: true, data: null }
  }
  if (command === 'companion_mark_read' || command === 'companion_mark_unread' || command === 'companion_restore') {
    records = records.map(item => item.id !== request.companionId ? item : command === 'companion_restore'
      ? { ...item, archivedAt: null }
      : { ...item, unreadCount: command === 'companion_mark_read' ? 0 : 1 })
  }
  return { ok: true, data: request?.companionId ? records.find(item => item.id === request.companionId) : null }
}, { shouldMockEvents: true })

const { Sidebar } = await import('../../src/features/chat/shell/fox-sidebar')
const { ContactsContext } = await import('../../src/features/companions/context')
const { ArchivedCompanionComposer } = await import('../../src/features/companions/chat-controls')
const { default: CompanionWorkspace } = await import('../../src/features/companions/workspace')
const { useWorkspaceSchedules } = await import('../../src/features/companions/tasks')

function Scene() {
  const [activeView, setActiveView] = useState<WorkspaceView>(prototype ? 'companion-inbox' : params.get('view') === 'work' ? 'chat' : 'companions')
  const [activeId, setActiveId] = useState('chat-writer')
  const [collapsed, setCollapsed] = useState(params.get('collapsed') === '1')
  const [companions, setCompanions] = useState(records)
  const [notificationsOpen, setNotificationsOpen] = useState(false)
  const [notificationHost, setNotificationHost] = useState<HTMLElement | null>(null)
  const schedules = useWorkspaceSchedules(companions, prototype)
  const current = companions.find(item => item.conversationId === activeId)
  const context: ContactsContextValue = {
    current, privateChat: true, list: { companions, pendingDeletions: [] }, projects: [], model: 'acceptance', permission: 'ask', error: '',
    refresh: async () => setCompanions(records), refreshConversation: async () => {}, open: item => { setActiveId(item.conversationId); if (prototype) setActiveView('companions') }, showProfile: noop,
    leave: noop, setPermission: noop, openSharedMemory: noop, askToOrganize: noop,
  }
  const navigate: NavigateWorkspace = (view, entityId, target) => {
    setNotificationsOpen(view === 'notifications')
    if (view === 'notifications') return
    setActiveView(view)
    if (view === 'companions' && (entityId || target?.conversationId)) setActiveId(target?.conversationId ?? `chat-${entityId}`)
  }
  return <ContactsContext.Provider value={context}><div className="fox-shell companion-acceptance-scene">
    <Sidebar
      collapsed={collapsed} assistantMode={isCompanionWorkspaceView(activeView) ? 'companions' : 'work'}
      onModeChange={mode => navigate(mode === 'knowledge' ? 'local-knowledge-home' : mode === 'companions' ? 'companions' : 'chat')}
      onNewChat={noop} onNewProjectChat={noop} onAddProject={noop}
      onOpenConversation={id => setActiveId(id ?? 'chat-writer')} onRenameConversation={noop}
      onPinConversation={noop} onArchiveConversation={noop} onUnarchiveConversation={noop}
      onTrashConversation={noop} onRestoreConversation={noop} onPurgeConversation={noop}
      onDeleteProject={noop} onExitManagement={() => setActiveView('chat')}
      onExpand={() => setCollapsed(false)} onTheme={noop} onSettings={noop}
      runtimeConversations={[]} activeConversationId={activeId} newChatActive={false}
      activeView={activeView} onNavigate={navigate} enabledTaskCount={schedules.enabledCount}
      notificationsOpen={notificationsOpen} notificationHost={notificationHost} onCloseNotifications={() => setNotificationsOpen(false)}
      onGlobalSearch={async query => searchItems.filter(item => `${item.title}${item.subtitle}`.toLocaleLowerCase().includes(query.toLocaleLowerCase()))}
    />
    {notificationsOpen ? <main className="fox-chat-pane" ref={setNotificationHost} /> :
    prototype && isCompanionWorkspaceView(activeView) && activeView !== 'companions'
      ? <main className="fox-chat-pane"><CompanionWorkspace view={activeView} schedules={schedules} navigate={navigate} /></main> :
    <main className="companion-acceptance-main">
      <span>伙伴页面 · 本地验收数据</span>
      <h1>{activeView === 'companions' ? '和伙伴继续工作' : '今天的工作'}</h1>
      <p>生产侧栏组件 · {theme} · {dark ? '深色' : '浅色'}</p>
      <button className="fixture-sidebar-toggle" type="button" onClick={() => setCollapsed(value => !value)}>{collapsed ? '展开侧栏' : '收起侧栏'}</button>
      {current?.archivedAt && <ArchivedCompanionComposer contact={current} />}
    </main>}
    <style>{`
      body{margin:0;background:var(--fox-canvas)}
      .companion-acceptance-scene{display:flex;height:100vh;min-height:760px}
      .companion-acceptance-scene>.fox-sidebar:not(.is-collapsed){width:var(--sidebar-acceptance-width,236px);flex:0 0 var(--sidebar-acceptance-width,236px);height:100vh}
      .companion-acceptance-main{position:relative;flex:1;min-width:0;padding:56px;background:var(--fox-canvas);color:var(--fox-text)}
      .companion-acceptance-main>span{font-size:var(--fox-type-caption);color:var(--fox-muted)}
      .companion-acceptance-main h1{margin:12px 0;font-size:28px;font-weight:var(--fox-weight-semibold)}
      .companion-acceptance-main p{font-size:var(--fox-type-ui);color:var(--fox-muted)}
      .fixture-sidebar-toggle{margin-top:24px;padding:8px 12px;border:1px solid var(--fox-border);border-radius:var(--fox-radius-control);background:var(--fox-card);color:var(--fox-text)}
    `}</style>
  </div></ContactsContext.Provider>
}

createRoot(document.getElementById('root')!).render(<TooltipProvider><Scene /></TooltipProvider>)
