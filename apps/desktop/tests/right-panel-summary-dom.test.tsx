import { ConversationSummaryFloat } from '../src/features/chat/right-panel/conversation-summary-float'
import { afterEach, expect, test } from 'bun:test'
import { cleanup, fireEvent, render, within } from '@testing-library/react'
import { ConversationSummary, SummaryContextBudget, SUMMARY_GROUPS, SUMMARY_ORDER_KEY, readSummaryOrder } from '../src/features/chat/right-panel/conversation-summary'
import type { ContextBudgetDto } from '../src/features/chat/components/context-budget-presentation'
import { RightPanel } from '../src/features/chat/right-panel/fox-right-panel'
import { ChangesPanel } from '../src/features/chat/right-panel/changes-panel'
import { ChatTopbar } from '../src/features/chat/shell/fox-chat-topbar'
import { TooltipProvider } from '../src/components/ui/tooltip'
import type { ArtifactRecord, ConversationDetail, ToolCallRecord } from '../src/features/conversations/model/types'
import { layoutStorage, readStoredRightMode, readStoredRightTabs } from '../src/features/chat/fox-shared'

afterEach(() => { cleanup(); localStorage.removeItem(SUMMARY_ORDER_KEY) })
const base = { conversation: { id: 'summary-test', projectRoot: null }, tasks: [], artifacts: [], toolCalls: [], childRuns: [], runtimeEvents: [], knowledgeBindings: [], approvals: [] } as unknown as ConversationDetail

test('空摘要各组默认展开，用户可单独折叠并在重开时恢复展开', () => {
  const view = render(<ConversationSummary detail={base} modelName="本地模型" onMode={() => {}} />)
  expect(view.getAllByRole('region').map(node => node.getAttribute('aria-label'))).toEqual(['项目', '模型'])
  expect(view.getByText('本地模型')).toBeDefined()
  expect(view.getByText(/开始对话后/)).toBeDefined()
  expect(view.getByText('暂无上下文用量。模型响应后更新。')).toBeDefined()
  fireEvent.click(view.getByRole('button', { name: '模型 本地模型' }))
  expect(view.queryByText('暂无上下文用量。模型响应后更新。')).toBeNull()
  expect(view.getByText('当前对话未绑定项目。')).toBeDefined()
  view.unmount()
  const reopened = render(<ConversationSummary detail={base} modelName="本地模型" onMode={() => {}} />)
  expect(reopened.getByText('暂无上下文用量。模型响应后更新。')).toBeDefined()
})

test('项目菜单显示可读路径，且保留网络共享路径', () => {
  const view = render(<ConversationSummary detail={{ ...base, conversation: { ...base.conversation, projectRoot: '\\\\?\\D:\\python\\projects' } }} modelName="本地模型" onMode={() => {}} />)
  const project = view.getByRole('button', { name: '项目目录操作' })
  expect(project.title).toBe('D:\\python\\projects')
  fireEvent.keyDown(project, { key: 'Enter' })
  expect(view.getByRole('menuitem', { name: '复制路径' })).toBeDefined()
  expect(view.getByRole('menuitem', { name: '打开项目文件夹' })).toBeDefined()
  expect(within(view.getByRole('menu')).getByText('D:\\python\\projects')).toBeDefined()
  view.rerender(<ConversationSummary detail={{ ...base, conversation: { ...base.conversation, projectRoot: '\\\\?\\UNC\\nas\\reports' } }} modelName="本地模型" onMode={() => {}} />)
  expect(view.getByRole('button', { name: '项目目录操作' }).title).toBe('\\\\nas\\reports')
})

