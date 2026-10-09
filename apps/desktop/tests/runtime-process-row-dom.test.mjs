import assert from 'node:assert/strict'
import { after, afterEach, test } from 'node:test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { createServer } from 'vite'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
globalThis.IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const server = await createServer({
  root, configFile: false, appType: 'custom', cacheDir: path.join(root, 'dist', '.vite-row-test-cache'),
  optimizeDeps: { noDiscovery: true, include: [] }, server: { middlewareMode: true, hmr: false, watch: { ignored: ['**/*'] } },
  resolve: { alias: { '@': path.join(root, 'src') } }, esbuild: { jsx: 'automatic' },
})
const { RuntimeTimeline } = await server.ssrLoadModule('/src/features/chat/workbench.tsx')
const { RunStatusText } = await server.ssrLoadModule('/src/features/chat/components/FoxAssistantAvatar.tsx')
const { persistProcessDisplayMode } = await server.ssrLoadModule('/src/features/chat/process-display-mode.ts')
afterEach(() => { cleanup(); persistProcessDisplayMode('standard') })
after(async () => { await server.close() })

const user = { id: 'user', conversationId: 'conversation', runId: 'run', role: 'user', kind: 'text',
  content: '请处理', status: 'completed', ordinal: 1, createdAt: 1, updatedAt: 1 }
const assistant = { id: 'assistant-run', conversationId: 'conversation', runId: 'run',
  role: 'assistant', kind: 'text', content: '示例答复', status: 'streaming', ordinal: 2, createdAt: 2, updatedAt: 2 }
const event = (seq, eventType, data = {}) => ({ runId: 'run', seq, eventType, event: { type: eventType, ...data }, createdAt: seq })
const timeline = (reasoning, overrides = {}) => React.createElement(RuntimeTimeline, {
  messages: [user, assistant],
  attachments: [], artifacts: [], events: reasoning ? [event(1, 'reasoning.delta', { delta: reasoning })] : [], runtimeRunning: true, activeRunId: 'run',
  state: 'streaming', streamingText: '', onRetry: () => {}, onRerun: async () => false,
  ...overrides,
})

/** Open the turn disclosure (when the turn owns one) and the stage disclosure, and
 *  return the requested row. Grouping helpers arrive asynchronously and can remount
 *  the process, so a disclosure click is repeated until the row really exists. */
async function openFirstRow(view, selector = '.fox-runtime-step-row') {
  await waitFor(() => assert.ok(view.container.querySelector('.fox-chain-of-thought-header')), { timeout: 5000 })
  const expand = view.queryByRole('button', { name: /展开过程/ })
  if (expand) fireEvent.click(expand)
  await waitFor(() => {
    const header = view.container.querySelector('.fox-chain-of-thought-header')
    if (header?.getAttribute('aria-expanded') !== 'true') fireEvent.click(header)
    assert.ok(view.container.querySelector(selector))
  }, { timeout: 5000 })
  return view.container.querySelector(selector)
}

test('shared status sweep keeps one accessible text node and stops when inactive', () => {
  const view = render(React.createElement(RunStatusText, { text: '正在处理', active: true }))
  assert.equal(view.container.textContent, '正在处理')
  const decoration = view.container.querySelector('.fox-row-shimmer-decoration')
  assert.equal(decoration?.getAttribute('aria-hidden'), 'true')
  assert.equal(decoration?.hasAttribute('inert'), true)
  assert.equal(decoration?.querySelector('[data-shimmer-text]')?.getAttribute('data-shimmer-text'), '正在处理')
  view.rerender(React.createElement(RunStatusText, { text: '已完成', active: false }))
  assert.equal(view.container.textContent, '已完成')
  assert.equal(view.container.querySelector('.fox-row-shimmer-decoration'), null)
})

