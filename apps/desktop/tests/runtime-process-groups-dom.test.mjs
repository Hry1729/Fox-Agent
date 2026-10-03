import assert from 'node:assert/strict'
import { after, afterEach, test } from 'node:test'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { createServer } from 'vite'

const ownsDomRegistration = !GlobalRegistrator.isRegistered
if (ownsDomRegistration) GlobalRegistrator.register()
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
afterEach(() => { cleanup(); persistProcessDisplayMode('standard') })
// Late animation callbacks from rendered controls can outlive the final test.
// The test file runs in its own process, so leave its DOM registered until exit.
after(async () => { await server.close() })

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
const timeline = (runtimeEvents, overrides = {}) => React.createElement(RuntimeTimeline, {
  messages, attachments: [], artifacts: [], events: runtimeEvents, runtimeRunning: false,
  state: 'idle', streamingText: '', onRetry: () => {}, onRerun: async () => false,
  ...overrides,
})

test('a run still waiting for an execution slot says so, with its real queue wait', async () => {
  const promptedAt = Date.now() - 585_000
  const queued = timeline([], {
    runtimeRunning: true, activeRunId: 'run', state: 'streaming',
    messages: [
      { ...messages[0], createdAt: promptedAt, updatedAt: promptedAt },
      { ...messages[1], content: '', status: 'streaming' },
    ],
  })
  const view = render(queued)
  // No run.started yet: this Run is holding a place in the shared execution queue,
  // which must not be reported as "analysing the request".
  await waitFor(() => assert.match(view.container.querySelector('.fox-runtime-process-summary')?.textContent ?? '', /排队等待执行/))
  assert.match(view.container.querySelector('.fox-runtime-process-summary').textContent, /已排队 \d+分/)
  assert.doesNotMatch(view.container.textContent, /正在分析请求/)
})

test('a started run reports the model wait and the generation stage separately', async () => {
  const promptedAt = Date.now() - 90_000
  const started = { ...event(1, 'run.started'), createdAt: Date.now() - 36_000 }
  const queuedFirst = timeline([started], {
    runtimeRunning: true, activeRunId: 'run', state: 'streaming',
    messages: [
      { ...messages[0], createdAt: promptedAt, updatedAt: promptedAt },
      { ...messages[1], content: '', status: 'streaming' },
    ],
  })
  const view = render(queuedFirst)
  await waitFor(() => assert.match(view.container.querySelector('.fox-runtime-process-summary')?.textContent ?? '', /等待模型响应/))
  assert.match(view.container.querySelector('.fox-runtime-process-summary').textContent, /已等待 3[0-9]秒/)

  await React.act(async () => {
    view.rerender(timeline([started, event(2, 'reasoning.delta', { delta: '先分析' })], {
      runtimeRunning: true, activeRunId: 'run', state: 'streaming',
      messages: [
        { ...messages[0], createdAt: promptedAt, updatedAt: promptedAt },
        { ...messages[1], content: '', status: 'streaming' },
      ],
    }))
  })
  await waitFor(() => assert.match(view.container.querySelector('.fox-runtime-process-summary')?.textContent ?? '', /正在生成/))
})

test('answer segments remain once and in order while the whole process is collapsed', async () => {
  const view = render(timeline(events))
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-answer-segment').length, 2), { timeout: 5000 })
  let segments = view.container.querySelectorAll('.fox-answer-segment')
  assert.equal(segments.length, 2)
  assert.match(segments[0].textContent, /第一段/)
  assert.match(segments[1].textContent, /第二段/)
  assert.equal(segments[0].hidden, true)
  assert.equal(segments[1].hidden, false)
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 2)
  fireEvent.click(view.getByRole('button', { name: /展开过程/ }))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  assert.equal(view.container.querySelectorAll('.fox-chain-of-thought-header[aria-expanded="true"]').length, 1)
  fireEvent.click(view.getByRole('button', { name: /收起过程/ }))
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 2)
  segments = view.container.querySelectorAll('.fox-answer-segment')
  assert.equal(segments.length, 2)
  assert.equal(segments[0].hidden, true)
  assert.equal(segments[1].hidden, false)
  fireEvent.click(view.getByRole('button', { name: /展开过程/ }))
  assert.equal(view.container.querySelectorAll('.fox-runtime-process').length, 2)
  assert.equal(view.container.querySelectorAll('.fox-chain-of-thought-header[aria-expanded="true"]').length, 0)
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

