import { ConversationSummaryFloat } from '../../src/features/chat/right-panel/conversation-summary-float'
import { ArtifactCardActionsContext } from '../../src/features/chat/artifact-card-menu'
import { MemoizedRuntimeAssistantMessage } from '../../src/features/chat/timeline/fox-runtime-assistant-message'
import { useEffect, useState } from 'react'
import { CONTACT_SIDE_CARD_WIDTH } from '../../src/features/companions/contact-side-card'
import { mockIPC } from '@tauri-apps/api/mocks'
import { RightPanel, createOpenFileTab } from '../../src/features/chat/right-panel/fox-right-panel'
import { ChatTopbar } from '../../src/features/chat/shell/fox-chat-topbar'
import { RightQuickActions } from '../../src/features/chat/shell/right-quick-actions'
import { useRightPanelNavigation } from '../../src/features/chat/right-panel/navigation'
import { ResizeDivider } from '../../src/features/chat/shell/fox-resize-divider'
import type { RightMode, OpenFileTab, ChildAgentSelection } from '../../src/features/chat/fox-shared'
import type { ConversationDetail, ChildRunRecord, ToolCallRecord } from '../../src/features/conversations/model/types'
import type { GoalProgressData } from '../../src/features/chat/hooks/use-goal-progress'
import { TooltipProvider } from '../../src/components/ui/tooltip'

const now = Date.now()
const date = new Date(now).toISOString()
const evidence: ConversationDetail['evidence'][number] = { id: 'e1', taskId: 't1', sourceRunId: 'r1', evidenceType: 'test_result', refKind: 'run_event', refId: 'event1', summary: '资料来源与附件已核对。', metadata: {}, validityStatus: 'valid', traceId: null, spanId: null, checkedAt: date, invalidReason: null, createdAt: date }
const goal: GoalProgressData = {
  goal: { id: 'g1', conversationId: 'panel-fixture', title: '完成资料核对', objective: '核对资料和附件', acceptanceSummary: null, status: 'active', version: 1, createdBy: 'user', createdAt: date, updatedAt: date, completedAt: null, blockedReason: null },
  tasks: [{ id: 't1', goalId: 'g1', parentTaskId: null, ordinal: 1, title: '核对附件', detail: null, status: 'completed', ownerRunId: 'r1', attempt: 1, version: 1, blockedReason: null, createdAt: date, updatedAt: date, startedAt: date, finishedAt: date }],
  evidence: [evidence], planRevisions: [{ id: 'p1', goalId: 'g1', conversationId: 'panel-fixture', revision: 1, title: '核对计划', summary: '', tasks: [], status: 'approved', createdBy: 'user', createdAt: date, approvedAt: date }],
  reviewFindings: [], acceptances: [], expertWorkflow: null, expertTeam: null, completedCount: 1, totalCount: 2, currentTask: null,
}
const longName = '年度资料整理与附件核对结果以及需要复核的详细说明文件.md'
const tool = (id: string, name: string, status = 'completed') => ({
  id, runId: 'r1', conversationId: 'panel-fixture', toolName: name, status,
  input: name === 'run_command' ? { command: location.search.includes('long-command') ? `python check.py ${' --validate data/review.xlsx'.repeat(24)}` : 'python check.py', cwd: 'D:/验收项目' } : { path: longName, oldText: '未核对', newText: '已核对' },
  result: { stdout: '核对完成\n8 项检查通过', exitCode: 0 }, startedAt: now - 10000, completedAt: status === 'running' ? null : now,
  errorMessage: null, runtimeToolCallId: id,
} as ToolCallRecord)
const agent = (id: string, status: ChildRunRecord['status']): ChildRunRecord => ({
  id, parentRunId: 'r1', childRunId: id, childConversationId: 'child-fixture', workerAgentId: 'reviewer',
  workerAgentName: status === 'running' ? '资料核对助手' : '写作助手', objective: '核对资料的来源与附件，说明需要复核的差异。',
  context: '', teamRunId: null, teamMemberId: null, allowedTools: [], status, depth: 1,
  budget: { maxDurationMs: 600000, maxTotalTokens: 10000, maxOutputTokens: 1000, maxToolCalls: 10 },
  resultText: '已完成资料核对。', inputTokens: 0, outputTokens: 0, totalTokens: 0, toolCallCount: 1,
  errorCode: status === 'failed' ? 'timeout' : null, errorMessage: null, createdAt: now, startedAt: now, finishedAt: status === 'running' ? null : now,
})
const detail = {
  conversation: { id: 'panel-fixture', title: '右侧栏验收', agentId: 'fox-general', agentName: 'Fox', projectRoot: new URLSearchParams(location.search).has('verbatim') ? '\\\\?\\D:\\验收项目' : 'D:/验收项目', projectId: null, permissionMode: 'ask', status: 'active', createdAt: now, updatedAt: now, lastMessageAt: now },
  messages: [], attachments: [], approvals: [], expertBindings: [], goals: [], evidence: [], planRevisions: [], reviewFindings: [], acceptances: [],
  hasEarlierMessages: false, expertWorkflow: null, expertTeam: null,
  lastRun: { id: 'r1', status: 'running', startedAt: now - 10000, errorMessage: null },
  tasks: [{ id: 't1', title: '读取资料', status: 'completed' }, { id: 't2', title: '核对附件', status: 'in_progress' }, { id: 't3', title: '整理结果', status: 'queued' }],
  artifacts: [{ id: 'a1', displayName: longName, storagePath: 'D:/验收项目/' + longName, artifactType: 'created_file', byteSize: 2048, createdAt: now, updatedAt: now }],
  knowledgeBindings: [{ knowledgeBaseId: 'kb1', knowledgeBaseName: '项目资料', enabled: true }],
  runtimeEvents: [{ runId: 'r1', seq: 1, eventType: 'source.added', createdAt: now, event: { id: 's1', title: '附件核对说明', excerpt: '本次核对使用已提供的项目资料。对于缺失的附件，应记录缺失项，并在最终结果中说明需要复核的内容。这里的摘录用于检查最多两行的截断显示。' } }],
  toolCalls: [tool('c1', 'edit_file'), tool('c2', 'run_command'), tool('c3', 'web_search'), tool('c4', 'read_file'), tool('c5', 'read_file'), tool('c6', 'run_command', 'running')],
  childRuns: [agent('child1', 'running'), agent('child2', 'completed'), agent('child3', 'failed')],
} as unknown as ConversationDetail