test('streaming thought keeps a fixed title, one ellipsized summary line, and expands to full Markdown', async () => {
  const first = '首段已完成\r\n续行\r\n\r\n后段未完成'
  // The live process group is only the one that trails the turn, so the fixture
  // streams into an assistant message whose answer has not started yet.
  const live = (reasoning) => timeline(reasoning, { messages: [user, { ...assistant, content: '' }] })
  const view = render(live(first))
  await waitFor(() => assert.ok(view.container.querySelector('.fox-chain-of-thought-header')))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  await waitFor(() => assert.ok(view.container.querySelector('.fox-runtime-step-row.is-reasoning')))
  const row = view.container.querySelector('.fox-runtime-step-row.is-reasoning')
  assert.ok(row)
  assert.equal(row.className.includes('is-active'), true)
  const status = row.querySelector('.fox-run-status-step')
  const title = status.querySelector(':scope > .fox-run-status-prefix')
  const label = status.querySelector(':scope > .fox-run-status-viewport > .fox-run-status-label')
  // A live thought is 正在思考 because it is still running; the prefix is the row's
  // own state and is never derived from the text or from the disclosure state.
  assert.equal(title.textContent, '正在思考 ·')
  assert.equal(label.textContent, '首段已完成 续行 后段未完成')
  assert.equal(title.closest('.fox-run-status-viewport'), null)
  assert.equal(status.textContent, '正在思考 ·首段已完成 续行 后段未完成')
  assert.equal(status.querySelector('.fox-row-shimmer-decoration')?.hasAttribute('inert'), true)

  await React.act(async () => { view.rerender(live(`${first}继续写\r\n后段续行`)) })
  // The summary grows in place instead of jumping to the next completed line,
  // and it never scrolls sideways to chase the newest characters.
  assert.equal(label.textContent, '首段已完成 续行 后段未完成继续写 后段续行')
  assert.equal(title.textContent, '正在思考 ·')
  assert.equal(label.scrollLeft, 0)
  assert.equal(status.querySelector('.fox-row-shimmer-decoration .fox-run-status-label').scrollLeft, 0)
  assert.equal(status.classList.contains('is-clamped'), false)

  fireEvent.click(row.querySelector('.fox-runtime-step-trigger'))
  assert.equal(row.getAttribute('data-state'), 'open')
  assert.ok(title.textContent.startsWith('正在思考'))
  assert.equal(title.textContent.includes('深度思考'), false)
  assert.equal(label.textContent, '')
  await waitFor(() => assert.match(row.querySelector('.fox-reasoning-text')?.textContent ?? '', /后段续行/), { timeout: 5000 })
})

test('expanding and collapsing a live thought never rewrites its active state', async () => {
  const live = timeline('正在核对输出\r\n第二行', { messages: [user, { ...assistant, content: '' }] })
  const view = render(live)
  const row = await openFirstRow(view, '.fox-runtime-step-row.is-reasoning')
  assert.equal(row.className.includes('is-active'), true)
  const prefix = () => row.querySelector('.fox-run-status-prefix').textContent
  assert.equal(prefix(), '正在思考 ·')
  fireEvent.click(row.querySelector('.fox-runtime-step-trigger'))
  assert.equal(row.getAttribute('data-state'), 'open')
  // Expanded: still the live state, never the settled wording.
  assert.ok(prefix().startsWith('正在思考'))
  assert.equal(prefix().includes('深度思考'), false)
  fireEvent.click(row.querySelector('.fox-runtime-step-trigger'))
  assert.equal(row.getAttribute('data-state'), 'closed')
  assert.equal(prefix(), '正在思考 ·')
})

test('a settled thought summarises its whole content instead of only the first line', async () => {
  const view = render(timeline('**结算首行**\n后续正文\n\n收尾段落', { runtimeRunning: false, activeRunId: undefined, state: 'idle' }))
  await waitFor(() => assert.ok(view.container.querySelector('.fox-chain-of-thought-header')))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  await waitFor(() => assert.ok(view.container.querySelector('.fox-runtime-step-row.is-reasoning')))
  const row = view.container.querySelector('.fox-runtime-step-row.is-reasoning')
  const label = row.querySelector('.fox-run-status-label')
  assert.equal(label.textContent, '结算首行 后续正文 收尾段落')
  assert.equal(row.querySelector('.fox-run-status-prefix').textContent, '深度思考 ·')
  // Nothing is running, so the decorative sweep is absent and the row is static.
  assert.equal(row.querySelector('.fox-row-shimmer-decoration'), null)
})

test('a settled thought keeps its one-line summary in the compact layout', async () => {
  await React.act(async () => persistProcessDisplayMode('compact'))
  const view = render(timeline('**结算首行**\n后续正文\n\n收尾段落', { runtimeRunning: false, activeRunId: undefined, state: 'idle' }))
  const row = await openFirstRow(view, '.fox-runtime-step-row.is-reasoning')
  const status = row.querySelector('.fox-run-status-step')
  // Compact mode still previews a settled thought: the row is its one summary
  // line, not an empty title that only reveals itself when expanded.
  assert.equal(status.querySelector(':scope > .fox-run-status-prefix').textContent, '深度思考 ·')
  assert.equal(status.querySelector(':scope > .fox-run-status-viewport > .fox-run-status-label').textContent, '结算首行 后续正文 收尾段落')
  assert.equal(row.querySelector('.fox-row-shimmer-decoration'), null)
})