test('failed, cancelled, and waiting groups retain their visible status after grouping loads', async () => {
  const failed = render(timeline([...events, event(6, 'run.failed', { code: 'provider_error', message: '失败' })]))
  await waitFor(() => assert.equal(failed.container.querySelectorAll('.fox-answer-segment').length, 2))
  assert.match(failed.container.querySelector('.fox-assistant-content').textContent, /过程未完成/)
  cleanup()
  const cancelled = render(timeline([...events, event(6, 'run.cancelled')]))
  await waitFor(() => assert.equal(cancelled.container.querySelectorAll('.fox-answer-segment').length, 2))
  assert.match(cancelled.container.querySelector('.fox-assistant-content').textContent, /过程已取消/)
  cleanup()
  const waiting = render(timeline(events.map(item => item.seq === 4
    ? event(4, 'tool.completed', { toolCallId: 'read', tool: 'read', result: { status: 'awaiting_user' } }) : item)))
  await waitFor(() => assert.equal(waiting.container.querySelectorAll('.fox-answer-segment').length, 2))
  assert.match(waiting.container.querySelector('.fox-assistant-content').textContent, /等待你的回答/)
})

test('run notices remain visible when process rows are collapsed', async () => {
  const view = render(timeline([...events, event(6, 'run.retrying')]))
  await waitFor(() => assert.ok(view.queryByRole('button', { name: /展开过程/ })))
  assert.match(view.container.querySelector('.fox-process-notice').textContent, /正在重试/)
})

test('Kernel messages and multiple assistant messages in one run use the safe legacy display after grouping loads', async () => {
  const kernel = render(timeline(events))
  await waitFor(() => assert.equal(kernel.container.querySelectorAll('.fox-answer-segment').length, 2))
  kernel.rerender(timeline(events, {
    messages: [messages[0], { ...messages[1], id: 'kernel-message:run:1' }],
  }))
  assert.equal(kernel.container.querySelectorAll('.fox-answer-segment').length, 0)
  cleanup()
  const multiple = render(timeline(events))
  await waitFor(() => assert.equal(multiple.container.querySelectorAll('.fox-answer-segment').length, 2))
  multiple.rerender(timeline(events, {
    messages: [messages[0], { ...messages[1], content: '第一段', id: 'earlier' }, { ...messages[1], id: 'assistant-run', content: '第二段', ordinal: 3 }],
  }))
  assert.equal(multiple.container.querySelectorAll('.fox-answer-segment').length, 0)
  assert.match(multiple.container.querySelector('.fox-answer-body').textContent, /第一段/)
  assert.match(multiple.container.querySelector('.fox-answer-body').textContent, /第二段/)
})

test('a failed tool stays visible in its stage and in the collapsed turn summary', async () => {
  const failedToolEvents = events.map(item => item.seq === 4
    ? event(4, 'tool.completed', { toolCallId: 'read', tool: 'read', result: { error: 'permission denied' }, isError: true }) : item)
  const view = render(timeline(failedToolEvents))
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-answer-segment').length, 2))
  assert.match(view.container.querySelectorAll('.fox-runtime-process-summary')[1].textContent, /失败/)
  assert.match(view.getByRole('button', { name: /展开过程/ }).textContent, /失败/)
})

