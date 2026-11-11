import { useEffect, useState } from 'react'
import { createRoot } from 'react-dom/client'
import { mockIPC } from '@tauri-apps/api/mocks'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import AssistantUiChat from '../../src/features/chat/thread/fox-assistant-ui-chat'
import { ContactSidebar, ContactTopbar } from '../../src/features/companions/chat-controls'
import { AgentCard, type AgentCardData } from '../../src/features/workspace/entity-card'
import { ContactSideCard } from '../../src/features/companions/contact-side-card'
import { ContactApprovalCard } from '../../src/features/companions/chat-decision'
import { FoxApprovalControl } from '../../src/features/chat/composer/composer-approval-control'
import { MemoizedComposer } from '../../src/features/chat/composer/fox-composer'
import { ContactsContext, type ContactsContextValue } from '../../src/features/companions/context'
import { useContactChatSearch } from '../../src/features/companions/chat-search'
import type { Companion } from '../../src/features/companions/api'
import type { ConversationDetail } from '../../src/features/conversations/model/types'
import '../../src/styles/globals.css'
import '../../src/styles/themes.css'
import '../../src/styles/workbench.css'
import '../../src/styles/workspace-pages.css'
import '../../src/styles/areas/pages.css'

let contact: Companion = {
  id: 'ui-contact', conversationId: 'ui-chat', templateAgentId: 'fox-general', displayName: '小狐',
  avatarRef: '/app-logo.png', duty: '整理资料，协助写作与编程', personaOverride: null,
  latestMessagePreview: '今天已经整理好了资料，接下来可以继续核对表格中的数据和相关附件。',
  workspaceDir: 'D:/验收工作目录', workspaceState: 'ready', workspaceError: null,
  isDefault: true, pinnedAt: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  reminderMode: 'follow_global', lastReadDeliveryOrder: 0, manualUnreadFromOrder: null, manualUnreadAt: null,
  readDeliveryOrder: 0, pendingActionCount: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
}
// Only the desktop IPC boundary is simulated; the renderer uses production components.
mockIPC((command, payload: any) => {
  if (command === 'companion_update_profile') {
    contact = { ...contact, ...payload.request }
    return { ok: true, data: contact }
  }
  const responses: Record<string, unknown> = {
    model_providers_list: [{ id: 'test-provider', name: '测试服务', enabled: true, models: [{ id: 'test-model', displayName: '测试模型', supportsImageInput: true }] }], managed_file_versions_list: [], project_files_list: [],
    companion_files_list: { provided: [], generated: [] },
    companion_grants_list: [], companion_template_status: { updateAvailable: false },
    companion_chat_search: { hits: [], total: 0 },
  }
  return { ok: true, data: responses[command] ?? [] }
})
const now = Date.now()
const detail: ConversationDetail = {
  conversation: { id: 'ui-chat', agentId: 'fox-general', agentName: 'Fox', title: 'UI 验收', projectId: null,
    projectRoot: null, permissionMode: 'ask', status: 'active', createdAt: now, updatedAt: now, lastMessageAt: now },
  companionId: 'ui-contact',
  messages: [
    { id: 'u1', conversationId: 'ui-chat', runId: 'r1', role: 'user', kind: 'text', content: '你可以做些什么？', status: 'completed', ordinal: 1, createdAt: now, updatedAt: now },
    { id: 'a1', conversationId: 'ui-chat', runId: 'r1', role: 'assistant', kind: 'text', content: '你好，我是小狐。我们可以从你的资料和工作目标开始。\n\n## 日常对话与创作\n\n- 回答问题，解释概念，翻译和润色。\n- 起草邮件、报告和演示文稿。\n- 梳理任务，检查结果并继续修改。\n\n## 文件与代码\n\n1. 阅读你授权的工作目录。\n2. 分析数据，整理表格并生成文件。\n3. 根据需求修改代码，执行验证。\n\n这是一段用于检查气泡最大宽度的较长文字。消息应随聊天区域变宽，列表圆点应保持在气泡内部。', status: 'completed', ordinal: 2, createdAt: now, updatedAt: now },
  ],
  runtimeEvents: [], toolCalls: [], approvals: [], attachments: [], artifacts: [], knowledgeBindings: [],
  expertBindings: [], hasEarlierMessages: false, goals: [], tasks: [], evidence: [], planRevisions: [],
  reviewFindings: [], acceptances: [], childRuns: [], expertWorkflow: null, expertTeam: null, lastRun: null,
}
function Scene() {
  const [current, setCurrent] = useState(contact)
  const [side, setSide] = useState(false)
  const [empty, setEmpty] = useState(false)
  const [dark, setDark] = useState(false)
  useEffect(() => { document.documentElement.classList.toggle('dark', dark) }, [dark])
  const [decision, setDecision] = useState('')
  const [draft, setDraft] = useState('')
  const snapshot = empty ? { ...detail, messages: [] } : detail
  const search = useContactChatSearch({ conversationId: 'ui-chat', detail: snapshot, loadEarlierMessages: async () => {} })
  const ctx: ContactsContextValue = {
    current, privateChat: true, list: { companions: [current], pendingDeletions: [] }, projects: [], model: '测试模型', permission: 'ask', error: '',
    refresh: async () => { setCurrent({ ...contact }) }, refreshConversation: async () => {},
    open: () => {}, add: () => {}, showProfile: () => setSide(true), leave: () => {},
    setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
  }
  if (location.search.includes('avatars')) return <TooltipProvider><ContactsContext.Provider value={ctx}>
    <div className="fox-shell" style={{ padding: 32, display: 'flex', gap: 48, alignItems: 'flex-start' }}>
      <div className="fox-sidebar-list-pane" style={{ width: 260 }}><ContactSidebar activeId="ui-chat" /></div>
      <div className="fox-agent-list-page"><AgentCard agent={{ id: 'sample', name: '资料整理专家', description: '协助整理资料和核对文件。', image: '/app-logo.png', available: true, tools: 0, knowledge: 0, mcps: 0, skills: 0 } as AgentCardData} onUse={() => {}} /></div>
    </div>
  </ContactsContext.Provider></TooltipProvider>
  return <TooltipProvider><ContactsContext.Provider value={ctx}>
    <div className={'fox-shell ui-refresh-fixture' + (dark ? ' dark' : '')}>
      <nav className="fixture-controls" aria-label="验收场景"><span>生产组件 · 模拟数据</span><button onClick={() => setEmpty(!empty)}>{empty ? '显示消息' : '空联系人'}</button><button onClick={() => setDark(!dark)}>切换深浅色</button><FoxApprovalControl catalog={null} value="ask" /></nav>
      <div className="fixture-layout">
        <main className="fox-content-card is-contact-chat">
          <div className="fox-chat-pane">
            <ContactTopbar detail={snapshot} search={search} />
            <div className="fox-chat-stage" style={{ '--fox-composer-occlusion': '130px' } as React.CSSProperties}>
              <div className="fox-thread-shell"><AssistantUiChat conversation={{ enabled: true, detail: snapshot, streamingText: '' } as any} callbacks={{ onSubmit: () => {} }} approvalSlot={!empty && <div className="fox-thread-msg"><div className="fox-contact-decision-entry"><ContactApprovalCard title="读取工作目录" description="检查审批卡与气泡的宽度。" onAllowOnce={() => setDecision('已允许一次')} onDeny={() => setDecision('已拒绝')} state={decision ? 'done' : 'request'} statusLabel={decision} /></div></div>} /></div>
              <MemoizedComposer resetKey={0} draft={draft} setDraft={setDraft} chatState="complete" entryFrozen newConversation={false} />
            </div>
          </div>
        </main>
        {side && <ContactSideCard contact={current} detail={snapshot} onClose={() => setSide(false)} onResetContext={() => {}} search={search} />}
      </div>
    </div>
    <style>{`.ui-refresh-fixture{height:100vh;display:flex;flex-direction:column;background:var(--fox-surface);color:var(--fox-text)}.fixture-controls{display:flex;align-items:center;gap:16px;padding:8px 20px;font-size:12px}.fixture-controls button{border:1px solid var(--fox-border);border-radius:6px;padding:4px 9px}.fixture-layout{display:flex;flex:1;min-height:0;gap:8px;padding:8px}.fixture-layout>.fox-content-card{margin:0;flex:1;min-width:0;container-type:inline-size;container-name:fox-chat}.fixture-layout .fox-chat-pane{height:100%;display:flex;flex-direction:column;position:relative}.fixture-layout .fox-chat-stage{flex:1;min-height:0}.fixture-layout>.fox-contact-side-card{flex:0 0 320px}`}</style>
  </ContactsContext.Provider></TooltipProvider>
}
createRoot(document.getElementById('root')!).render(<Scene />)
