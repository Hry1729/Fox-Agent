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
  root, configFile: false, appType: 'custom', cacheDir: path.join(root, 'dist', '.vite-test-cache'),
  optimizeDeps: { noDiscovery: true, include: [] }, server: { middlewareMode: true, hmr: false, watch: { ignored: ['**/*'] } },
  resolve: { alias: { '@': path.join(root, 'src') } }, esbuild: { jsx: 'automatic' },
})
const { RuntimeTimeline } = await server.ssrLoadModule('/src/features/chat/workbench.tsx')
const { persistProcessDisplayMode } = await server.ssrLoadModule('/src/features/chat/process-display-mode.ts')
const { answerDeltaFingerprint } = await server.ssrLoadModule('/src/features/conversations/model/runtime-delta-fingerprint.ts')
const { groupAssistantContinuations } = await server.ssrLoadModule('/src/features/chat/runtime-timeline-performance.ts')
const { formatRunElapsed, runElapsedBounds } = await server.ssrLoadModule('/src/features/chat/turn-process-timing.ts')
afterEach(() => { cleanup(); persistProcessDisplayMode('standard') })
after(async () => { await server.close() })

const baseTime = Date.now() - 10_000
const event = (seq, eventType, data = {}, createdAt = baseTime + seq * 1000) => ({
  runId: 'run', seq, eventType, event: { type: eventType, ...data }, createdAt,
})
const answer = (seq, text) => event(seq, 'message.delta', { deltaLength: text.length, deltaFingerprint: answerDeltaFingerprint(text) })
const events = [
  event(1, 'run.started'),
  event(2, 'reasoning.delta', { delta: '先分析' }),
  answer(3, '第一段'),
  event(4, 'tool.started', { toolCallId: 'read', tool: 'read', input: { path: 'README.md' } }),
  event(5, 'tool.completed', { toolCallId: 'read', tool: 'read', result: { content: 'ok' } }),
  answer(6, '第二段'),
  event(7, 'run.completed'),
]
const user = { id: 'user', conversationId: 'conversation', runId: 'run', role: 'user', kind: 'text', content: '请处理', status: 'completed', ordinal: 1, createdAt: baseTime, updatedAt: baseTime }
const assistant = { id: 'assistant-run', conversationId: 'conversation', runId: 'run', role: 'assistant', kind: 'text', content: '第一段第二段', status: 'completed', ordinal: 2, createdAt: baseTime + 1000, updatedAt: baseTime + 7000 }
const timeline = (runEvents = events, overrides = {}) => React.createElement(RuntimeTimeline, {
  messages: [user, assistant], attachments: [], artifacts: [], events: runEvents, runtimeRunning: false,
  state: 'idle', streamingText: '', onRetry: () => {}, onRerun: async () => false, ...overrides,
})
const live = (runEvents = events.slice(0, -1), overrides = {}) => timeline(runEvents, {
  runtimeRunning: true, activeRunId: 'run', state: 'streaming',
  messages: [user, { ...assistant, status: 'streaming' }], ...overrides,
})
const stages = (view) => [...view.container.querySelectorAll('.fox-process-stage')]

test('duration comes only from persisted run lifecycle events', () => {
  assert.deepEqual(runElapsedBounds(events), { startedAt: baseTime + 1000, endedAt: baseTime + 7000 })
  assert.deepEqual(runElapsedBounds(events.slice(1)), { startedAt: null, endedAt: null })
  assert.deepEqual(runElapsedBounds(events.slice(0, -1)), { startedAt: baseTime + 1000, endedAt: null })
  assert.equal(formatRunElapsed(6_500), '6秒')
  assert.equal(formatRunElapsed(3_661_000), '1时1分1秒')
})

test('the assistant bucket key stays stable when steering inserts another user and assistant bucket', () => {
  const followUp = { ...user, id: 'user-follow-up', content: '补充要求', ordinal: 3, createdAt: baseTime + 8000 }
  const continuation = { ...assistant, id: 'assistant-continuation', content: '已补充', ordinal: 4 }
  const before = groupAssistantContinuations([user, assistant])
  const after = groupAssistantContinuations([user, assistant, followUp, continuation])
  assert.equal(after[1].timelineKey, before[1].timelineKey)
  assert.notEqual(after[1].timelineKey, after[3].timelineKey)
  assert.equal(after[2].content, '补充要求')
})

test('one turn header holds the avatar, fold control, and durable duration; final reply and footer stay out of the fold', async () => {
  const view = render(timeline())
  await waitFor(() => assert.equal(stages(view).length, 2))
  const header = view.container.querySelector('.fox-process-turn-header')
  assert.equal(header.querySelectorAll('.fox-assistant-avatar').length, 1)
  assert.equal(view.container.querySelectorAll('.fox-assistant-content .fox-assistant-avatar').length, 1)
  assert.equal(header.querySelector('[role="timer"]').textContent, '用时 6秒')
  assert.equal(stages(view).filter(stage => stage.hidden).length, 2)
  assert.equal(view.container.querySelectorAll('.fox-answer-segment:not([hidden])').length, 1)
  assert.match(view.container.querySelector('.fox-answer-segment:not([hidden])').textContent, /第二段/)
  assert.equal(view.container.querySelectorAll('.fox-message-actions button').length, 4)
  fireEvent.click(view.getByRole('button', { name: /展开过程/ }))
  assert.equal(stages(view).filter(stage => stage.hidden).length, 0)
  assert.equal(view.container.querySelectorAll('.fox-assistant-content .fox-assistant-avatar').length, 1)
  assert.equal(view.container.querySelectorAll('.fox-answer-segment:not([hidden])').length, 2)
})