test('a reply closes its preceding process group even when a tool has no result yet', async () => {
  const liveEvents = events.filter(item => item.seq !== 4)
  const view = render(timeline(liveEvents, {
    runtimeRunning: true, activeRunId: 'run', state: 'streaming',
    messages: [messages[0], { ...messages[1], status: 'streaming' }],
  }))
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-answer-segment').length, 2))
  assert.match(view.container.querySelectorAll('.fox-runtime-process-summary')[1].textContent, /读取了文件/)
  assert.equal(view.container.querySelectorAll('.fox-runtime-process-summary.is-running').length, 0)
  view.rerender(timeline([...liveEvents, event(6, 'run.completed')], {
    runtimeRunning: true, activeRunId: 'run', state: 'streaming',
    messages: [messages[0], { ...messages[1], status: 'streaming' }],
  }))
  assert.equal(view.container.querySelectorAll('.fox-runtime-process-summary.is-running').length, 0)
})

test('opening a live stage jumps to its end while reopening a completed stage starts at its top', async () => {
  const height = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollHeight')
  Object.defineProperty(HTMLElement.prototype, 'scrollHeight', { configurable: true, get() { return this.classList.contains('fox-runtime-process-scroll') ? 1000 : 0 } })
  try {
    const liveEvents = [...events.filter(item => item.seq !== 4), event(6, 'reasoning.delta', { delta: '继续分析' })]
    const view = render(timeline(liveEvents, {
      runtimeRunning: true, activeRunId: 'run', state: 'streaming',
      messages: [messages[0], { ...messages[1], status: 'streaming' }],
    }))
    await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-answer-segment').length, 2))
    const headers = view.container.querySelectorAll('.fox-chain-of-thought-header')
    fireEvent.click(headers[2])
    const liveScroll = view.container.querySelectorAll('.fox-runtime-process-scroll')[0]
    assert.equal(liveScroll.scrollTop, 1000)
    fireEvent.click(headers[0])
    const completedScroll = view.container.querySelectorAll('.fox-runtime-process-scroll')[0]
    completedScroll.scrollTop = 200
    fireEvent.click(headers[0])
    fireEvent.click(headers[0])
    assert.equal(completedScroll.scrollTop, 0)
  } finally {
    if (height) Object.defineProperty(HTMLElement.prototype, 'scrollHeight', height)
    else delete HTMLElement.prototype.scrollHeight
  }
})


test('Kernel rounds group by Host ownership and retain disclosures across preview replacement and reload', async () => {
  const kernelEvents = [
    event(1, 'tool.started', { toolCallId: 'grep', tool: 'grep', input: { pattern: 'a' }, kernelCheckpointSeq: '2' }),
    event(2, 'reasoning.delta', { source: 'kernel-model:2', delta: '先分析请求' }),
    event(3, 'tool.completed', { toolCallId: 'grep', tool: 'grep', kernelCheckpointSeq: '2', result: { content: 'found' } }),
    event(4, 'reasoning.delta', { source: 'kernel-model:12', delta: '整理搜索结果' }),
  ]
  const first = { ...messages[1], id: 'kernel-message:run:2', content: '先搜索代码', status: 'completed' }
  const last = { ...messages[1], id: 'kernel-message:run:12', content: '旧答案', ordinal: 3,
    kernelPreview: { checkpointSeq: 12, revision: 1, reasoning: '预览推理' } }
  const renderKernel = (tail) => timeline(kernelEvents, { messages: [messages[0], first, tail], runtimeRunning: true })
  const view = render(renderKernel(last))
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-process-stage').length, 2))
  assert.deepEqual([...view.container.querySelectorAll('.fox-chain-of-thought-header')].map(el => el.getAttribute('aria-expanded')), ['false', 'false'])
  fireEvent.click(view.container.querySelectorAll('.fox-chain-of-thought-header')[1])
  assert.match(view.container.textContent, /预览推理/)
  await React.act(async () => { view.rerender(renderKernel({ ...last, content: '新答案', kernelPreview: { checkpointSeq: 12, revision: 2, reasoning: '新的推理' } })) })
  await waitFor(() => { assert.match(view.container.textContent, /新的推理/); assert.match(view.container.textContent, /新答案/) })
  assert.doesNotMatch(view.container.textContent, /预览推理|旧答案/)
  assert.equal(view.container.querySelectorAll('.fox-chain-of-thought-header')[1].getAttribute('aria-expanded'), 'true')
  assert.deepEqual([...view.container.querySelectorAll('.fox-answer-segment')].map(el => el.textContent.trim()), ['先搜索代码', '新答案'])
  view.rerender(timeline(kernelEvents, { messages: [messages[0], first, { ...last, kernelPreview: undefined, content: '最终答案', status: 'completed' }] }))
  await waitFor(() => assert.match(view.container.textContent, /整理搜索结果/))
  assert.doesNotMatch(view.container.textContent, /新的推理/)
})

