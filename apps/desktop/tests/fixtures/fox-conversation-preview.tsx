import { useState, type CSSProperties } from 'react'
import { createRoot } from 'react-dom/client'
import { AssistantRuntimeProvider, useExternalStoreRuntime, type ThreadMessageLike } from '@assistant-ui/react'
import { mockIPC } from '@tauri-apps/api/mocks'
import { RotateCcw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Toaster } from '@/components/ui/sonner'
import { TooltipProvider } from '@/components/ui/tooltip'
import { ArtifactCardActionsContext } from '@/features/chat/artifact-card-menu'
import { MemoizedComposer } from '@/features/chat/composer/fox-composer'
import { ComposerEntryContext } from '@/features/chat/composer/composer-entry-context'
import { ComposerRuntimeContext, type ChatState, type OpenFileTab, type RightMode } from '@/features/chat/fox-shared'
import { ConversationSummaryFloat } from '@/features/chat/right-panel/conversation-summary-float'
import { RightPanel, createOpenFileTab } from '@/features/chat/right-panel/fox-right-panel'
import { useRightPanelNavigation } from '@/features/chat/right-panel/navigation'
import { ChatTopbar } from '@/features/chat/shell/fox-chat-topbar'
import { RightQuickActions } from '@/features/chat/shell/right-quick-actions'
import { Sidebar } from '@/features/chat/shell/fox-sidebar'
import AssistantUiChat from '@/features/chat/thread/fox-assistant-ui-chat'
import { ContactReplyBubble } from '@/features/companions/marks'
import { ContactsContext, type ContactsContextValue } from '@/features/companions/context'
import type { RuntimeQuestionRequest } from '@/features/conversations/model/pending-interactions'
import type { AgentRecord, ApprovalRecord, ArtifactRecord, ConversationDetail, ConversationMessage, ModelServiceRecord, ProjectRecord } from '@/features/conversations/model/types'
import { notify } from '@/features/notifications'
import { useModelService } from '@/features/settings/use-model-service'
import '@/styles/globals.css'
import '@/styles/themes.css'
import '@/styles/workbench.css'
import '@/styles/areas/chat.css'
import '@/styles/areas/composer.css'
import '@/styles/areas/sidebar.css'
import '@/styles/workspace-pages.css'

const now = Date.now()
const projectRoot = '\\\\?\\D:\\Fox 虚拟验收\\订单台账'
const reviewPath = 'output/review.md'
const agent = { id: 'fox-general', name: 'Fox 通用助手', runtimeType: 'pi', defaultModel: 'configured-model', resources: { tools: [], skills: [], knowledges: [], mcps: [] }, available: true, isDefault: true, packageManifest: {} } as unknown as AgentRecord
const project = { id: 'preview-project', name: '订单台账', rootPath: projectRoot, status: 'active', permissionMode: 'ask' } as ProjectRecord
let service = { name: 'DeepSeek', baseUrl: 'https://preview.invalid/v1', modelId: 'deepseek-v3.2', apiType: 'openai-completions', contextWindow: 65536, maxOutputTokens: 8192, supportsImageInput: false, enabled: true, credentialConfigured: true } as ModelServiceRecord
const models = [
  { id: 'v3', modelId: 'deepseek-v3.2', displayName: 'DeepSeek V3.2', contextWindow: 65536, maxOutputTokens: 8192 },
  { id: 'r1', modelId: 'deepseek-r1', displayName: 'DeepSeek R1', contextWindow: 65536, maxOutputTokens: 8192 },
]
const artifact: ArtifactRecord = {
  id: 'preview-review', runId: 'preview-report', conversationId: 'preview-chat', displayName: 'review.md', artifactType: 'created_file',
  artifactClass: 'deliverable', artifactOrigin: 'project', storagePath: projectRoot + '\\output\\review.md',
  mediaType: 'text/markdown', byteSize: 1240, sha256: null, status: 'ready', declaredAt: now, createdAt: now, updatedAt: now,
}
const officeArtifacts: ArtifactRecord[] = [
  { ...artifact, id: 'preview-docx', displayName: 'report.docx', storagePath: projectRoot + '\\output\\report.docx', mediaType: null },
  { ...artifact, id: 'preview-xlsx', displayName: 'ledger.xlsx', storagePath: projectRoot + '\\output\\ledger.xlsx', mediaType: null },
]
const ticket: ApprovalRecord = {
  id: 'preview-write-approval', runId: 'preview-write', conversationId: 'preview-chat', toolCallId: 'preview-write-file',
  toolName: 'write_file', requestedAction: '写入文件', status: 'pending', requestedAt: now, resolvedAt: null,
  request: { tool: 'write_file', input: { path: 'D:\\Fox 虚拟验收\\订单台账\\output\\summary.csv', content: '地区,订单金额\n华东,3500\n华南,2300' }, availableDecisions: ['deny', 'allow_once'] },
}
const request: RuntimeQuestionRequest = { runId: 'preview-question', questions: [{
  questionId: 'ledger', question: '工作区里没有找到「测试订单台账」文件，我该怎么处理？',
  options: [
    { value: 'create', label: '新建一个台账（CSV）并写入订单 O900' },
    { value: 'locate', label: '台账在别处，我来提供文件路径或上传文件' },
    { value: 'fields', label: '先列出 O900 需要哪些字段，确认后再建' },
  ],
  multiSelect: false, allowOther: true,
}] }
if (new URLSearchParams(location.search).has('question-pages')) request.questions.push(
  { questionId: 'grouping', question: '按什么维度汇总订单？', options: [{ value: 'region', label: '按地区' }, { value: 'month', label: '按月份' }], multiSelect: false, allowOther: true },
  { questionId: 'format', question: '你需要哪些交付格式？', options: [{ value: 'xlsx', label: 'Excel' }, { value: 'csv', label: 'CSV' }], multiSelect: true, allowOther: true },
)
const previewText = '# 台账核对记录\n\n- 已核对 8 笔订单。\n- 华东和华南两组数据已汇总。\n- 汇总表等待写入审批。'
const editorApps = [{ id: 'vscode', label: 'Visual Studio Code' }, { id: 'cursor', label: 'Cursor' }]
const fileEntry = { name: 'review.md', path: reviewPath, parent: 'output', isDirectory: false, byteSize: artifact.byteSize, modifiedAt: now }
const outputFiles = [fileEntry, ...officeArtifacts.map(file => ({ ...fileEntry, name: file.displayName, path: 'output/' + file.displayName }))]
const outputEntry = { name: 'output', path: 'output', parent: '', isDirectory: true, hasChildren: true, byteSize: 0, modifiedAt: now }

