import assert from 'node:assert/strict'
import { after, afterEach, test } from 'node:test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { createServer } from 'vite'

GlobalRegistrator.register()
const React = await import('react')
const { cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const server = await createServer({
  root, configFile: false, appType: 'custom', cacheDir: path.join(root, 'dist', '.vite-test-cache'),
  optimizeDeps: { noDiscovery: true, include: [] }, server: { middlewareMode: true },
  resolve: { alias: { '@': path.join(root, 'src') } }, esbuild: { jsx: 'automatic' },
})
const { RuntimeTimeline } = await server.ssrLoadModule('/src/features/chat/workbench.tsx')
const { answerDeltaFingerprint } = await server.ssrLoadModule('/src/features/conversations/model/runtime-delta-fingerprint.ts')
afterEach(() => cleanup())
after(async () => { await server.close(); GlobalRegistrator.unregister() })

const messages = [
  { id: 'user', conversationId: 'conversation', runId: 'run', role: 'user', kind: 'text', content: '请处理', status: 'completed', ordinal: 1, createdAt: 1, updatedAt: 1 },
  { id: 'assistant-run', conversationId: 'conversation', runId: 'run', role: 'assistant', kind: 'text', content: '第一段第二段', status: 'completed', ordinal: 2, createdAt: 2, updatedAt: 2 },
]
const event = (seq, eventType, data = {}) => ({ runId: 'run', seq, eventType, event: { type: eventType, ...data }, createdAt: seq })
const answer = (seq, text) => event(seq, 'message.delta', { deltaLength: text.length, deltaFingerprint: answerDeltaFingerprint(text) })
const events = [
  event(1, 'reasoning.delta', { delta: '先分析' }),
  answer(2, '第一段'),
  event(3, 'tool.started', { toolCallId: 'read', tool: 'read', input: { path: 'README.md' } }),
  event(4, 'tool.completed', { toolCallId: 'read', tool: 'read', result: { content: 'ok' } }),
  answer(5, '第二段'),
]
const timeline = runtimeEvents => React.createElement(RuntimeTimeline, {
  messages, attachments: [], artifacts: [], events: runtimeEvents, runtimeRunning: false,
  state: 'idle', streamingText: '', onRetry: () => {}, onRerun: async () => false,
})

test('answer segments remain once and in order while the whole process is collapsed', async () => {
  const view = render(timeline(events))
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-answer-segment').length, 2), { timeout: 5000 })
  let segments = view.container.querySelectorAll('.fox-answer-segment')
  assert.equal(segments.length, 2)
  assert.match(segments[0].textContent, /第一段/)
  assert.match(segments[1].textContent, /第二段/)
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  assert.equal(view.container.querySelectorAll('.fox-chain-of-thought-header[aria-expanded="true"]').length, 1)
  fireEvent.click(view.getByRole('button', { name: /收起过程/ }))
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 2)
  segments = view.container.querySelectorAll('.fox-answer-segment')
  assert.equal(segments.length, 2)
  fireEvent.click(view.getByRole('button', { name: /展开过程/ }))
  assert.equal(view.container.querySelectorAll('.fox-runtime-process').length, 2)
  assert.equal(view.container.querySelectorAll('.fox-chain-of-thought-header[aria-expanded="true"]').length, 1)
})

test('old history with missing answer deltas keeps one answer body', () => {
  const view = render(timeline(events.filter(item => item.eventType !== 'message.delta')))
  assert.equal(view.container.querySelectorAll('.fox-answer-segment').length, 0)
  assert.equal(view.container.querySelectorAll('.fox-answer-body').length, 1)
  assert.match(view.container.querySelector('.fox-answer-body').textContent, /第一段第二段/)
})

test('streaming a later stage keeps an earlier disclosure open', async () => {
  const view = render(timeline(events))
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-answer-segment').length, 2))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  const nextEvents = [...events, event(6, 'reasoning.delta', { delta: '再核对' }), answer(7, '第三段')]
  view.rerender(React.createElement(RuntimeTimeline, {
    ...timeline(nextEvents).props,
    messages: [messages[0], { ...messages[1], content: '第一段第二段第三段', status: 'streaming' }],
  }))
  assert.equal(view.container.querySelectorAll('.fox-answer-segment').length, 3)
  assert.equal(view.container.querySelectorAll('.fox-chain-of-thought-header[aria-expanded="true"]').length, 1)
})

test('failed, cancelled, and waiting groups retain their visible status', () => {
  const failed = render(timeline([...events, event(6, 'run.failed', { code: 'provider_error', message: '失败' })]))
  assert.match(failed.container.querySelector('.fox-assistant-content').textContent, /过程未完成/)
  cleanup()
  const cancelled = render(timeline([...events, event(6, 'run.cancelled')]))
  assert.match(cancelled.container.querySelector('.fox-assistant-content').textContent, /过程已取消/)
  cleanup()
  const waiting = render(timeline(events.map(item => item.seq === 4
    ? event(4, 'tool.completed', { toolCallId: 'read', tool: 'read', result: { status: 'awaiting_user' } }) : item)))
  assert.match(waiting.container.querySelector('.fox-assistant-content').textContent, /等待你的回答/)
})

test('run notices remain visible when process rows are collapsed', async () => {
  const view = render(timeline([...events, event(6, 'run.retrying')]))
  await waitFor(() => assert.ok(view.queryByRole('button', { name: /收起过程/ })))
  fireEvent.click(view.getByRole('button', { name: /收起过程/ }))
  assert.match(view.container.querySelector('.fox-process-notice').textContent, /正在重试/)
})

test('Kernel messages and multiple assistant messages in one run use the safe legacy display', () => {
  const kernel = render(React.createElement(RuntimeTimeline, {
    ...timeline(events).props,
    messages: [messages[0], { ...messages[1], id: 'kernel-message:run:1' }],
  }))
  assert.equal(kernel.container.querySelectorAll('.fox-answer-segment').length, 0)
  cleanup()
  const multiple = render(React.createElement(RuntimeTimeline, {
    ...timeline(events).props,
    messages: [messages[0], { ...messages[1], content: '第一段', id: 'earlier' }, { ...messages[1], id: 'assistant-run', content: '第二段', ordinal: 3 }],
  }))
  assert.equal(multiple.container.querySelectorAll('.fox-answer-segment').length, 0)
  assert.match(multiple.container.querySelector('.fox-answer-body').textContent, /第一段/)
  assert.match(multiple.container.querySelector('.fox-answer-body').textContent, /第二段/)
})