test('mode changes keep manual group and reasoning disclosure state', async () => {
  const markdownEvents = events.map(item => item.seq === 1
    ? event(1, 'reasoning.delta', { delta: '**判断**\n\n- 查证' }) : item)
  const view = render(timeline(markdownEvents))
  await waitFor(() => assert.ok(view.queryByRole('button', { name: /展开过程/ })))
  fireEvent.click(view.getByRole('button', { name: /展开过程/ }))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  const thought = view.container.querySelector('.fox-runtime-step-row')
  fireEvent.click(thought.querySelector('.fox-runtime-step-trigger'))
  await waitFor(() => assert.ok(thought.querySelector('.fox-reasoning-text [data-streamdown="strong"]')), { timeout: 3000 })
  assert.equal(thought.querySelector('.fox-run-status-prefix').textContent, '深度思考')
  assert.equal(thought.querySelector('.fox-run-status-label').textContent, '')
  await React.act(async () => persistProcessDisplayMode('verbose'))
  assert.equal(view.container.querySelectorAll('.fox-runtime-process.is-fully-expanded').length, 2)
  assert.equal(thought.getAttribute('data-state'), 'open')
  await React.act(async () => persistProcessDisplayMode('standard'))
  assert.equal(thought.getAttribute('data-state'), 'open')
  assert.equal(view.container.querySelectorAll('.fox-chain-of-thought-header[aria-expanded="true"]').length, 1)
})

test('streaming thought preview advances only after a paragraph first line completes', async () => {
  const first = '第一段首行\n第一段续行\n\n第二段未完成'
  const renderReasoning = (reasoning, streaming) => timeline([], {
    runtimeRunning: streaming, activeRunId: streaming ? 'run' : undefined,
    state: streaming ? 'streaming' : 'idle',
    messages: [messages[0], { ...messages[1], id: 'kernel-message:run:1', content: '',
      status: streaming ? 'streaming' : 'completed',
      kernelPreview: { checkpointSeq: 1, revision: reasoning.length, reasoning } }],
  })
  const view = render(renderReasoning(first, true))
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-process-stage').length, 1))
  const label = () => view.container.querySelector('.fox-runtime-step-row .fox-run-status-label')?.textContent
  // Grouping helpers arrive asynchronously and can remount the process, which
  // drops a disclosure click made before they settle. Re-open until the row exists.
  await waitFor(() => {
    const header = view.container.querySelector('.fox-chain-of-thought-header')
    if (header?.getAttribute('aria-expanded') !== 'true') fireEvent.click(header)
    assert.equal(typeof label(), 'string')
  }, { timeout: 3000 })
  // One continuous summary line derived from the current thought: whitespace
  // collapses, but nothing is dropped while the thought is still streaming.
  await waitFor(() => assert.equal(label(), '第一段首行 第一段续行 第二段未完成'))
  await React.act(async () => { view.rerender(renderReasoning(`${first}继续写`, true)) })
  assert.equal(label(), '第一段首行 第一段续行 第二段未完成继续写')
  await React.act(async () => { view.rerender(renderReasoning(`${first}继续写\n第二段续行`, true)) })
  assert.equal(label(), '第一段首行 第一段续行 第二段未完成继续写 第二段续行')
  await React.act(async () => { view.rerender(renderReasoning('CRLF首行\r\n\r\n第二段未完成', true)) })
  assert.equal(label(), 'CRLF首行 第二段未完成')
  await React.act(async () => { view.rerender(renderReasoning('带空白首行\r\n  \r\n第二段未完成', true)) })
  assert.equal(label(), '带空白首行 第二段未完成')
  await React.act(async () => { view.rerender(renderReasoning('尚未换行', true)) })
  assert.equal(label(), '尚未换行')
  assert.equal(view.container.querySelector('.fox-runtime-step-row .fox-run-status-prefix').textContent, '深度思考 ·')
  await React.act(async () => { view.rerender(renderReasoning('**结算首行**\n后续正文', false)) })
  assert.equal(label(), '结算首行 后续正文')
  assert.equal(view.container.querySelector('.fox-runtime-step-row .fox-run-status-prefix').textContent, '深度思考 ·')
  fireEvent.click(view.container.querySelector('.fox-runtime-step-row .fox-runtime-step-trigger'))
  assert.equal(view.container.querySelector('.fox-runtime-step-row .fox-run-status-prefix').textContent, '深度思考')
  assert.equal(label(), '')
})