mockIPC((command, payload) => {
  const args = payload as { request?: { path?: string; action?: string; artifactId?: string; source?: { artifactId?: string; path?: string } } & Partial<ModelServiceRecord> } | undefined
  switch (command) {
    case 'kernel_reliability_snapshot': return { ok: true, data: { runId: 'preview-report', jobs: [], contextBudget: null, contextWindow: { usedTokens: 41359, limitTokens: 65536, source: 'session' } } }
    case 'model_providers_list': return { ok: true, data: [{ id: 'preview-deepseek', name: service.name, baseUrl: service.baseUrl, apiType: service.apiType, icon: 'deepseek.svg', enabled: true, models }] }
    case 'model_service_get': return { ok: true, data: service }
    case 'model_service_save': service = { ...service, ...args?.request }; return { ok: true, data: service }
    case 'project_folder_open': notify.info('模拟操作：打开项目文件夹'); return { ok: true, data: true }
    case 'project_files_list': return { ok: true, data: { entries: [outputEntry, ...outputFiles], truncated: false } }
    case 'project_files_children': return { ok: true, data: { entries: args?.request?.path ? outputFiles : [outputEntry], truncated: false } }
    case 'project_files_search': return { ok: true, data: { entries: outputFiles, truncated: false } }
    case 'project_file_read': return { ok: true, data: { path: reviewPath, name: 'review.md', content: previewText, byteSize: artifact.byteSize, truncated: false } }
    case 'conversation_office_preview_read': {
      const source = args?.request?.source
      const docx = source?.artifactId === 'preview-docx' || source?.path?.endsWith('.docx')
      const url = docx ? new URL('./preview-assets/conversation-preview.docx', import.meta.url) : new URL('./preview-assets/sample.xlsx', import.meta.url)
      return fetch(url).then(response => response.arrayBuffer())
    }
    case 'artifact_inspect':
    case 'artifact_action': {
      const file = officeArtifacts.find(file => file.id === args?.request?.artifactId) ?? artifact
      return { ok: true, data: { artifact: file, action: args?.request?.action, completed: true, applications: editorApps, capabilities: { preview: file === artifact, open: true, reveal: true, copyPath: true, openWith: true }, preview: { kind: 'text', available: file === artifact, content: file === artifact ? previewText : null, truncated: false, byteSize: file.byteSize, reason: null } } }
    }
    case 'project_file_action':
      return { ok: true, data: { action: args?.request?.action, completed: true, applications: editorApps } }
    default: return { ok: true, data: [] }
  }
}, { shouldMockEvents: true })