test('摘要显示运行命令和已结束任务计数，排除状态查询及其他工具', () => {
  const modes: string[] = []
  const detail = {
    ...base, tasks: [{ id: 'task', title: '核对资料', status: 'completed' }],
    artifacts: [{ id: 'file', displayName: '结果.md', artifactType: 'created_file' }],
    toolCalls: [
      ...Array.from({ length: 4 }, (_, i) => ({ id: `active-${i}`, input: { command: `node run-${i}.mjs` }, toolName: 'run_command', status: 'running', startedAt: Date.now() })),
      { id: 'done', toolName: 'run_command', input: { command: 'node done.mjs' }, status: 'failed' },
      { id: 'lookup', toolName: 'run_command', input: { action: 'status' }, status: 'completed' },
      { id: 'start', toolName: 'run_command', input: { action: 'start', command: 'node background.mjs' }, status: 'completed' },
      { id: 'unrelated', toolName: 'read_file', input: { path: 'input.md' }, status: 'completed' },
    ],
    childRuns: [{ id: 'child', workerAgentName: '资料助手', objective: '检查来源', status: 'completed' }],
    knowledgeBindings: [{ knowledgeBaseId: 'kb', knowledgeBaseName: '已绑定资料', enabled: true }],
    runtimeEvents: [{ eventType: 'source.added', seq: 1, event: { id: 'source', title: '引用原文', excerpt: '原文摘录' } }],
  } as unknown as ConversationDetail
  const view = render(<ConversationSummary detail={detail} modelName="测试模型" onMode={mode => modes.push(mode)} />)
  expect(view.getAllByRole('region').map(node => node.getAttribute('aria-label'))).toEqual(['项目', '模型', '任务进度', '生成文件', '后台任务', '协作助手', '资料来源'])
  expect([...view.container.querySelectorAll('.fox-summary-toggle')].every(toggle => toggle.getAttribute('aria-expanded') === 'true')).toBe(true)
  for (const button of view.getAllByRole('button', { name: /^查看全部/ })) fireEvent.click(button)
  const background = within(view.getByRole('region', { name: '后台任务' }))
  expect(background.getAllByText(/^node run-/)).toHaveLength(4)
  expect(background.queryByText('node done.mjs')).toBeNull()
  fireEvent.click(background.getByRole('button', { name: '1 个已结束' }))
  expect(modes).toEqual(['agents', 'knowledge', 'background'])
  expect(view.getByText('引用原文')).toBeDefined()
  expect(view.getByText('核对资料')).toBeDefined()
  const progress = within(view.getByRole('region', { name: '任务进度' }))
  expect(progress.getByRole('listitem').classList.contains('is-completed')).toBe(true)
  fireEvent.click(progress.getByRole('button', { name: /^任务进度/ }))
  expect(progress.queryByText('核对资料')).toBeNull()
})

test('生成文件按生成时间倒序，展开五条后可继续展开全部或直接收起', () => {
  const artifacts = [1, 12, 2, 10, 3, 11, 4, 9, 5, 8, 6, 7].map(createdAt => ({
    id: String(createdAt), displayName: `文件 ${createdAt}.md`, createdAt, updatedAt: createdAt === 1 ? 99 : createdAt,
  } as ArtifactRecord))
  const view = render(<ConversationSummary detail={{ ...base, artifacts }} modelName="测试模型" onMode={() => {}} />)
  const section = within(view.getByRole('region', { name: '生成文件' }))
  const visibleFiles = () => section.getAllByText(/^文件 \d+/).map(node => node.textContent)
  expect(visibleFiles()).toEqual(['文件 12.md', '文件 11.md', '文件 10.md', '文件 9.md', '文件 8.md'])
  fireEvent.click(section.getByRole('button', { name: '查看更多' }))
  expect(visibleFiles()).toHaveLength(10)
  fireEvent.click(section.getByRole('button', { name: '收起' }))
  expect(visibleFiles()).toHaveLength(5)
  fireEvent.click(section.getByRole('button', { name: '查看更多' }))
  fireEvent.click(section.getByRole('button', { name: '查看更多' }))
  expect(visibleFiles()).toHaveLength(12)
  expect(section.queryByRole('button', { name: '查看更多' })).toBeNull()
  fireEvent.click(section.getByRole('button', { name: '收起' }))
  expect(visibleFiles()).toHaveLength(5)
  view.rerender(<ConversationSummary detail={{ ...base, artifacts: artifacts.slice(0, 5) }} modelName="测试模型" onMode={() => {}} />)
  expect(section.queryByRole('button', { name: '查看更多' })).toBeNull()
})

