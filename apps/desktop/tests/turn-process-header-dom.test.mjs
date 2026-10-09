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
  // Failure state and diagnostics are separate checks:
  //  1. the row keeps stating the failure in the whitelisted wording, and the
  //     internal diagnostic body is never rendered (this test used to require the
  //     raw `not_found` text, which the whitelist policy deliberately withholds);
  assert.match(view.container.textContent, /工具调用未成功|本次操作未能完成/)
  assert.doesNotMatch(view.container.textContent, /not_found/)
  //  2. the source path is verified where the interface really shows one — the
  //     successful sibling read of README.md — so a missing path on an errored row
  //     can never be confused with a missing error state. (In this harness no open
  //     handler is provided, so the path renders as row text rather than as the
  //     clickable `.fox-runtime-step-path` element.)
  assert.match(view.container.textContent, /README\.md/)
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

/** The header's visible line is the toggle's own text plus the separator the header
 *  renders only when there is status text, plus the one whole-round timer. */
const header = (view) => view.container.querySelector('.fox-process-turn-header')
const toggle = (view, state = '收起') => view.getByRole('button', { name: new RegExp(`${state}过程`) })

test('the round header joins its own fields: no leading separator, and one whole-round timer', async () => {
  const view = render(live())
  await waitFor(() => assert.equal(stages(view).length, 2))
  // Running and expanded: [avatar] [chevron] 正在处理 · 已用时 N秒
  assert.equal(toggle(view).textContent, '正在处理')
  assert.equal(toggle(view).getAttribute('aria-label'), '收起过程 · 正在处理')
  assert.equal(toggle(view).getAttribute('title'), '收起过程')
  assert.equal(toggle(view).getAttribute('aria-expanded'), 'true')
  // A native button keeps Enter/Space working; nothing about the line changed that.
  assert.equal(toggle(view).tagName, 'BUTTON')
  assert.equal(toggle(view).getAttribute('type'), 'button')
  assert.match(header(view).textContent, /^正在处理·已用时 \d+秒$/)
  assert.equal(view.container.querySelectorAll('[role="timer"]').length, 1)
  assert.equal(view.container.querySelectorAll('.fox-process-turn-separator').length, 1)

  // Collapsed: the same fields, folded — the status text is never rewritten.
  fireEvent.click(toggle(view))
  assert.equal(toggle(view, '展开').getAttribute('aria-expanded'), 'false')
  assert.equal(toggle(view, '展开').textContent, '正在处理')
  assert.equal(toggle(view, '展开').getAttribute('aria-label'), '展开过程 · 正在处理')
  assert.match(header(view).textContent, /^正在处理·已用时 \d+秒$/)
  fireEvent.click(toggle(view, '展开'))
  assert.equal(toggle(view).getAttribute('aria-expanded'), 'true')
  assert.equal(toggle(view).textContent, '正在处理')
})

test('a settled turn leaves no dangling separator and shows the whole-round time once', async () => {
  const view = render(timeline())
  await waitFor(() => assert.equal(stages(view).length, 2))
  // 工作过程 is the settled default, not a state: the line holds nothing before the
  // timer, and a settled turn folds itself, so the control reads 展开过程.
  const settled = () => view.getByRole('button', { name: /^(展开|收起)过程$/ })
  assert.equal(settled().getAttribute('aria-expanded'), 'false')
  assert.equal(settled().textContent, '')
  assert.equal(settled().getAttribute('aria-label'), '展开过程')
  assert.equal(header(view).textContent, '用时 6秒')
  assert.doesNotMatch(header(view).textContent, /·/)
  assert.equal(view.container.querySelectorAll('.fox-process-turn-separator').length, 0)
  assert.equal(view.container.querySelectorAll('[role="timer"]').length, 1)
  // Both fold states read the same: the fields are independent of the disclosure.
  fireEvent.click(settled())
  assert.equal(view.getByRole('button', { name: '收起过程' }).textContent, '')
  assert.equal(header(view).textContent, '用时 6秒')
  assert.equal(view.container.querySelectorAll('.fox-process-turn-separator').length, 0)
})