const scenarios = [
  ['approval', '待审批'],
  ['question', '四行提问'],
  ['files', '文件与模型'],
  ['companion', '伙伴选择'],
  ['new', '新对话'],
  ['sidebar', '左栏滚动'],
] as const
type SceneId = typeof scenarios[number][0]

function message(id: string, role: ConversationMessage['role'], content: string, ordinal: number, runId: string | null = null): ConversationMessage {
  return { id, conversationId: 'preview-chat', role, kind: 'text', content, status: 'completed', runId, ordinal, createdAt: now + ordinal, updatedAt: now + ordinal }
}

const guide = { title: '可以从这里开始', options: ['看看你能做什么', '帮我新建一个伙伴', '整理文件和资料'].map(label => ({ label, message: label })) }

function CompanionPreview() {
  const [answer, setAnswer] = useState('')
  const messages: ThreadMessageLike[] = answer ? [{ id: 'guide-answer', role: 'user', content: answer }] : []
  const runtime = useExternalStoreRuntime({ messages, convertMessage: message => message, isRunning: false, onNew: async () => {} })
  const context = { sendPrompt: async (text: string) => setAnswer(text) } as ContactsContextValue
  return <ContactsContext.Provider value={context}>
    <div className="fox-thread is-contact-chat fox-preview-companion">
      <div className="fox-contact-run-body"><AssistantRuntimeProvider runtime={runtime}>
        <ContactReplyBubble reply={{ id: 'welcome', text: '你好，我是小狐。我可以帮你处理文件、整理资料和日常问题。', createdAt: now, guide }} />
      </AssistantRuntimeProvider></div>
      {answer && <div className="fox-contact-run-body"><div className="fox-contact-message"><div className="fox-contact-bubble">收到，我们从「{answer}」开始。</div></div></div>}
    </div>
  </ContactsContext.Provider>
}