test('running turn stays visible and settles to a folded process with its answer exposed', async () => {
  const live = timeline(events.filter(item => item.seq !== 4), {
    runtimeRunning: true, activeRunId: 'run', state: 'streaming',
    messages: [messages[0], { ...messages[1], status: 'streaming' }],
  })
  const view = render(live)
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-process-stage:not([hidden])').length, 2))
  assert.equal(view.getByRole('button', { name: /收起过程/ }).getAttribute('aria-expanded'), 'true')
  view.rerender(timeline(events))
  await waitFor(() => assert.ok(view.queryByRole('button', { name: /展开过程/ })))
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 2)
  assert.equal(view.container.querySelectorAll('.fox-answer-segment:not([hidden])').length, 1)
})

test('automatic completion fold does not hide a focused process control', async () => {
  const live = timeline(events.filter(item => item.seq !== 4), {
    runtimeRunning: true, activeRunId: 'run', state: 'streaming',
    messages: [messages[0], { ...messages[1], status: 'streaming' }],
  })
  const view = render(live)
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-process-stage').length, 2))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  const row = view.container.querySelector('.fox-runtime-step-trigger')
  await React.act(async () => { row.focus() })
  assert.equal(document.activeElement, row)
  await React.act(async () => { view.rerender(timeline(events)) })
  await waitFor(() => assert.ok(view.queryByRole('button', { name: /收起过程/ })))
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 0)
  const toggle = view.getByRole('button', { name: /收起过程/ })
  await React.act(async () => { toggle.focus() })
  assert.equal(document.activeElement, toggle)
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 0)
  fireEvent.click(toggle)
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 2)
})

test('automatic completion fold keeps selected process text until the reader closes it', async () => {
  const live = timeline(events.filter(item => item.seq !== 4), {
    runtimeRunning: true, activeRunId: 'run', state: 'streaming',
    messages: [messages[0], { ...messages[1], status: 'streaming' }],
  })
  const view = render(live)
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-process-stage').length, 2))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  const label = view.container.querySelector('.fox-runtime-step-row .fox-run-status-prefix')
  const range = document.createRange()
  range.selectNodeContents(label)
  const selection = document.getSelection()
  await React.act(async () => {
    selection.removeAllRanges()
    selection.addRange(range)
    document.dispatchEvent(new Event('selectionchange'))
  })
  await React.act(async () => { view.rerender(timeline(events)) })
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 0)
  await React.act(async () => { selection.removeAllRanges(); document.dispatchEvent(new Event('selectionchange')) })
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 0)
  fireEvent.click(view.getByRole('button', { name: /收起过程/ }))
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 2)
})