test('后台任务标签分开显示运行和结束的命令，展开后显示对应输出', () => {
  const running = { id: 'live', runId: 'run-1', runtimeToolCallId: 'call-1', toolName: 'run_command', input: { command: 'node live.mjs' }, status: 'running', startedAt: Date.now(), completedAt: null } as ToolCallRecord
  const failed = { ...running, id: 'done', runtimeToolCallId: 'call-2', input: { command: 'node done.mjs' }, status: 'failed', completedAt: Date.now(), result: { details: { stderr: '写入失败', exitCode: 1 } } }
  const detail = { ...base, toolCalls: [running, failed], runtimeEvents: [
    { runId: 'run-1', eventType: 'tool.updated', seq: 2, createdAt: Date.now(), event: { toolCallId: 'call-1', update: { streamedOutput: true, output: '已读到第一行' } } },
  ] } as ConversationDetail
  const opened: string[] = []
  const summary = render(<ConversationSummaryFloat detail={detail} modelName="测试模型" onMode={mode => opened.push(mode)} onClose={() => opened.push('close')} />)
  fireEvent.click(summary.getByRole('button', { name: '1 个已结束' }))
  expect(opened).toEqual(['close', 'background'])
  summary.unmount()
  const panel = (value: ConversationDetail) => <TooltipProvider><RightPanel mode="background" tabs={['background']} detail={value} activity={{ goal: null, focusedEvidence: null }} width={320} fileTabs={[]} activeFileTabId={null} activeChildAgent={null} onMode={() => {}} onCloseMode={() => {}} onOpenFile={() => {}} onActivateFile={() => {}} onCloseFile={() => {}} onOpenChildAgent={() => {}} onBackChildAgent={() => {}} onToggleMaximized={() => {}} onCollapse={() => {}} /></TooltipProvider>
  const view = render(panel(detail))
  expect(view.getByRole('tab', { name: '后台任务' }).getAttribute('aria-selected')).toBe('true')
  expect(view.getByText('正在运行 1')).toBeDefined()
  expect(view.getByText('已结束 1')).toBeDefined()
  expect(view.queryByText('已读到第一行')).toBeNull()
  fireEvent.click(view.getByRole('button', { name: /node live/ }))
  expect(view.getByText('$ node live.mjs')).toBeDefined()
  expect(view.getByText('已读到第一行')).toBeDefined()
  fireEvent.click(view.getByRole('button', { name: /node done/ }))
  expect(view.getByText('退出码 1')).toBeDefined()
  expect(view.getByText(/写入失败/)).toBeDefined()
  view.rerender(panel({ ...detail, toolCalls: [
    { ...running, status: 'completed', completedAt: Date.now(), result: { details: { stdout: '命令执行完毕', exitCode: 0 } } },
    failed,
  ] }))
  expect(view.getByText('已结束 2')).toBeDefined()
  expect(view.getByText('命令执行完毕')).toBeDefined()
})