export function installRightPanelFixture() {
  const params = new URLSearchParams(location.search)
  const state = params.get('state') || 'ready'
  const empty = params.get('frame') === 'empty' || state === 'empty'
  const artifactState = params.get('artifact')
    mockIPC(async (command, payload: any) => {
      if (params.has('artifact-menu') && command === 'project_file_action') {
        document.body.dataset.lastFileAction = JSON.stringify(payload.request)
      }
      if (command === 'artifact_inspect' || command === 'artifact_action') {
        if (artifactState === 'stale') return { ok: false, error: { code: 'artifact.integrity_mismatch', message: '产物文件大小已发生变化', retryable: false } }
        if (artifactState === 'missing') return { ok: false, error: { code: 'artifact.file_missing', message: '产物文件不存在', retryable: false } }
        if (command === 'artifact_action' && payload?.request?.action === 'open') return { ok: false, error: { code: 'artifact.open_failed', message: '系统没有可用的打开方式', retryable: true } }
        return { ok: true, data: { action: payload?.request?.action, completed: true, artifact: detail.artifacts[0], capabilities: { preview: true, open: true, reveal: true, copyPath: true, openWith: true }, applications: [{ id: 'vscode', label: 'Visual Studio Code' }], preview: { kind: 'text', available: true, content: '# 核对结果\n\n这是回答时交付的版本。\n\n- 附件 03 金额与明细表不一致\n- 附件 06 少了运输费', truncated: false, byteSize: 2048, reason: null } } }
      }
      if (state === 'loading' && ['project_files_list', 'project_files_children', 'project_files_search', 'project_file_read', 'managed_file_versions_list', 'conversation_load'].includes(command)) return new Promise(() => {})
      if (state === 'error' && ['project_files_list', 'project_files_children', 'project_files_search', 'project_file_read', 'managed_file_versions_list', 'conversation_load'].includes(command)) return { ok: false, error: { code: 'fixture', message: '读取失败，请重试', retryable: true } }
      const version = { id: 'v2', runId: 'r1', toolCallId: 'c1', tool: 'edit_file', storagePath: 'D:/验收项目/' + longName, displayName: longName, versionNo: 2, changeKind: 'modified', beforeHash: 'old', beforeSize: 1200, afterHash: 'new', afterSize: 2048, backupPath: 'backup', restoredFromId: null, createdAt: now, backupAvailable: true, currentHash: 'new', drifted: false, deleted: false }
      const childDetail = { ...detail, messages: [{ id: 'u1', role: 'user', content: detail.childRuns[0].objective, createdAt: now }, { id: 'm1', role: 'assistant', content: '已核对资料来源。\n\n- 附件齐全。\n- 一项金额需要复核。', createdAt: now }], toolCalls: [tool('ct1', 'read_file')] }
      const query = String(payload?.request?.query ?? '').trim().toLocaleLowerCase()
      const responses: Record<string, unknown> = {
        project_files_list: { entries: empty ? [] : [{ path: longName, name: longName, parent: '', isDirectory: false }], truncated: false },
        project_files_children: { entries: empty ? [] : params.get('tree') === 'overflow' ? (payload?.request?.path ? [{ path: `${payload.request.path}/nested.json`, name: 'nested.json', parent: payload.request.path, isDirectory: false, hasChildren: false }] : [{ path: 'docs', name: 'docs', parent: '', isDirectory: true, hasChildren: true }, { path: 'Fox', name: 'Fox', parent: '', isDirectory: true, hasChildren: true }, ...Array.from({ length: 75 }, (_, i) => ({ path: `资料整理与附件核对-${i}.md`, name: `资料整理与附件核对-${i}.md`, parent: '', isDirectory: false, hasChildren: false }))]) : [{ path: longName, name: longName, parent: '', isDirectory: false, hasChildren: false }], truncated: false },
        project_files_search: { entries: empty || !longName.toLocaleLowerCase().includes(query) ? [] : [{ path: longName, name: longName, parent: '', isDirectory: false, hasChildren: false }], truncated: false },
        project_file_read: { path: longName, name: longName, content: '# 核对结果\n\n8 项检查已完成。\n\n这是实际预览组件的测试文本。', byteSize: 2048, truncated: true },
        project_file_action: { action: payload?.request?.action || 'inspect', completed: true, applications: [{ id: 'vscode', label: 'Visual Studio Code' }] },
        managed_file_versions_list: empty ? [] : [version, { ...version, id: 'v1', versionNo: 1, changeKind: 'created', createdAt: now - 60000, afterHash: 'old' }],
        kernel_reliability_snapshot: params.has('pi-budget')
          ? { runId: 'r1', jobs: [], contextBudget: null, contextWindow: { usedTokens: 41359, limitTokens: 65536, source: 'last_request' } }
          : { runId: 'r1', jobs: [], contextBudget: { modelWindowTokens: 200000, historyTokens: 78000, toolsTokens: 14000, systemTokens: 7000, pendingAppendTokens: 0, outputReserveTokens: 8000, protocolOverheadTokens: 1300, safetyMarginTokens: 7000, availableTokens: 84700, pressureRatio: 0.5765, estimateSource: 'heuristic', updatedAt: now } },
        permission_audit_list: empty ? [] : [{ id: 'pa1', tool: 'write_file', mode: 'auto', reason: 'mode', ruleIds: [], risk: 'high', summary: 'docs/核对结果.md', findings: [], createdAt: now }], conversation_load: childDetail, project_folder_open: true,
      }
      return { ok: true, data: responses[command] ?? [] }
    }, { shouldMockEvents: true })
}