test('missing start or terminal lifecycle timestamp leaves only the status in history', async () => {
  const missingStart = render(timeline(events.slice(1)))
  await waitFor(() => assert.equal(stages(missingStart).length, 2))
  assert.equal(missingStart.container.querySelector('[role="timer"]'), null)
  cleanup()
  const missingEnd = render(timeline(events.slice(0, -1)))
  await waitFor(() => assert.equal(stages(missingEnd).length, 2))
  assert.equal(missingEnd.container.querySelector('[role="timer"]'), null)
})

test('running turn can be folded and reopened; explicit open choice survives completion', async () => {
  const view = render(live())
  await waitFor(() => assert.equal(stages(view).length, 2))
  assert.equal(view.getByRole('button', { name: /收起过程/ }).getAttribute('aria-expanded'), 'true')
  const timer = view.container.querySelector('[role="timer"]')
  assert.match(timer.textContent, /^已用时 /)
  const answerNode = view.container.querySelector('.fox-answer-segment.is-final')
  fireEvent.click(view.getByRole('button', { name: /收起过程/ }))
  assert.equal(stages(view).filter(stage => stage.hidden).length, 2)
  assert.equal(answerNode.hidden, false)
  fireEvent.click(view.getByRole('button', { name: /展开过程/ }))
  view.rerender(timeline())
  assert.equal(stages(view).filter(stage => stage.hidden).length, 0)
  assert.equal(view.getByRole('button', { name: /收起过程/ }).getAttribute('aria-expanded'), 'true')
  assert.equal(view.container.querySelector('[role="timer"]').textContent, '用时 6秒')
})

test('explicit running collapse survives completion and interleaved user input remains visible', async () => {
  const view = render(live())
  await waitFor(() => assert.equal(stages(view).length, 2))
  fireEvent.click(view.getByRole('button', { name: /收起过程/ }))
  const followUp = { ...user, id: 'user-follow-up', content: '补充要求', ordinal: 3, createdAt: baseTime + 8000 }
  view.rerender(timeline(events, { messages: [user, assistant, followUp] }))
  assert.equal(stages(view).filter(stage => stage.hidden).length, 2)
  assert.match(view.container.textContent, /补充要求/)
  assert.match(view.container.querySelector('.fox-answer-segment:not([hidden])').textContent, /第二段/)
})

test('failed and cancelled turns keep a clear status and an accessible process toggle', async () => {
  const view = render(timeline([...events.slice(0, -1), event(7, 'run.failed', { code: 'provider_error', message: '失败' })]))
  await waitFor(() => assert.equal(stages(view).length, 2))
  assert.ok(view.getByRole('button', { name: /收起过程.*过程未完成/ }))
  fireEvent.click(view.getByRole('button', { name: /收起过程.*过程未完成/ }))
  assert.ok(view.getByRole('button', { name: /展开过程.*过程未完成/ }))
  cleanup()
  const cancelled = render(timeline([...events.slice(0, -1), event(7, 'run.cancelled')]))
  await waitFor(() => assert.equal(stages(cancelled).length, 2))
  assert.ok(cancelled.getByRole('button', { name: /收起过程.*过程已取消/ }))
})

test('a recoverable tool error stays in the process while a completed run has no failure count', async () => {
  const runEvents = [
    event(1, 'run.started'),
    event(2, 'tool.started', { toolCallId: 'first', tool: 'read_file', input: { path: 'missing.txt' } }),
    event(3, 'tool.completed', { toolCallId: 'first', tool: 'read_file', isError: true, result: { code: 'not_found', message: 'missing.txt' } }),
    event(4, 'tool.started', { toolCallId: 'retry', tool: 'read_file', input: { path: 'README.md' } }),
    event(5, 'tool.completed', { toolCallId: 'retry', tool: 'read_file', result: { content: 'ok' } }),
    answer(6, '完成'),
    event(7, 'run.completed'),
  ]
  const view = render(timeline(runEvents, { messages: [user, { ...assistant, content: '完成' }] }))
  await waitFor(() => assert.ok(view.container.querySelector('.fox-process-turn-header')))
  assert.doesNotMatch(view.container.querySelector('.fox-process-turn-header').textContent, /项失败/)
  fireEvent.click(view.getByRole('button', { name: /展开过程/ }))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  await waitFor(() => assert.match(view.container.textContent, /工具调用未成功/))
  fireEvent.click(view.container.querySelector('.fox-runtime-step-trigger'))
  assert.match(view.container.textContent, /not_found|missing.txt/)
  assert.doesNotMatch(view.container.querySelector('.fox-process-turn-header').textContent, /过程未完成/)
})

test('reader focus protects the process at automatic completion; verbose mode remains expanded', async () => {
  const view = render(live())
  await waitFor(() => assert.equal(stages(view).length, 2))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  const focusedRow = view.container.querySelector('.fox-runtime-step-trigger')
  await React.act(async () => focusedRow.focus())
  view.rerender(timeline())
  assert.equal(stages(view).filter(stage => stage.hidden).length, 0)
  await React.act(async () => persistProcessDisplayMode('verbose'))
  assert.equal(stages(view).filter(stage => stage.hidden).length, 0)
  assert.equal(view.container.querySelectorAll('.fox-runtime-process-disclosure[aria-hidden="false"]').length, 2)
})