test('侧栏标签不包含摘要；页面仍可切换', () => {
  const selected: string[] = []
  const view = render(<TooltipProvider><RightPanel mode="knowledge" tabs={['files', 'knowledge']} detail={null} activity={{ goal: null, focusedEvidence: null }} width={320} fileTabs={[{ id: 'a', path: 'a.md', name: 'a.md', artifactId: null }]} activeFileTabId={null} activeChildAgent={null} onMode={mode => selected.push(mode)} onCloseMode={() => {}} onOpenFile={() => {}} onActivateFile={id => selected.push(id)} onCloseFile={() => {}} onOpenChildAgent={() => {}} onBackChildAgent={() => {}} onToggleMaximized={() => {}} onCollapse={() => {}} /></TooltipProvider>)
  const tabs = view.getAllByRole('tab')
  expect(tabs.map(tab => tab.getAttribute('aria-label'))).toEqual(['文件', '资料库', 'a.md'])
  expect(tabs.map(tab => tab.textContent)).toEqual(['', '资料库', ''])
  expect(view.queryByRole('button', { name: '关闭摘要页面' })).toBeNull()
  expect(view.queryByRole('button', { name: '关闭文件页面' })).toBeNull()
  expect(view.getByRole('button', { name: '关闭资料库页面' })).toBeDefined()
  fireEvent.mouseDown(tabs[0], { button: 0, ctrlKey: false })
  expect(selected).toEqual(['files'])
})

test('更改只展开当前行的详情，长命令保留原文', () => {
  const command = 'Write-Output ' + 'long value '.repeat(45).trimEnd()
  const detail = { ...base, toolCalls: [
    { id: 'command', toolName: 'run_command', input: { command, cwd: 'D:/project' }, status: 'completed', startedAt: 3, completedAt: 12, result: { stdout: '完整输出', exitCode: 0 } },
    { id: 'edit', toolName: 'edit_file', input: { path: 'report.md', oldText: '原内容', newText: '新内容' }, status: 'completed', startedAt: 2, completedAt: 5 },
  ] } as ConversationDetail
  const view = render(<ChangesPanel detail={detail} focusedEvidence={null} />)
  const commandRow = view.getByRole('button', { name: /^Write-Output/ })
  const fileRow = view.getByRole('button', { name: /^report\.md/ })
  expect(commandRow.getAttribute('aria-expanded')).toBe('false')
  expect(view.queryByText(/完整输出/)).toBeNull()
  fireEvent.click(fileRow)
  expect(view.getByText('+ 新内容')).toBeDefined()
  expect(commandRow.getAttribute('aria-expanded')).toBe('false')
  fireEvent.click(commandRow)
  const expanded = document.getElementById(commandRow.getAttribute('aria-controls')!)!
  expect(within(expanded).getByText(command)).toBeDefined()
  expect(within(expanded).getByText(/完整输出/)).toBeDefined()
  expect(fileRow.getAttribute('aria-expanded')).toBe('true')
  fireEvent.click(commandRow)
  expect(view.queryByText(/完整输出/)).toBeNull()
})

test('顶栏摘要和侧栏展开按钮分别触发各自入口', () => {
  const opened: string[] = []
  const view = render(<TooltipProvider><ChatTopbar rightSidebarCollapsed={true} onOpenSummary={() => opened.push('summary')} onRightSidebarExpand={() => opened.push('guide')} onRightSidebarCollapse={() => {}} onOpenRightMode={() => {}} modelName="测试模型" detail={null} state="complete" onPinConversation={() => {}} onRenameConversation={() => {}} onResetContext={() => {}} onArchiveConversation={() => {}} /></TooltipProvider>)
  fireEvent.click(view.getByRole('button', { name: '对话摘要' }))
  fireEvent.click(view.getByRole('button', { name: '展开右侧边栏' }))
  expect(opened).toEqual(['summary', 'guide'])
  expect(view.getByRole('button', { name: '展开右侧边栏' })).toBeDefined()
  expect(view.container.querySelector('.fox-conversation-summary')).toBeNull()
  expect(view.getAllByRole('button').length).toBeGreaterThan(0)
})