test('tool path remains one interactive node and opens the sidebar without toggling details', async () => {
  const opened = []
  const events = [event(1, 'tool.started', { toolCallId: 'read', tool: 'read', input: { path: 'README.md' } })]
  const view = render(timeline('', { events, onOpenFileInSidebar: path => opened.push(path) }))
  await waitFor(() => assert.ok(view.container.querySelector('.fox-chain-of-thought-header')))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  await waitFor(() => assert.ok(view.container.querySelector('.fox-runtime-step-row')))
  const row = view.container.querySelector('.fox-runtime-step-row')
  const path = row.querySelector('.fox-runtime-step-path')
  assert.ok(path)
  assert.equal(row.querySelectorAll('.fox-runtime-step-path').length, 1)
  assert.equal(row.querySelector('.fox-row-shimmer-decoration'), null)
  fireEvent.click(path)
  assert.deepEqual(opened, ['README.md'])
  assert.equal(row.getAttribute('data-state'), 'closed')
  fireEvent.click(row.querySelector('.fox-runtime-step-trigger'))
  assert.equal(row.getAttribute('data-state'), 'open')
})

test('a row with both a path and a worksheet keeps the path clickable and in its own slot', async () => {
  const opened = []
  const events = [
    event(1, 'tool.started', { toolCallId: 'read', tool: 'read', input: { path: 'src/明细/a.ts' } }),
    event(2, 'tool.completed', { toolCallId: 'read', tool: 'read', result: { sheets: [{ name: '明细' }] } }),
  ]
  const view = render(timeline('', { events, onOpenFileInSidebar: path => opened.push(path) }))
  const row = await openFirstRow(view)
  const label = row.querySelector('.fox-run-status-label')
  const action = label.querySelector('.fox-run-step-action')
  const pathNode = label.querySelector('.fox-runtime-step-path')
  const tail = label.querySelector('.fox-run-step-tail')
  // The source is a real source, not a string appended after the path: the path is
  // still its own element and still the only interactive node in the row.
  assert.equal(action.textContent, '读取文件')
  assert.equal(pathNode.textContent, 'src/明细/a.ts')
  assert.equal(pathNode.getAttribute('role'), 'button')
  assert.equal(pathNode.getAttribute('tabindex'), '0')
  assert.equal(label.textContent, '读取文件 · src/明细/a.ts · 明细')
  assert.equal(tail.textContent, '明细')
  assert.equal(row.querySelectorAll('.fox-runtime-step-path').length, 1)
  // Neither the action nor the path is inside the clipped/faded region.
  assert.equal(action.closest('.fox-run-step-tail'), null)
  assert.equal(pathNode.closest('.fox-run-step-tail'), null)
  fireEvent.click(pathNode)
  assert.deepEqual(opened, ['src/明细/a.ts'])
  assert.equal(row.getAttribute('data-state'), 'closed')
  fireEvent.keyDown(pathNode, { key: 'Enter' })
  assert.deepEqual(opened, ['src/明细/a.ts', 'src/明细/a.ts'])
  assert.equal(row.getAttribute('data-state'), 'closed')
  fireEvent.keyDown(pathNode, { key: ' ' })
  assert.deepEqual(opened, ['src/明细/a.ts', 'src/明细/a.ts', 'src/明细/a.ts'])
  assert.equal(row.getAttribute('data-state'), 'closed')
})

test('an attachment id renders only through the conversation name table', async () => {
  const mappedId = 'e2c8b6a4-1f00-4c2a-9b3d-0f9e8d7c6b5a'
  const unmappedId = 'compute-artifact:4969408f41561445'
  // messageId stays null so the name table is the only thing the record feeds.
  const attachments = [{ id: mappedId, conversationId: 'conversation', messageId: null, displayName: '6-1.xlsx' }]
  const computeEvents = (attachmentIds) => [
    event(1, 'tool.started', { toolCallId: 'compute', tool: 'attachment_compute', input: { attachmentIds } }),
    event(2, 'tool.completed', { toolCallId: 'compute', tool: 'attachment_compute', result: { status: 'completed' } }),
  ]
  const view = render(timeline('', { events: computeEvents([mappedId]), attachments }))
  const mappedRow = await openFirstRow(view)
  assert.equal(mappedRow.querySelector('.fox-run-status-label').textContent, '计算附件数据 · 6-1.xlsx')
  cleanup()
  const unmapped = render(timeline('', { events: computeEvents([unmappedId]), attachments }))
  const unmappedRow = await openFirstRow(unmapped)
  const unmappedLabel = unmappedRow.querySelector('.fox-run-status-label')
  // Nothing: no id, no truncated id, no invented file name.
  assert.equal(unmappedLabel.textContent, '计算附件数据')
  assert.doesNotMatch(unmappedLabel.textContent, /4969408f|compute-artifact|…/)
  assert.doesNotMatch(unmapped.container.textContent, /4969408f|compute-artifact/)
})

