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
afterEach(cleanup)
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
  assert.equal(title.textContent, '深度思考 ·')
  assert.equal(label.textContent, '首段已完成 续行 后段未完成')
  assert.equal(title.closest('.fox-run-status-viewport'), null)
  assert.equal(status.textContent, '深度思考 ·首段已完成 续行 后段未完成')
  assert.equal(status.querySelector('.fox-row-shimmer-decoration')?.hasAttribute('inert'), true)

  await React.act(async () => { view.rerender(live(`${first}继续写\r\n后段续行`)) })
  // The summary grows in place instead of jumping to the next completed line,
  // and it never scrolls sideways to chase the newest characters.
  assert.equal(label.textContent, '首段已完成 续行 后段未完成继续写 后段续行')
  assert.equal(title.textContent, '深度思考 ·')
  assert.equal(label.scrollLeft, 0)
  assert.equal(status.querySelector('.fox-row-shimmer-decoration .fox-run-status-label').scrollLeft, 0)
  assert.equal(status.classList.contains('is-clamped'), false)

  fireEvent.click(row.querySelector('.fox-runtime-step-trigger'))
  assert.equal(row.getAttribute('data-state'), 'open')
  assert.equal(title.textContent, '深度思考')
  assert.equal(label.textContent, '')
  await waitFor(() => assert.match(row.querySelector('.fox-reasoning-text')?.textContent ?? '', /后段续行/), { timeout: 5000 })
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