test('compact mode previews only the live reasoning row in a running group', async () => {
  await React.act(async () => persistProcessDisplayMode('compact'))
  const thoughts = [
    event(1, 'reasoning.delta', { source: 'kernel-model:1', delta: '旧首行\n旧续行' }),
    event(2, 'reasoning.delta', { kernelCheckpointSeq: '1', delta: '最新首行\n最新续行' }),
  ]
  const message = { ...messages[1], id: 'kernel-message:run:1', content: '', status: 'streaming' }
  const view = render(timeline(thoughts, { runtimeRunning: true, activeRunId: 'run', state: 'streaming', messages: [messages[0], message] }))
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-process-stage').length, 1))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-runtime-step-row').length, 2))
  const rows = view.container.querySelectorAll('.fox-runtime-step-row')
  assert.deepEqual([...rows].map(row => row.querySelector('.fox-run-status-prefix')?.textContent), ['深度思考', '深度思考 ·'])
  assert.deepEqual([...rows].map(row => row.querySelector('.fox-run-status-label')?.textContent), ['', '最新首行 最新续行'])
})

test('detailed mode opens only running groups and compact mode hides settled thought previews', async () => {
  await React.act(async () => persistProcessDisplayMode('detailed'))
  const live = timeline(events.filter(item => item.seq !== 4), {
    runtimeRunning: true, activeRunId: 'run', state: 'streaming',
    messages: [messages[0], { ...messages[1], status: 'streaming' }],
  })
  const view = render(live)
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-runtime-process.is-fully-expanded').length, 2))
  view.rerender(timeline(events))
  assert.equal(view.container.querySelectorAll('.fox-runtime-process.is-fully-expanded').length, 0)
  await React.act(async () => persistProcessDisplayMode('compact'))
  fireEvent.click(view.getByRole('button', { name: /展开过程/ }))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  const thought = view.container.querySelector('.fox-runtime-step-row')
  assert.equal(thought.querySelector('.fox-run-status-prefix').textContent, '深度思考')
  assert.equal(thought.querySelector('.fox-run-status-label').textContent, '')
})

test('prepending an older Kernel reasoning round keeps the existing open group and turn', async () => {
  const older = { ...messages[1], id: 'kernel-message:run:1', content: '', ordinal: 2,
    kernelPreview: { checkpointSeq: 1, revision: 1, reasoning: '更早的思考' } }
  const newest = { ...messages[1], id: 'kernel-message:run:2', content: '最终答案', ordinal: 3,
    kernelPreview: { checkpointSeq: 2, revision: 1, reasoning: '原有的思考' } }
  const view = render(timeline([], { messages: [messages[0], newest] }))
  await waitFor(() => assert.ok(view.queryByRole('button', { name: /展开过程/ })))
  fireEvent.click(view.getByRole('button', { name: /展开过程/ }))
  fireEvent.click(view.container.querySelector('.fox-chain-of-thought-header'))
  const originalStage = view.container.querySelector('.fox-process-stage')
  await React.act(async () => { view.rerender(timeline([], { messages: [messages[0], older, newest] })) })
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-markdown-loading').length, 0))
  assert.equal(view.container.querySelector('.fox-process-stage'), originalStage)
  assert.equal(view.container.querySelector('.fox-chain-of-thought-header').getAttribute('aria-expanded'), 'true')
  assert.equal(view.getByRole('button', { name: /收起过程/ }).getAttribute('aria-expanded'), 'true')
  assert.match(view.container.querySelector('.fox-process-stage').textContent, /更早的思考/)
})