function Preview({ scene }: { scene: SceneId }) {
  const isNew = scene === 'new'
  const isCompanion = scene === 'companion'
  const showLeftSidebar = scene === 'sidebar'
  const showSummary = !isNew && !isCompanion
  const [summaryOpen, setSummaryOpen] = useState(showSummary)
  const nav = useRightPanelNavigation()
  const [decision, setDecision] = useState('')
  const [answer, setAnswer] = useState('')
  const [draft, setDraft] = useState('')
  const [sent, setSent] = useState<string[]>([])
  const [tabs, setTabs] = useState<RightMode[]>([])
  const [fileTabs, setFileTabs] = useState<OpenFileTab[]>([])
  const [activeFileTabId, setActiveFileTabId] = useState<string | null>(null)
  const modelService = useModelService().service
  const currentModel = models.find(model => model.modelId === modelService?.modelId)?.displayName ?? 'DeepSeek V3.2'
  const approvalPending = scene === 'approval' && !decision
  const questionPending = scene === 'question' && !answer
  const completedReport = scene === 'approval' || scene === 'files' || showLeftSidebar
  const sentMessages = sent.map((text, index) => message('preview-send-' + index, 'user', text, 10 + index))
  const messages = isNew ? sentMessages : [
    message('preview-u1', 'user', '检查订单台账，整理核对记录并生成汇总。', 1, completedReport ? 'preview-report' : null),
    message('preview-a1', 'assistant', completedReport
      ? '我已核对台账里的 8 笔订单。核对记录已保存为 [review.md](#fox-artifact=output/review.md)。'
      : '台账还没有出现在工作区里。确认处理方式后，我会继续。', 2, completedReport ? 'preview-report' : null),
    ...(scene === 'approval' ? [message('preview-u2', 'user', '把汇总写入 output/summary.csv。', 3, 'preview-write'),
      message('preview-a2', 'assistant', decision || '汇总已准备好，等你确认后写入文件。', 4, 'preview-write')] : []),
    ...(answer ? [message('preview-a3', 'assistant', '收到。' + answer, 5)] : []),
    ...sentMessages,
  ]
  const detail = {
    conversation: { id: 'preview-chat', title: isCompanion ? '小狐' : '订单台账核对', agentId: 'fox-general', agentName: 'Fox', projectId: project.id, projectRoot, permissionMode: 'ask', status: 'active', createdAt: now, updatedAt: now, lastMessageAt: now },
    messages, artifacts: scene === 'files' ? [artifact, ...officeArtifacts] : completedReport ? [artifact] : [],
    runtimeEvents: [], toolCalls: [], approvals: approvalPending ? [ticket] : [], attachments: [], knowledgeBindings: [],
    expertBindings: [], hasEarlierMessages: false, goals: [], tasks: [], evidence: [], planRevisions: [], reviewFindings: [], acceptances: [], childRuns: [], expertWorkflow: null, expertTeam: null,
    lastRun: isNew ? null : { id: completedReport ? 'preview-report' : 'preview-question', status: 'completed', model: currentModel, createdAt: now },
  } as ConversationDetail
  const desktop = { detail, enabled: true, selectedConversationId: detail.conversation.id, streamingText: '' } as Parameters<typeof AssistantUiChat>[0]['conversation']
  const switchPage = (mode: RightMode) => {
    nav.setMode(mode)
    nav.setCollapsed(false)
    setTabs(current => current.includes(mode) ? current : [...current, mode])
    setSummaryOpen(false)
    setActiveFileTabId(null)
  }
  const openFile = (path: string, artifactId?: string | null) => {
    switchPage('files')
    const file = createOpenFileTab(path, artifactId)
    setFileTabs(current => current.some(tab => tab.id === file.id) ? current : [...current, file])
    setActiveFileTabId(file.id)
  }
  const addMessage = (text: string) => setSent(current => [...current, text])
  const state: ChatState = approvalPending ? 'approval' : questionPending ? 'question' : 'complete'
  const sidebarConversations = Array.from({ length: 48 }, (_, index) => ({ ...detail.conversation, id: 'preview-sidebar-' + index, title: '台账核对 · 第 ' + (index + 1) + ' 批' }))
  return <div className="fox-preview-body">
    {showLeftSidebar && <Sidebar collapsed={false} assistantMode="work" activeView="chat" newChatActive={false}
      projects={[project]} runtimeConversations={sidebarConversations} activeConversationId={sidebarConversations[0].id}
      onModeChange={() => {}} onNewChat={() => {}} onNewProjectChat={() => {}} onAddProject={() => {}}
      onOpenConversation={() => {}} onRenameConversation={() => {}} onPinConversation={() => {}} onArchiveConversation={() => {}}
      onUnarchiveConversation={() => {}} onTrashConversation={() => {}} onRestoreConversation={() => {}} onPurgeConversation={() => {}}
      onDeleteProject={() => {}} onNavigate={() => {}} onExitManagement={() => {}} onExpand={() => {}} onTheme={() => {}} onSettings={() => {}} />}
    <main className="fox-content-card fox-preview-content">
    <div className="fox-content-surface"><section className="fox-chat-pane">
      {!isNew && <ChatTopbar conversation={detail.conversation} detail={detail} modelName={currentModel} state={state} rightSidebarCollapsed={nav.collapsed}
        summaryOpen={summaryOpen && nav.collapsed} onOpenSummary={() => { nav.collapse(); setSummaryOpen(current => !current) }} onRightSidebarExpand={() => { setSummaryOpen(false); nav.openGuide() }} onRightSidebarCollapse={nav.collapse} onOpenRightMode={switchPage}
        onPinConversation={() => {}} onRenameConversation={() => {}} onResetContext={() => {}} onArchiveConversation={() => {}} />}
      {showSummary && nav.collapsed && <RightQuickActions onMode={switchPage} />}
      <div className={'fox-chat-stage' + (isNew && sent.length === 0 ? ' is-empty' : '')} style={{ '--fox-composer-occlusion': questionPending ? '310px' : approvalPending ? '198px' : '80px' } as CSSProperties}>
        {isCompanion ? <CompanionPreview /> : <ArtifactCardActionsContext.Provider value={{ projectRoot, onAttachFile: path => setDraft(current => (current ? current.trimEnd() + ' ' : '') + '@' + path + ' ') }}>
          <div className="fox-thread-shell"><AssistantUiChat conversation={desktop} callbacks={{ onSubmit: addMessage }}
            onOpenArtifact={record => openFile('output/' + record.displayName, record.id)} onOpenFileInSidebar={openFile} /></div>
        </ArtifactCardActionsContext.Provider>}
        {!isCompanion && <ComposerRuntimeContext.Provider value={{ knowledgeBindings: [], onKnowledge: () => notify.info('这是模拟资料库') }}>
          <ComposerEntryContext.Provider value={{ projects: [project], defaultExpert: agent, onSelectProject: () => {}, onAddProject: () => notify.info('这是模拟项目'), onViewDefaultExpert: () => notify.info('Fox 通用助手') }}>
            <MemoizedComposer resetKey={0} runtimeControlled draft={draft} setDraft={setDraft} chatState={state} newConversation={isNew && sent.length === 0} centered={isNew && sent.length === 0}
              activeAgent={agent} projectRoot={isNew ? null : projectRoot} projectPermissionMode="ask" modelService={modelService}
              runtimeApprovals={approvalPending ? new URLSearchParams(location.search).has('approval-pages')
                ? [ticket, { ...ticket, id: 'preview-approval-second', requestedAt: ticket.requestedAt + 1 }]
                : [ticket] : []} onResolveApproval={async (_id, value) => {
                setDecision(value === 'deny' ? '已拒绝写入。' : '已允许本次写入。预览中已模拟保存汇总表。'); return true
              }}
              questionRequest={questionPending ? request : null} onSubmitQuestion={async text => { setAnswer(text); return true }}
              onChangeExpert={() => notify.info('这里显示当前的默认 Fox 助手')} onSubmitPrompt={text => { addMessage(text); return 'complete' }} />
          </ComposerEntryContext.Provider>
        </ComposerRuntimeContext.Provider>}
      </div>
    </section></div>
    {showSummary && nav.collapsed && summaryOpen && <div className="fox-preview-summary-dock">
      <ConversationSummaryFloat detail={detail} modelName={currentModel} onMode={switchPage} onClose={() => setSummaryOpen(false)} />
    </div>}
    {!nav.collapsed && <RightPanel detail={detail} mode={nav.mode} tabs={tabs} width={330} maximized={nav.maximized}
      fileTabs={fileTabs} activeFileTabId={activeFileTabId} activeChildAgent={null} activity={{ goal: null, focusedEvidence: null }}
      onMode={switchPage} onCloseMode={mode => { setTabs(current => current.filter(tab => tab !== mode)); nav.collapse() }}
      onOpenFile={openFile} onActivateFile={id => { nav.setMode('files'); setActiveFileTabId(id) }}
      onCloseFile={id => { setFileTabs(current => current.filter(tab => tab.id !== id)); setActiveFileTabId(null); nav.collapse() }}
      onOpenChildAgent={() => {}} onBackChildAgent={() => {}} onToggleMaximized={() => nav.setMaximized(value => !value)} onCollapse={nav.collapse} />}
    </main>
  </div>
}