test('failed, cancelled and interrupted turns keep their own status as the first field', async () => {
  const cases = [
    { name: 'failed', events: [...events.slice(0, -1), event(7, 'run.failed', { code: 'provider_error', message: '失败' })],
      status: '过程未完成', headerText: '过程未完成·用时 6秒' },
    { name: 'cancelled', events: [...events.slice(0, -1), event(7, 'run.cancelled')],
      status: '过程已取消', headerText: '过程已取消·用时 6秒' },
    // Interrupted before the run recorded an end: there is no whole-round time to
    // separate the status from, so the header must not leave a dangling separator.
    { name: 'interrupted', events: events.slice(0, -1), messages: [user, { ...assistant, status: 'interrupted' }],
      status: '过程已中断', headerText: '过程已中断' },
  ]
  for (const item of cases) {
    const view = render(item.messages ? timeline(item.events, { messages: item.messages }) : timeline(item.events))
    await waitFor(() => assert.equal(stages(view).length, 2))
    const toggleText = toggle(view).textContent
    // Exactly one state word, never prefixed by a separator of its own.
    assert.equal(toggleText, item.status, item.name)
    assert.equal(toggle(view).getAttribute('aria-label'), `收起过程 · ${item.status}`)
    assert.equal(header(view).textContent, item.headerText, item.name)
    if (item.name === 'interrupted') {
      assert.equal(view.container.querySelectorAll('[role="timer"]').length, 0)
      assert.equal(view.container.querySelectorAll('.fox-process-turn-separator').length, 0)
    } else {
      assert.equal((header(view).textContent.match(/·/g) ?? []).length, 1)
    }
    fireEvent.click(toggle(view))
    assert.equal(toggle(view, '展开').textContent, item.status)
    assert.equal(header(view).textContent, item.headerText, item.name)
    cleanup()
  }
})

test('a turn waiting on the model says so without a leading separator', async () => {
  const waiting = [
    event(1, 'run.started'),
    event(2, 'run.model_waiting', { phase: 'model_response', requestId: 'req-1', dispatchSeq: 1,
      startedAt: baseTime + 1000, elapsedMs: 1000, remainingMs: 1000, outcomeKnown: false }),
  ]
  const view = render(live(waiting))
  await waitFor(() => assert.equal(toggle(view).textContent, '正在等待回复'))
  assert.equal(toggle(view).getAttribute('aria-label'), '收起过程 · 正在等待回复')
  assert.match(header(view).textContent, /^正在等待回复·已用时 \d+秒$/)
  assert.doesNotMatch(header(view).textContent, /^·/)
})

test('a planned tool retry keeps its note joined to the state by exactly one separator', async () => {
  const retrying = [
    event(1, 'run.started'),
    event(2, 'tool.started', { toolCallId: 'read', tool: 'read', input: { path: 'README.md' } }),
    event(3, 'tool.completed', { toolCallId: 'read', tool: 'read', isError: true, result: { code: 'tool.unknown' } }),
  ]
  const view = render(live(retrying))
  await waitFor(() => assert.equal(toggle(view).textContent, '正在处理 · 工具调用待调整'))
  assert.equal(toggle(view).getAttribute('aria-label'), '收起过程 · 正在处理 · 工具调用待调整')
  assert.match(header(view).textContent, /^正在处理 · 工具调用待调整·已用时 \d+秒$/)
  // The separator belongs to the join: the state line has exactly one, and the
  // header adds exactly one more before the timer.
  assert.equal((header(view).textContent.match(/·/g) ?? []).length, 2)
})

test('a header without a fold control keeps its own word and joins the timer to it', async () => {
  await React.act(async () => persistProcessDisplayMode('verbose'))
  const view = render(timeline())
  await waitFor(() => assert.ok(view.container.querySelector('.fox-process-turn-toggle.is-status')))
  const status = view.container.querySelector('.fox-process-turn-toggle.is-status')
  assert.equal(status.textContent, '工作过程')
  assert.equal(status.getAttribute('role'), 'status')
  assert.equal(header(view).textContent, '工作过程·用时 6秒')
  assert.equal(view.container.querySelectorAll('[role="timer"]').length, 1)
})