test('摘要打开时，标题栏仍能收起已打开的侧栏', () => {
  let collapsed = false
  const view = render(<TooltipProvider><ChatTopbar summaryOpen rightSidebarCollapsed={false} onOpenSummary={() => {}} onRightSidebarExpand={() => {}} onRightSidebarCollapse={() => { collapsed = true }} onOpenRightMode={() => {}} modelName="测试模型" detail={null} state="complete" onPinConversation={() => {}} onRenameConversation={() => {}} onResetContext={() => {}} onArchiveConversation={() => {}} /></TooltipProvider>)
  expect(view.getByRole('button', { name: '对话摘要' }).getAttribute('aria-pressed')).toBe('true')
  fireEvent.click(view.getByRole('button', { name: '收起右侧边栏' }))
  expect(collapsed).toBe(true)
})

test('键盘排序持久化，取消还原，恢复默认顺序', () => {
  const view = render(<ConversationSummary detail={base} modelName="模型" onMode={() => {}} />)
  const grip = view.getByRole('button', { name: '拖动调整「模型」的顺序' })
  expect(view.queryByText('恢复默认顺序')).toBeNull()
  fireEvent.keyDown(grip, { key: ' ' })
  fireEvent.keyDown(grip, { key: 'ArrowUp' })
  expect(view.getByText('已移到第 1 位')).toBeDefined()
  fireEvent.keyDown(grip, { key: 'Escape' })
  expect(view.getAllByRole('region')[0].getAttribute('aria-label')).toBe('项目')
  expect(localStorage.getItem(SUMMARY_ORDER_KEY)).toBeNull()
  fireEvent.keyDown(grip, { key: ' ' })
  fireEvent.keyDown(grip, { key: 'ArrowUp' })
  fireEvent.keyDown(grip, { key: 'Enter' })
  expect(readSummaryOrder()[0]).toBe('model')
  view.unmount()
  const restored = render(<ConversationSummary detail={base} modelName="模型" onMode={() => {}} />)
  expect(restored.getAllByRole('region')[0].getAttribute('aria-label')).toBe('模型')
  fireEvent.click(restored.getByRole('button', { name: '恢复默认顺序' }))
  expect(readSummaryOrder()).toEqual([...SUMMARY_GROUPS])
  expect(restored.queryByText('恢复默认顺序')).toBeNull()
})

test('读取顺序丢弃未知和重复 id，并补齐新分组', () => {
  localStorage.setItem(SUMMARY_ORDER_KEY, JSON.stringify(['files', 'unknown', 'files', 3]))
  expect(readSummaryOrder()).toEqual(['files', 'project', 'model', 'tasks', 'processes', 'agents', 'sources'])
})

test('协作助手待批准数按子会话审批去重，不把普通运行状态当作审批', () => {
  const detail = { ...base, childRuns: [{ id: 'a', childConversationId: 'child-a', workerAgentName: '助手 A', status: 'running' }, { id: 'b', childConversationId: 'child-b', workerAgentName: '助手 B', status: 'running' }], approvals: [{ id: 'p1', conversationId: 'child-a', status: 'pending' }, { id: 'p2', conversationId: 'child-a', status: 'pending' }, { id: 'p3', conversationId: 'child-b', status: 'approved' }] } as unknown as ConversationDetail
  const view = render(<ConversationSummary detail={{ ...detail, conversation: { ...detail.conversation, id: 'approval-summary' } }} modelName="模型" onMode={() => {}} />)
  expect(view.getByText('1 个需要你批准').className).toContain('is-warning')
  expect(view.getByText('等待批准')).toBeDefined()
})

test('上下文分类合并安全余量，隐藏零追加输入，保留剩余用量', () => {
  const budget: ContextBudgetDto = { modelWindowTokens: 200000, historyTokens: 78000, toolsTokens: 14000, systemTokens: 7000, pendingAppendTokens: 0, outputReserveTokens: 8000, protocolOverheadTokens: 1300, safetyMarginTokens: 7000, availableTokens: 84700, pressureRatio: .5765, estimateSource: 'heuristic', updatedAt: 1 }
  const view = render(<SummaryContextBudget budget={budget} />)
  expect(Number(view.getByRole('meter').getAttribute('aria-valuenow'))).toBeCloseTo(57.65)
  expect(view.queryByText('待追加输入')).toBeNull()
  expect(view.getByText('安全余量').parentElement?.textContent).toContain('8.3K')
  expect(view.getByText('安全余量').parentElement?.textContent).toContain('4.2%')
  expect(view.getByText('剩余可用')).toBeDefined()
  view.rerender(<SummaryContextBudget budget={{ ...budget, pendingAppendTokens: 1000 }} />)
  expect(view.getByText('待追加输入')).toBeDefined()
})