export function RightPanelFixture() {
  const params = new URLSearchParams(location.search)
  const frame = params.get('frame') || 'summary'
  const state = params.get('state') || 'ready'
  const { mode, setMode, collapsed, setCollapsed, maximized, setMaximized, openGuide: openSidebar, collapse } = useRightPanelNavigation(frame === 'summary' || frame === 'empty' || frame === 'closed' ? null : frame === 'file-preview' ? 'files' : frame === 'agent-chat' ? 'agents' : frame as RightMode, frame === 'closed' || frame === 'summary')
  const [tabs, setTabs] = useState<RightMode[]>(mode ? [mode] : [])
  const [files, setFiles] = useState<OpenFileTab[]>(frame === 'file-preview' ? [createOpenFileTab(longName, params.get('artifact') ? 'a1' : null)] : [])
  const [activeFile, setActiveFile] = useState<string | null>(files[0]?.id ?? null)
  const [child, setChild] = useState<ChildAgentSelection | null>(frame === 'agent-chat' ? { run: detail.childRuns[0], avatarIndex: 0 } : null)
  const [width, setWidth] = useState(Number(params.get('width')) || CONTACT_SIDE_CARD_WIDTH)
  const [summaryOpen, setSummaryOpen] = useState(frame === 'summary')
  const empty = frame === 'empty' || state === 'empty'
  const snapshot = empty ? { ...detail, conversation: { ...detail.conversation, projectRoot: null }, messages: [], tasks: [], artifacts: [], knowledgeBindings: [], runtimeEvents: [], toolCalls: [], childRuns: [], lastRun: null }
    : params.get('artifact') ? { ...detail, artifacts: detail.artifacts.map(item => ({ ...item, declaredDescription: '资料核对结果，列出需要复核的附件和金额差异。', delivery: { versionNo: 1, verificationStatus: 'declared', summary: null } })) } as ConversationDetail
    : detail
  useEffect(() => {
    document.documentElement.classList.toggle('dark', params.get('mode') === 'dark')
    document.documentElement.dataset.colorTheme = params.get('theme') || 'graphite'

  }, [])
  const openSummary = () => setSummaryOpen(value => !value)
  const openMode = (value: RightMode) => { setCollapsed(false); setTabs(current => current.includes(value) ? current : [...current, value]); setMode(value); setActiveFile(null); setChild(null) }
  if (params.has('artifact-menu')) {
    const record = { ...detail.artifacts[0], conversationId: detail.conversation.id, runId: 'r1', artifactOrigin: 'project', displayName: 'review.md', storagePath: 'D:/验收项目/output/review.md', declaredAt: now }
    const message = { id: 'm1', conversationId: detail.conversation.id, runId: 'r1', role: 'assistant', kind: 'text', status: 'completed', content: '[review.md](#fox-artifact=output/review.md)', createdAt: now, updatedAt: now } as ConversationDetail['messages'][number]
    return <TooltipProvider><main className="fox-shell" style={{ padding: 36 }}>
      <ArtifactCardActionsContext.Provider value={{ projectRoot: detail.conversation.projectRoot, onAttachFile: path => { document.body.dataset.attached = path } }}>
        <MemoizedRuntimeAssistantMessage message={message} processEvents={[]} groupHelpers={null} singleRunMessage running={false} artifacts={[record]} assistantName="Fox" onOpenArtifact={artifact => { document.body.dataset.opened = artifact.id }} />
      </ArtifactCardActionsContext.Provider>
    </main></TooltipProvider>
  }
  return <TooltipProvider><div className="fox-shell right-panel-fixture">
    <nav className="fixture-controls"><span>右侧栏生产组件 · 测试数据</span><button onClick={() => document.documentElement.classList.toggle('dark')}>切换深浅色</button><button onClick={() => { document.documentElement.dataset.colorTheme = document.documentElement.dataset.colorTheme === 'paper' ? 'graphite' : 'paper' }}>切换主题</button></nav>
    <div className="fixture-layout">
      <main className="fox-content-card"><div className="fox-content-surface"><section className="fox-chat-pane">{!empty && <ChatTopbar conversation={snapshot.conversation} detail={snapshot} modelName="测试模型" state="complete" rightSidebarCollapsed={collapsed} summaryOpen={summaryOpen} onOpenSummary={openSummary} onRightSidebarExpand={openSidebar} onRightSidebarCollapse={collapse} onOpenRightMode={openMode} onPinConversation={() => {}} onRenameConversation={() => {}} onResetContext={() => {}} onArchiveConversation={() => {}} />}{!empty && collapsed && <RightQuickActions onMode={openMode} />}{!empty && summaryOpen && <ConversationSummaryFloat detail={snapshot} modelName="测试模型" onMode={openMode} onClose={() => setSummaryOpen(false)} />}<p style={{ padding: 24 }}>逐页检查卡片、标签、文件预览与异常状态。拖动右侧分隔条可调整宽度。</p></section></div>
      {frame !== 'empty' && (!collapsed || tabs.includes('terminal')) && <>{!collapsed && !maximized && <ResizeDivider className="fox-resize-divider" label="调整右侧面板宽度" width={width} direction={-1} measure={() => ({ width, min: 280, max: 760 })} onResize={setWidth} />}<RightPanel hidden={collapsed} detail={snapshot} mode={mode} tabs={tabs} width={width} maximized={maximized} fileTabs={files} activeFileTabId={activeFile} activeChildAgent={child} activity={params.get('goal') === '1' ? { goal, focusedEvidence: null, onDeleteGoal: async () => true, onGoalRunningChange: async () => true } : { goal: null, focusedEvidence: null }} onMode={openMode} onCloseMode={value => { setTabs(current => current.filter(tab => tab !== value)); collapse() }} onOpenFile={(path, artifactId) => { const file = createOpenFileTab(path, artifactId); setFiles(current => current.some(tab => tab.id === file.id) ? current : [...current, file]); setActiveFile(file.id); setMode('files') }} onActivateFile={id => { setMode('files'); setActiveFile(id) }} onCloseFile={id => { setFiles(current => current.filter(file => file.id !== id)); setActiveFile(null); collapse() }} onOpenChildAgent={(run, avatarIndex) => setChild({ run, avatarIndex })} onBackChildAgent={() => setChild(null)} onToggleMaximized={() => setMaximized(value => !value)} onCollapse={collapse} /></>}
    </main></div>
    <style>{`.right-panel-fixture{height:100vh;display:flex;flex-direction:column;background:var(--shell)}.right-panel-fixture .fixture-controls{display:flex;gap:16px;padding:10px 20px;font-size:12px}.right-panel-fixture .fixture-layout{display:flex;flex:1;min-height:0;padding:12px;gap:5px}.right-panel-fixture .fox-content-card{flex:1;min-width:0;margin:0}.right-panel-fixture .fox-context-panel{height:100%;margin:0}.right-panel-fixture .fox-context-panel.is-maximized{position:relative;inset:auto;flex:1;width:auto}`}</style>
  </div></TooltipProvider>
}