function App() {
  const [scene, setScene] = useState<SceneId>('approval')
  const [restart, setRestart] = useState(0)
  return <TooltipProvider><div className="fox-app fox-shell fox-preview">
    <header className="fox-preview-bar"><strong>Fox 虚拟测试</strong><span>本机模拟数据</span>
      <nav aria-label="测试场景">{scenarios.map(([id, label]) => <Button key={id} size="sm" variant={scene === id ? 'default' : 'ghost'} aria-pressed={scene === id} onClick={() => setScene(id)}>{label}</Button>)}</nav>
      <Button size="sm" variant="outline" onClick={() => setRestart(value => value + 1)}><RotateCcw />重置场景</Button>
    </header>
    <Preview key={scene + restart} scene={scene} />
    <Toaster />
    <style>{'.fox-preview{height:100dvh;display:flex;flex-direction:column;background:var(--shell);padding:8px;gap:8px}.fox-preview-bar{display:flex;flex-wrap:wrap;align-items:center;gap:12px;padding:4px 10px;color:var(--fox-text);font-size:var(--fox-type-meta)}.fox-preview-bar>span{color:var(--fox-muted)}.fox-preview-bar>nav{display:flex;flex-wrap:wrap;gap:4px;flex:1}.fox-preview-body{display:flex;flex:1;min-width:0;min-height:0;gap:8px}.fox-preview-body>.fox-sidebar{width:240px}.fox-preview-content{margin:0}.fox-preview-summary-dock{flex:0 0 318px;min-width:0;border-left:1px solid var(--fox-border);background:var(--fox-card)}.fox-preview-summary-dock>.fox-summary-float{position:relative;inset:auto;width:100%;max-height:100%;border:0;border-radius:0;box-shadow:none}.fox-preview-companion{max-width:760px;margin:24px auto;padding:0 20px}.fox-preview-bar button:focus-visible{outline:var(--fox-focus-outline);outline-offset:var(--fox-focus-offset)}@media(max-width:720px){.fox-preview-summary-dock{flex-basis:min(35vw,260px)}.fox-preview-content>.fox-context-panel{max-width:55vw}.fox-preview-bar{gap:6px}}'}</style>
  </div></TooltipProvider>
}

document.documentElement.dataset.colorTheme = 'graphite'
createRoot(document.getElementById('root')!).render(<App />)
