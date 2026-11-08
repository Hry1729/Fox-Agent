/**
 * 用 Fox 桌面端的真实 React 组件 + 真实测试夹具渲染静态 HTML 片段。
 *
 * 运行（必须从 apps/desktop 目录启动，保证 tsconfig 的 @ 别名与 node_modules 解析）：
 *   cd apps/desktop && bun "docs/评审/Fox界面改进交接/验证/render-real.tsx"
 *
 * 产物：验证/真实组件片段.json（供预览页 / 构建脚本内联使用）。
 * 不修改任何产品代码；所有片段都是当前源码渲染出来的 DOM。
 */
import { writeFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const nm = '../../../../apps/desktop/node_modules/'
const src = '../../../../apps/desktop/src/'

const { GlobalRegistrator } = await import(nm + '@happy-dom/global-registrator')
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import(nm + 'react')
const { render, cleanup, fireEvent } = await import(nm + '@testing-library/react')

type Fragment = { html: string; from: string; note: string }
const fragments: Record<string, Fragment> = {}
const errors: Record<string, string> = {}

function capture(key: string, element: any, from: string, note: string) {
  try {
    const view = render(element)
    const html = view.container.innerHTML
    cleanup()
    if (!html || html.length < 20) throw new Error('empty render output')
    fragments[key] = { html, from, note }
    console.log(`OK   ${key.padEnd(24)} ${String(html.length).padStart(7)} chars`)
  } catch (error) {
    errors[key] = String(error instanceof Error ? error.stack || error.message : error).slice(0, 1200)
    console.log(`FAIL ${key.padEnd(24)} ${String(error).slice(0, 160)}`)
    try { cleanup() } catch { /* ignore */ }
  }
}

/** 真实组件默认把工具输出折叠；点开真实的 Collapsible 触发器后再取 DOM，
 *  这样「取消后输出仍可见」是组件自己渲染的，不是另外画一个输出框。 */
function captureWithToolOpen(key: string, element: any, from: string, note: string) {
  try {
    const view = render(element)
    const triggers = Array.from(view.container.querySelectorAll('[data-slot="tool-call"] [data-slot="collapsible-trigger"]'))
    triggers.forEach((trigger) => fireEvent.click(trigger))
    const html = view.container.innerHTML
    cleanup()
    fragments[key] = { html, from, note }
    console.log(`OK   ${key.padEnd(24)} ${String(html.length).padStart(7)} chars (工具展开 ${triggers.length} 处)`)
  } catch (error) {
    errors[key] = String(error instanceof Error ? error.stack || error.message : error).slice(0, 1200)
    console.log(`FAIL ${key.padEnd(24)} ${String(error).slice(0, 160)}`)
    try { cleanup() } catch { /* ignore */ }
  }
}

const noop = () => {}
const asyncNoop = async () => false

// ---- 真实事件夹具：13.38 A-2 / tests/runtime-tool-settled.test.ts 的 ping 取消序列 ----
const event = (seq: number, eventType: string, data: Record<string, unknown> = {}): any => ({
  runId: 'run', seq, eventType, event: { type: eventType, ...data }, createdAt: seq,
})
const ping = [
  event(1, 'run.started'),
  event(2, 'tool.started', { toolCallId: 'p1', tool: 'bash', input: { command: 'ping -n 60 127.0.0.1' } }),
  event(3, 'tool.updated', {
    toolCallId: 'p1',
    update: {
      status: 'in_progress',
      text: '正在 Ping 127.0.0.1 具有 32 字节的数据:\n来自 127.0.0.1 的回复: 字节=32 时间<1ms TTL=128\n来自 127.0.0.1 的回复: 字节=32 时间<1ms TTL=128\n',
      streamedOutput: true,
    },
  }),
]
const cancelled = [...ping, event(4, 'run.cancelled')]
const emptyCancelled = [
  event(1, 'run.started'),
  event(2, 'tool.started', { toolCallId: 'p2', tool: 'bash', input: { command: 'ping -n 60 127.0.0.1' } }),
  event(3, 'run.cancelled'),
]
const baseTime = 1_700_000_000_000
const userMessage = {
  id: 'user', conversationId: 'conversation', runId: 'run', role: 'user', kind: 'text',
  content: '用 run_command 执行 ping -n 60 127.0.0.1', status: 'completed', ordinal: 1,
  createdAt: baseTime, updatedAt: baseTime,
}
const assistantMessage = (status: string, content: string) => ({
  id: 'assistant-run', conversationId: 'conversation', runId: 'run', role: 'assistant', kind: 'text',
  content, status, ordinal: 2, createdAt: baseTime + 1, updatedAt: baseTime + 4,
})

// ---- 1. 真实时间线（当前代码对取消事实的渲染） ----
const { RuntimeTimeline } = await import(src + 'features/chat/timeline/fox-runtime-timeline.tsx')
const timeline = (events: any[], overrides: Record<string, unknown> = {}) =>
  React.createElement(RuntimeTimeline, {
    messages: [userMessage, assistantMessage('completed', '正在执行 ping，我会把结果贴给你。')],
    attachments: [], artifacts: [], events, runtimeRunning: false, state: 'idle', streamingText: '',
    onRetry: noop, onRerun: asyncNoop, ...overrides,
  } as any)

capture('timeline-cancelled', timeline(cancelled),
  'features/chat/timeline/fox-runtime-timeline.tsx',
  '真实事件夹具：run.started → tool.started → tool.updated(部分输出) → run.cancelled')
capture('timeline-cancelling', timeline(ping, { runtimeRunning: true, state: 'streaming', activeRunId: 'run' }),
  'features/chat/timeline/fox-runtime-timeline.tsx',
  '同一夹具去掉 run.cancelled 终态：仅发出取消请求，未收到终态')
capture('timeline-empty-cancelled', timeline(emptyCancelled),
  'features/chat/timeline/fox-runtime-timeline.tsx',
  '真实夹具变体：取消前没有收到任何输出')
capture('timeline-running', timeline(ping, { runtimeRunning: true, state: 'streaming', activeRunId: 'run' }),
  'features/chat/timeline/fox-runtime-timeline.tsx',
  '运行中（未取消）基线')
capture('timeline-draft', React.createElement(RuntimeTimeline, {
  messages: [userMessage], attachments: [], artifacts: [], events: [],
  runtimeRunning: false, state: 'idle', streamingText: '', onRetry: noop, onRerun: asyncNoop,
} as any), 'features/chat/timeline/fox-runtime-timeline.tsx',
  '发送前失败：只有已发出的用户消息，没有 Run 与助手消息')
captureWithToolOpen('timeline-cancelled-open', timeline(cancelled),
  'features/chat/timeline/fox-runtime-timeline.tsx',
  '同一取消夹具，点击真实工具卡折叠触发器后的 DOM（输出由真实组件渲染）')
captureWithToolOpen('timeline-empty-cancelled-open', timeline(emptyCancelled),
  'features/chat/timeline/fox-runtime-timeline.tsx',
  '无输出取消夹具，展开真实工具卡后的 DOM')

// ---- 2. 真实输入框（新对话 / 历史对话） ----
const { MemoizedComposer } = await import(src + 'features/chat/composer/fox-composer.tsx')
const { ComposerEntryContext } = await import(src + 'features/chat/composer/composer-entry-context.tsx')
const { TooltipProvider } = await import(src + 'components/ui/tooltip.tsx')

const projects = [{ id: 'project-1', name: '项目甲', rootPath: 'D:/测试/项目甲', status: 'active', permissionMode: 'ask' }]
function ComposerHarness(props: Record<string, unknown>) {
  const [draft, setDraft] = React.useState('')
  return React.createElement(TooltipProvider, null,
    React.createElement(ComposerEntryContext.Provider, {
      value: { projects, onSelectProject: noop, onAddProject: noop, draftAgentBackend: 'fox', onSelectAgentBackend: noop },
    } as any),
    React.createElement(MemoizedComposer, {
      resetKey: 0, draft, setDraft, chatState: 'idle', runtimeControlled: true, ...props,
    } as any),
  )
}
capture('composer-new', React.createElement(ComposerHarness, { centered: true, newConversation: true }),
  'features/chat/composer/fox-composer.tsx', '新对话：centered=true / newConversation=true')
capture('composer-history', React.createElement(ComposerHarness, { centered: false, newConversation: false }),
  'features/chat/composer/fox-composer.tsx', '历史对话：centered=false / newConversation=false')

// ---- 3. 真实审批卡 ----
const { ApprovalCard } = await import(src + 'components/assistant-ui/upstream/elements/approval-card.tsx')
capture('approval-request', React.createElement(ApprovalCard, {
  state: 'request', title: '需要批准执行命令', subtitle: 'run_command · 项目甲',
  description: 'Fox 想要在项目目录执行一条命令。',
  command: 'ping -n 60 127.0.0.1',
  details: [
    { label: '作用目录', value: 'D:/测试/项目甲' },
    { label: '权限范围', value: '本机命令 · 只读网络' },
    { label: '拒绝后果', value: '本次操作不会执行' },
  ],
  onAllowOnce: noop, onAlwaysAllow: noop, onDeny: noop,
} as any), 'components/assistant-ui/upstream/elements/approval-card.tsx', 'state=request，真实授权选项')

// ---- 4. 真实模型服务编辑器 ----
const { ProviderEditor } = await import(src + 'features/settings/settings-pages.tsx')
const providerForm = {
  id: 'provider-demo', name: '本地模型服务', icon: null, baseUrl: 'http://127.0.0.1:11434/v1',
  apiType: 'openai-completions', isDefault: true, apiKey: '', clearApiKey: false,
  protocolProfiles: [], protocolKeys: {},
  models: [{ id: 'm1', modelId: 'demo-model', displayName: 'Demo Model', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false, isDefault: true }],
}
capture('provider-editor', React.createElement(ProviderEditor, {
  form: providerForm, busy: false, layout: 'sheet', testing: false, credentialConfigured: true,
  connectionResult: { status: 'error', text: '连接失败：无法连接到 127.0.0.1:11434' },
  existingServices: [], onChange: noop, onCancel: noop, onSave: noop, onTest: noop,
} as any), 'features/settings/settings-pages.tsx · ProviderEditor', '已存 Key、地址失败态')

// ---- 5. 真实回合过程头 ----
const { TurnProcessHeader } = await import(src + 'features/chat/turn-process-header.tsx')
capture('turn-header-cancelled', React.createElement(TurnProcessHeader, {
  events: cancelled, active: false, canToggle: true, collapsed: false,
  status: '过程已取消', onToggle: noop,
} as any), 'features/chat/turn-process-header.tsx', '真实 summarizeRunSteps 统计')

// ---- 6. 真实标题栏 / 侧栏（未在本次改进范围，原样保留） ----
const { MemoizedWindowTitlebar } = await import(src + 'features/chat/shell/fox-window-titlebar.tsx')
capture('window-titlebar', React.createElement(MemoizedWindowTitlebar, {
  leftSidebarCollapsed: false, onNewChat: noop, onOpenProject: noop, onSettings: noop,
  onAbout: noop, onToggleSidebar: noop, onZoom: noop,
} as any), 'features/chat/shell/fox-window-titlebar.tsx', '原样保留，不参与改进')

const { MemoizedSidebar } = await import(src + 'features/chat/shell/fox-sidebar.tsx')
const fn = () => () => {}
capture('sidebar', React.createElement(TooltipProvider, null, React.createElement(MemoizedSidebar, {
  collapsed: false, assistantMode: 'fox', onModeChange: noop, onNewChat: noop, onNewProjectChat: noop,
  onAddProject: noop, onCreateLocalKnowledge: noop, onOpenConversation: noop,
  onRenameConversation: noop, onPinConversation: noop, onPinProject: noop,
  onArchiveConversation: noop, onUnarchiveConversation: noop, onTrashConversation: noop,
  onRestoreConversation: noop, onPurgeConversation: noop, onDeleteProject: noop,
  onNavigate: noop, onExitManagement: noop, onExpand: noop, onTheme: noop, onSettings: noop,
  runtimeConversations: [{ id: 'conv-1', title: 'ping 取消与输出', updatedAt: baseTime + 4000 }],
  archivedConversations: [], trashedConversations: [],
  activeConversationId: 'conv-1', newChatActive: false, runIndicators: new Map(), projects: [],
  activeView: 'chat', activeEntityId: null, activeDocumentId: null,
} as any)), 'features/chat/shell/fox-sidebar.tsx', '原样保留，不参与改进')

// ---- 7. 真实发送失败文案（来自产品模块，不是手写） ----
const noticeModule = await import(src + 'features/chat/send-failure-notice.ts')
const copies = {
  unreachable: noticeModule.sendNotStartedNotice('model.connection_unreachable', 'connect ECONNREFUSED 127.0.0.1:11434', { backend: 'fox' }),
  unverified: noticeModule.sendNotStartedNotice('model.connection_unverified', 'address changed', { backend: 'fox' }),
  constants: {
    MODEL_UNREACHABLE_TITLE: noticeModule.MODEL_UNREACHABLE_TITLE,
    MODEL_UNREACHABLE_BODY: noticeModule.MODEL_UNREACHABLE_BODY,
  },
}

const output = {
  generatedAt: new Date().toISOString(),
  sourceRoot: 'apps/desktop/src',
  css: [
    'apps/desktop/dist/assets/index-DxrMkQVk.css',
    'apps/desktop/dist/assets/workbench-YYhw9Pkp.css',
    'apps/desktop/dist/assets/fox-assistant-ui-chat-5e27r6OB.css',
    'apps/desktop/dist/assets/settings-pages-C5bmnBP2.css',
  ],
  fragments, errors, copies,
}
const outPath = join(here, '真实组件片段.json')
writeFileSync(outPath, JSON.stringify(output, null, 2), 'utf8')
console.log('\nWrote', outPath)
console.log('fragments:', Object.keys(fragments).join(', '))
console.log('errors:', Object.keys(errors).join(', ') || 'none')