test('a user input after process evidence stays visible when the reader folds the process', async () => {
  const assistant = { ...messages[1], id: 'kernel-message:run:1', content: '阶段回复',
    kernelPreview: { checkpointSeq: 1, revision: 1, reasoning: '先查证' } }
  const steering = { ...messages[0], id: 'steering', ordinal: 3, content: '继续检查' }
  const view = render(timeline([], { messages: [messages[0], assistant, steering] }))
  await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-process-stage').length, 1))
  assert.equal(view.container.querySelectorAll('.fox-process-turn-toggle.is-status').length, 0)
  assert.equal(view.container.querySelector('.fox-process-turn-toggle').getAttribute('aria-expanded'), 'true')
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 0)
  fireEvent.click(view.getByRole('button', { name: /收起过程/ }))
  assert.equal(view.container.querySelectorAll('.fox-process-stage[hidden]').length, 1)
  assert.ok(view.getByText('继续检查').getBoundingClientRect)
  assert.equal(view.getByText('继续检查').closest('[hidden]'), null)
})

test('an expanded live group follows new rows only while its own scroll remains at bottom', async () => {
  const height = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'scrollHeight')
  const client = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'clientHeight')
  let contentHeight = 1000
  Object.defineProperty(HTMLElement.prototype, 'scrollHeight', { configurable: true, get() { return this.classList.contains('fox-runtime-process-scroll') ? contentHeight : 0 } })
  Object.defineProperty(HTMLElement.prototype, 'clientHeight', { configurable: true, get() { return this.classList.contains('fox-runtime-process-scroll') ? 400 : 0 } })
  try {
    const running = (items) => timeline(items, { runtimeRunning: true, activeRunId: 'run', state: 'streaming',
      messages: [messages[0], { ...messages[1], status: 'streaming' }] })
    const tail = [...events.filter(item => item.seq !== 4), event(6, 'reasoning.delta', { delta: '继续分析' })]
    const view = render(running(tail))
    await waitFor(() => assert.equal(view.container.querySelectorAll('.fox-process-stage').length, 3))
    fireEvent.click(view.container.querySelectorAll('.fox-chain-of-thought-header')[2])
    const scroll = view.container.querySelector('.fox-runtime-process-scroll')
    assert.equal(scroll.scrollTop, 600)
    const movements = []
    scroll.scrollTo = (options) => { movements.push(options); if (options.behavior !== 'smooth') scroll.scrollTop = options.top }
    scroll.scrollTop = 250
    fireEvent.scroll(scroll)
    contentHeight = 1200
    view.rerender(running([...tail, event(7, 'reasoning.delta', { delta: '更多内容' })]))
    assert.equal(scroll.scrollTop, 250)
    assert.equal(movements.length, 0)
    scroll.scrollTop = 800
    fireEvent.scroll(scroll)
    contentHeight = 1400
    view.rerender(running([...tail, event(7, 'reasoning.delta', { delta: '更多内容' }), event(8, 'reasoning.delta', { delta: '末尾内容' })]))
    assert.equal(scroll.scrollTop, 800)
    assert.deepEqual(movements, [{ top: 1000, behavior: 'smooth' }])
    contentHeight = 1600
    view.rerender(running([...tail, event(7, 'reasoning.delta', { delta: '更多内容' }), event(8, 'reasoning.delta', { delta: '末尾内容' }), event(9, 'reasoning.delta', { delta: '新目标' })]))
    assert.equal(movements.length, 1, 'do not restart native smooth scrolling on every delta')
    scroll.scrollTop = 1000
    fireEvent(scroll, new Event('scrollend'))
    assert.deepEqual(movements.at(-1), { top: 1200, behavior: 'smooth' })
  } finally {
    if (height) Object.defineProperty(HTMLElement.prototype, 'scrollHeight', height)
    else delete HTMLElement.prototype.scrollHeight
    if (client) Object.defineProperty(HTMLElement.prototype, 'clientHeight', client)
    else delete HTMLElement.prototype.clientHeight
  }
})