test('未保存页面状态时从摘要开始，已有页面状态继续恢复', () => {
  const priorMode = localStorage.getItem(layoutStorage.rightMode)
  const priorTabs = localStorage.getItem(layoutStorage.rightTabs)
  try {
    localStorage.removeItem(layoutStorage.rightMode)
    localStorage.removeItem(layoutStorage.rightTabs)
    expect(readStoredRightMode()).toBeNull()
    expect(readStoredRightTabs()).toEqual([])
    localStorage.setItem(layoutStorage.rightMode, 'background')
    expect(readStoredRightMode()).toBe('background')
    expect(readStoredRightTabs()).toEqual(['background'])
    localStorage.setItem(layoutStorage.rightTabs, JSON.stringify(['files', 'background']))
    expect(readStoredRightTabs()).toEqual(['files', 'background'])
    localStorage.setItem(layoutStorage.rightMode, 'activity')
    localStorage.setItem(layoutStorage.rightTabs, JSON.stringify(['files', 'activity', 'terminal', 'evidence']))
    expect(readStoredRightMode()).toBe('terminal')
    expect(readStoredRightTabs()).toEqual(['files', 'terminal'])
  } finally {
    if (priorMode === null) localStorage.removeItem(layoutStorage.rightMode); else localStorage.setItem(layoutStorage.rightMode, priorMode)
    if (priorTabs === null) localStorage.removeItem(layoutStorage.rightTabs); else localStorage.setItem(layoutStorage.rightTabs, priorTabs)
  }
})

test('摘要浮层独立显示，可关闭且不渲染侧栏工具栏', () => {
  let closed = false
  const view = render(<ConversationSummaryFloat detail={base} modelName="测试模型" onMode={() => {}} onClose={() => { closed = true }} />)
  expect(view.queryByRole('tablist')).toBeNull()
  expect(view.container.querySelector('.fox-context-panel')).toBeNull()
  expect(view.getByRole('complementary', { name: '对话摘要' })).toBeDefined()
  expect(view.getByText('项目')).toBeDefined()
  fireEvent.click(view.getByRole('button', { name: '关闭对话摘要' }))
  expect(closed).toBe(true)
})

test('侧栏首页按参考顺序展示页面入口和最近文件空状态', () => {
  const selected: string[] = []
  const view = render(<TooltipProvider><RightPanel mode={null} tabs={['files', 'knowledge']} detail={null} activity={{ goal: null, focusedEvidence: null }} width={320} fileTabs={[{ id: 'a', path: 'a.md', name: 'a.md', artifactId: null }]} activeFileTabId={null} activeChildAgent={null} onMode={mode => selected.push(mode)} onCloseMode={() => {}} onOpenFile={() => {}} onActivateFile={() => {}} onCloseFile={() => {}} onOpenChildAgent={() => {}} onBackChildAgent={() => {}} onToggleMaximized={() => {}} onCollapse={() => {}} /></TooltipProvider>)
  expect(view.queryByRole('tablist')).toBeNull()
  const pages = within(view.getByRole('navigation', { name: '侧栏页面' })).getAllByRole('button')
  expect(pages.map(button => button.querySelector('strong')?.textContent)).toEqual(['文件', '资料库', '更改', '终端', '网页与网络', '后台任务', '协作助手'])
  fireEvent.click(pages[0])
  expect(selected).toEqual(['files'])
  expect(view.getByText('还没有最近文件。')).toBeDefined()
})