test('a failed row keeps its status and its real source without the raw diagnostic', async () => {
  const mappedId = 'e2c8b6a4-1f00-4c2a-9b3d-0f9e8d7c6b5a'
  const rawCode = 'compute_budget_exceeded'
  const rawMessage = 'budget 120000ms exceeded while reading scope=project:6-1.xlsx'
  const attachments = [{ id: mappedId, conversationId: 'conversation', messageId: null, displayName: '6-1.xlsx' }]
  const events = [
    event(1, 'tool.started', { toolCallId: 'compute', tool: 'attachment_compute', input: { attachmentIds: [mappedId] } }),
    event(2, 'tool.completed', {
      toolCallId: 'compute', tool: 'attachment_compute', isError: true,
      result: { error: { code: rawCode, message: rawMessage } },
    }),
  ]
  const view = render(timeline('', { events, attachments }))
  const row = await openFirstRow(view)
  const label = row.querySelector('.fox-run-status-label')
  assert.match(label.textContent, /工具调用未成功/)
  // The legitimate source survives the failure.
  assert.match(label.textContent, /6-1\.xlsx/)
  // The Host's raw diagnostic body does not.
  assert.doesNotMatch(view.container.textContent, new RegExp(rawCode))
  assert.doesNotMatch(view.container.textContent, /120000ms|scope=/)
})

test('a narrow row keeps the action name out of the clipped region', async () => {
  const longPath = 'src/功能模块/非常长的英文目录名称/组件/渲染器/RuntimeProcessRowImplementation.tsx'
  const longSource = '销售明细汇总工作表 Mixed Sheet Name With A Long Tail'
  const events = [
    event(1, 'tool.started', { toolCallId: 'read', tool: 'read', input: { path: longPath } }),
    event(2, 'tool.completed', { toolCallId: 'read', tool: 'read', result: { sheets: [{ name: longSource }] } }),
  ]
  // A layout-sized wrapper: happy-dom has no layout, so what this test can prove is
  // the structure that decides what may be clipped — the action name is its own
  // element outside the source/summary region that carries the ellipsis and fade.
  const view = render(React.createElement('div', { className: 'fox-sidebar-narrow', style: { width: '240px' } },
    timeline('', { events, onOpenFileInSidebar: () => {} })))
  const row = await openFirstRow(view)
  const label = row.querySelector('.fox-run-status-label')
  const action = label.querySelector('.fox-run-step-action')
  const tail = label.querySelector('.fox-run-step-tail')
  const pathSlot = label.querySelector('.fox-run-step-path-slot')
  assert.equal(action.textContent, '读取文件')
  assert.equal(action.closest('.fox-run-step-tail'), null)
  assert.equal(pathSlot.closest('.fox-run-step-tail'), null)
  assert.equal(tail.textContent, longSource)
  assert.equal(tail.textContent.includes('读取文件'), false)
  // The action really is the first thing on the line, before the path and the tail.
  assert.deepEqual([...label.children].map(node => node.className), [
    'fox-run-step-action', 'fox-run-step-separator', 'fox-run-step-path-slot',
    'fox-run-step-separator', 'fox-run-step-tail',
  ])
})

test('the whole-run elapsed time stays a single header timer beside the Fox avatar', async () => {
  const view = render(timeline('', {
    events: [
      event(1, 'run.started'),
      event(2, 'tool.started', { toolCallId: 'read', tool: 'read', input: { path: 'README.md' } }),
      event(3, 'run.completed'),
    ],
    runtimeRunning: false, activeRunId: undefined, state: 'idle',
  }))
  await waitFor(() => assert.ok(view.container.querySelector('.fox-process-turn-header')))
  const header = view.container.querySelector('.fox-process-turn-header')
  const timers = view.container.querySelectorAll('[role="timer"]')
  assert.equal(timers.length, 1)
  // The timer lives in the turn header, after the avatar — never inside a step row.
  assert.ok(header.contains(timers[0]))
  assert.equal(timers[0].closest('.fox-runtime-step-row'), null)
  const avatar = header.querySelector('.fox-assistant-avatar')
  assert.ok(avatar)
  const following = globalThis.Node?.DOCUMENT_POSITION_FOLLOWING ?? 4
  assert.ok(avatar.compareDocumentPosition(timers[0]) & following)
  assert.equal(timers[0].parentElement.closest('.fox-runtime-step-row'), null)
})
