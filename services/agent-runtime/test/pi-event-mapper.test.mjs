import test from 'node:test'
import assert from 'node:assert/strict'
import { createPiEventMapper, sanitizeAssistantHistory, splitMixedAssistantText } from '../src/pi-event-mapper.mjs'

test('maps Pi streaming, tool and completion events to stable Fox events', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'hello' } })
  mapper.handle({
    type: 'message_update',
    assistantMessageEvent: {
      type: 'thinking_delta',
      contentIndex: 0,
      delta: 'checking',
      partial: { content: [{ type: 'thinking', thinking: 'checking', thinkingSignature: 'reasoning_content' }] },
    },
  })
  mapper.handle({ type: 'tool_execution_start', toolCallId: 'tool-1', toolName: 'read', args: { path: 'a.txt' } })
  mapper.handle({ type: 'tool_execution_end', toolCallId: 'tool-1', toolName: 'read', result: {}, isError: false })
  mapper.handle({ type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text: 'hello' }], stopReason: 'stop', usage: { input: 2, output: 3, cacheRead: 4, cacheWrite: 1, totalTokens: 10 } } })
  mapper.handle({ type: 'agent_end', messages: [] })
  assert.deepEqual(events.map((event) => event.type), [
    'message.started', 'message.delta', 'reasoning.delta', 'tool.started', 'tool.completed',
    'usage.updated', 'message.completed', 'run.completed',
  ])
  assert.equal(events.find((event) => event.type === 'reasoning.delta')?.providerField, 'reasoning_content')
  assert.deepEqual(events.find((event) => event.type === 'usage.updated'), {
    type: 'usage.updated',
    inputTokens: 2,
    outputTokens: 3,
    cacheReadTokens: 4,
    cacheWriteTokens: 1,
    totalTokens: 10,
  })
})

test('emits Run-cumulative usage across a tool round without double-counting other events', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  const firstMessageEnd = {
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'toolCall', id: 'tool-1', name: 'read', arguments: { path: 'a.txt' } }],
      stopReason: 'toolUse',
      usage: { input: 100, output: 20, cacheRead: 10, cacheWrite: 0, totalTokens: 130 },
    },
  }
  mapper.handle(firstMessageEnd)
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'thinking_delta', delta: 'checking' } })
  mapper.handle({ type: 'tool_execution_start', toolCallId: 'tool-1', toolName: 'read', args: { path: 'a.txt' } })
  mapper.handle({ type: 'tool_execution_end', toolCallId: 'tool-1', toolName: 'read', result: {}, isError: false })
  const secondMessageEnd = {
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'text', text: 'Done.' }],
      stopReason: 'stop',
      usage: { input: 150, output: 5, cacheRead: 0, cacheWrite: 0, totalTokens: 155 },
    },
  }
  mapper.handle(secondMessageEnd)
  mapper.handle(secondMessageEnd)

  assert.deepEqual(events.filter(({ type }) => type === 'usage.updated'), [
    {
      type: 'usage.updated',
      inputTokens: 100,
      outputTokens: 20,
      cacheReadTokens: 10,
      cacheWriteTokens: 0,
      totalTokens: 130,
    },
    {
      type: 'usage.updated',
      inputTokens: 250,
      outputTokens: 25,
      cacheReadTokens: 10,
      cacheWriteTokens: 0,
      totalTokens: 285,
    },
  ])
})

test('resets cumulative usage for every mapper Run and raises low provider totals to component totals', () => {
  const firstRun = []
  const firstMapper = createPiEventMapper((type, payload = {}) => firstRun.push({ type, ...payload }))
  firstMapper.handle({
    type: 'message_end',
    message: { role: 'assistant', usage: { input: 20, output: 5, cacheRead: 3, cacheWrite: 2, totalTokens: 1 } },
  })
  const resumedSessionNewRun = []
  const secondMapper = createPiEventMapper((type, payload = {}) => resumedSessionNewRun.push({ type, ...payload }))
  secondMapper.handle({
    type: 'message_end',
    message: { role: 'assistant', usage: { input: 2, output: 1, cacheRead: 0, cacheWrite: 0, totalTokens: 3 } },
  })

  assert.equal(firstRun.find(({ type }) => type === 'usage.updated').totalTokens, 30)
  assert.deepEqual(resumedSessionNewRun.find(({ type }) => type === 'usage.updated'), {
    type: 'usage.updated',
    inputTokens: 2,
    outputTokens: 1,
    cacheReadTokens: 0,
    cacheWriteTokens: 0,
    totalTokens: 3,
  })
})

test('fails closed on invalid or overflowing provider usage without emitting a downgrade', () => {
  for (const [field, value] of [
    ['input', Number.NaN],
    ['output', -1],
    ['cacheRead', 1.5],
    ['cacheWrite', Number.POSITIVE_INFINITY],
    ['totalTokens', Number.MAX_SAFE_INTEGER + 1],
    ['input', '1'],
    ['output', null],
  ]) {
    const events = []
    const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
    assert.throws(() => mapper.handle({
      type: 'message_end',
      message: {
        role: 'assistant',
        usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, [field]: value },
      },
    }), /\[runtime\.usage\.invalid\]/)
    assert.equal(events.some(({ type }) => type === 'usage.updated'), false)
  }

  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      usage: { input: Number.MAX_SAFE_INTEGER, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: Number.MAX_SAFE_INTEGER },
    },
  })
  assert.throws(() => mapper.handle({
    type: 'message_end',
    message: { role: 'assistant', usage: { input: 1, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 1 } },
  }), /\[runtime\.usage\.overflow\]/)
  assert.equal(events.filter(({ type }) => type === 'usage.updated').length, 1)
})

test('keeps the final non-tool response in the assistant message', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: '脚本已经创建完成。' } })
  mapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'text', text: '脚本已经创建完成。' }],
      stopReason: 'stop',
      usage: {},
    },
  })

  assert.equal(events.find((event) => event.type === 'message.delta')?.delta, '脚本已经创建完成。')
  assert.equal(events.some((event) => event.type === 'reasoning.delta'), false)
})

test('streams an ordinary final answer before message_end', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: '好的，我先给出结论。' } })

  assert.equal(events.find((event) => event.type === 'message.delta')?.delta, '好的，我先给出结论。')
})

test('streams a short English final answer before message_end', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'Done.' } })

  assert.equal(events.find((event) => event.type === 'message.delta')?.delta, 'Done.')
})

test('keeps a single Let me explain sentence in the answer stream', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  const text = 'Let me explain how this works.\n\nThe function reads each file and writes a CSV row.'
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: text } })
  mapper.handle({ type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text }], stopReason: 'stop', usage: {} } })

  assert.equal(events.filter((event) => event.type === 'message.delta').map((event) => event.delta).join(''), text)
  assert.equal(events.some((event) => event.type === 'reasoning.delta'), false)
})

test('streams provider reasoning separately before completion', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({
    type: 'message_update',
    assistantMessageEvent: {
      type: 'thinking_delta',
      contentIndex: 0,
      delta: '正在检查项目文件',
      partial: { content: [{ type: 'thinking', thinking: '正在检查项目文件', thinkingSignature: 'reasoning_content' }] },
    },
  })

  const reasoning = events.find((event) => event.type === 'reasoning.delta')
  assert.equal(reasoning?.delta, '正在检查项目文件')
  assert.equal(reasoning?.providerField, 'reasoning_content')
})

test('recovers provider reasoning that is only present in the completed message', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [
        { type: 'thinking', thinking: 'Inspect the project before answering.', thinkingSignature: 'final-only' },
        { type: 'text', text: 'Done.' },
      ],
      stopReason: 'stop',
      usage: {},
    },
  })

  assert.deepEqual(events.find((event) => event.type === 'reasoning.delta'), {
    type: 'reasoning.delta',
    delta: 'Inspect the project before answering.',
    source: 'provider',
    providerField: 'final-only',
  })
})

test('keeps an ordinary Chinese explanation in the answer stream', () => {
  const text = '这个问题的根因是路径状态没有在选择文件夹后同步更新。'
  assert.deepEqual(splitMixedAssistantText(text), { reasoning: '', answer: text })
})

test('separates MiniMax mm:think tags from the final answer', () => {
  const text = '<mm:think>I need to inspect the persisted goal first.</mm:think>目标已经确认，可以继续创建任务。'
  assert.deepEqual(splitMixedAssistantText(text), {
    reasoning: 'I need to inspect the persisted goal first.',
    answer: '目标已经确认，可以继续创建任务。',
  })
})

test('handles a MiniMax closing marker without an opening marker', () => {
  const text = 'I should not expose this planning note.</mm:think>任务已经创建。'
  assert.deepEqual(splitMixedAssistantText(text), {
    reasoning: 'I should not expose this planning note.',
    answer: '任务已经创建。',
  })
})

test('keeps ordinary explanatory wording in the final answer', () => {
  const text = 'Let me explain how this works.\n\nThe function scans the current directory and writes a CSV file.'
  assert.deepEqual(splitMixedAssistantText(text), { reasoning: '', answer: text })
})

test('normalizes plain legacy assistant strings to Pi text blocks', () => {
  const messages = sanitizeAssistantHistory([{
    role: 'assistant',
    content: '目标已提交，等待确认。',
  }])

  assert.deepEqual(messages[0].content, [{ type: 'text', text: '目标已提交，等待确认。' }])
})

test('maps Pi errors and aborts to terminal Fox events only once', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_end', message: { role: 'assistant', stopReason: 'aborted', usage: {} } })
  mapper.handle({ type: 'agent_end', messages: [] })
  assert.equal(events.filter((event) => event.type === 'run.cancelled').length, 1)
  assert.equal(events.filter((event) => event.type === 'run.completed').length, 0)
})

test('defers a provider error while Pi retries and completes only once after recovery', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_end', message: { role: 'assistant', stopReason: 'error', errorMessage: 'temporary outage', usage: {} } })
  mapper.handle({ type: 'agent_end', messages: [], willRetry: true })
  mapper.handle({ type: 'auto_retry_start', attempt: 1, maxAttempts: 3, delayMs: 50, errorMessage: 'temporary outage' })
  mapper.handle({ type: 'auto_retry_end', success: true, attempt: 1 })
  mapper.handle({ type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text: 'Recovered.' }], stopReason: 'stop', usage: {} } })
  mapper.handle({ type: 'agent_end', messages: [], willRetry: false })

  assert.equal(events.filter((event) => event.type === 'run.retrying').length, 1)
  assert.equal(events.filter((event) => event.type === 'run.retry.completed').length, 1)
  assert.equal(events.filter((event) => event.type === 'run.failed').length, 0)
  assert.equal(events.filter((event) => event.type === 'run.completed').length, 1)
})

test('emits one final failure after retry exhaustion', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_end', message: { role: 'assistant', stopReason: 'error', errorMessage: 'first failure', usage: {} } })
  mapper.handle({ type: 'agent_end', messages: [], willRetry: true })
  mapper.handle({ type: 'auto_retry_start', attempt: 1, maxAttempts: 2, delayMs: 50, errorMessage: 'first failure' })
  mapper.handle({ type: 'auto_retry_end', success: false, attempt: 1, finalError: 'final failure' })
  mapper.handle({ type: 'message_end', message: { role: 'assistant', stopReason: 'error', errorMessage: 'final failure', usage: {} } })
  mapper.handle({ type: 'agent_end', messages: [], willRetry: false })
  mapper.fail(new Error('duplicate'))

  const failures = events.filter((event) => event.type === 'run.failed')
  assert.equal(failures.length, 1)
  assert.equal(failures[0].message, 'final failure')
  assert.equal(events.filter((event) => event.type === 'run.completed').length, 0)
})

test('maps a host cancellation to one terminal event even if cleanup also fails', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.cancel()
  mapper.cancel()
  mapper.fail(new Error('cleanup failure'))

  assert.equal(events.filter((event) => event.type === 'run.cancelled').length, 1)
  assert.equal(events.filter((event) => event.type === 'run.failed').length, 0)
})

test('preserves structured runtime failure codes', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  const error = new Error('tool budget exhausted')
  error.code = 'runtime.tool_call_budget_exceeded'
  mapper.fail(error)

  assert.deepEqual(events.filter((event) => event.type === 'run.failed'), [{
    type: 'run.failed',
    code: 'runtime.tool_call_budget_exceeded',
    message: 'tool budget exhausted',
  }])
})

test('maps a successful context compaction without creating assistant text', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'compaction_start', reason: 'threshold' })
  mapper.handle({ type: 'compaction_end', reason: 'threshold', aborted: false, willRetry: false, result: { summary: 'ok' } })

  assert.deepEqual(events.map((event) => event.type), [
    'context.compaction.started',
    'run.phase',
    'context.compaction.completed',
    'run.phase',
  ])
  assert.equal(events.some((event) => event.type.startsWith('message.')), false)
})

test('T1.7 maps a compaction error to context.compaction.failed only', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'compaction_start', reason: 'threshold' })
  mapper.handle({
    type: 'compaction_end',
    reason: 'threshold',
    aborted: false,
    willRetry: true,
    errorMessage: 'summary hit the token cap',
  })
  mapper.handle({ type: 'compaction_end', reason: 'overflow', aborted: false, willRetry: false })

  assert.deepEqual(events.map((event) => event.type), [
    'context.compaction.started',
    'run.phase',
    'context.compaction.failed',
    'context.compaction.failed',
  ])
  assert.equal(events.filter((event) => event.type === 'context.compaction.completed').length, 0)
  assert.match(events[2].errorMessage, /summary hit the token cap/)
  assert.equal(events[3].errorMessage, 'Context compaction completed without a result.')
})

test('T1.1 final length-only assistant fails with model.output_truncated', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'thinking', thinking: 'the model was cut off' }],
      stopReason: 'length',
      usage: {},
    },
  })
  mapper.handle({ type: 'agent_end', messages: [] })

  assert.deepEqual(events.filter((event) => event.type === 'run.failed'), [{
    type: 'run.failed',
    code: 'model.output_truncated',
    message: 'The model output was truncated before a final answer.',
  }])
  assert.equal(events.filter((event) => event.type === 'run.completed').length, 0)
})

test('T1.2 final thinking-only stop fails with model.empty_response', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'thinking', thinking: 'private planning only' }],
      stopReason: 'stop',
      usage: {},
    },
  })
  mapper.handle({ type: 'agent_end', messages: [] })

  assert.deepEqual(events.filter((event) => event.type === 'run.failed'), [{
    type: 'run.failed',
    code: 'model.empty_response',
    message: 'The model returned no visible answer.',
  }])
})

test('T1.3 an intermediate length does not fail before the SDK recovery answer', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'thinking', thinking: 'first attempt hit the cap' }],
      stopReason: 'length',
      usage: {},
    },
  })
  mapper.handle({ type: 'agent_end', messages: [], willRetry: true })
  mapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'text', text: 'recovered answer' }],
      stopReason: 'stop',
      usage: {},
    },
  })
  mapper.handle({ type: 'agent_end', messages: [], willRetry: false })

  assert.equal(events.filter((event) => event.type === 'run.failed').length, 0)
  assert.equal(events.filter((event) => event.type === 'run.completed').length, 1)
})

test('T1.4 tool turns stay completed while a final text answer is required for empty turns', () => {
  const toolEvents = []
  const toolMapper = createPiEventMapper((type, payload = {}) => toolEvents.push({ type, ...payload }))
  toolMapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'toolCall', id: 'call-1', name: 'read', arguments: { path: 'a.txt' } }],
      stopReason: 'toolUse',
      usage: {},
    },
  })
  toolMapper.handle({ type: 'agent_end', messages: [] })
  assert.equal(toolEvents.filter((event) => event.type === 'run.completed').length, 1)

  const finalEvents = []
  const finalMapper = createPiEventMapper((type, payload = {}) => finalEvents.push({ type, ...payload }))
  finalMapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'toolCall', id: 'call-2', name: 'read', arguments: { path: 'b.txt' } }],
      stopReason: 'toolUse',
      usage: {},
    },
  })
  finalMapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'text', text: 'final answer' }],
      stopReason: 'stop',
      usage: {},
    },
  })
  finalMapper.handle({ type: 'agent_end', messages: [] })
  assert.equal(finalEvents.filter((event) => event.type === 'run.completed').length, 1)
  assert.equal(finalEvents.filter((event) => event.type === 'run.failed').length, 0)
})

test('T1.5 cancellation keeps priority over a length-only final assistant', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [{ type: 'thinking', thinking: 'cancelled' }],
      stopReason: 'length',
      usage: {},
    },
  })
  mapper.cancel()
  mapper.handle({ type: 'agent_end', messages: [] })

  assert.equal(events.filter((event) => event.type === 'run.cancelled').length, 1)
  assert.equal(events.filter((event) => event.type === 'run.failed').length, 0)
})

test('T1.6 deferCompletion finish() derives the same terminal as agent_end', () => {
  const run = (message, events) => {
    const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }), { deferCompletion: true })
    mapper.handle({ type: 'message_end', message })
    mapper.handle({ type: 'agent_end', messages: [] })
    return mapper
  }

  const truncated = []
  run({ role: 'assistant', content: [{ type: 'thinking', thinking: 'cut off' }], stopReason: 'length', usage: {} }, truncated).finish()
  assert.equal(truncated.find((event) => event.type === 'run.failed')?.code, 'model.output_truncated')

  const empty = []
  run({ role: 'assistant', content: [{ type: 'thinking', thinking: 'no answer' }], stopReason: 'stop', usage: {} }, empty).finish()
  assert.equal(empty.find((event) => event.type === 'run.failed')?.code, 'model.empty_response')

  const completed = []
  run({ role: 'assistant', content: [{ type: 'text', text: 'answer' }], stopReason: 'stop', usage: {} }, completed).finish()
  assert.equal(completed.filter((event) => event.type === 'run.completed').length, 1)
  assert.equal(completed.filter((event) => event.type === 'run.failed').length, 0)
})

test('T1.6b deferCompletion keeps one message open across a compaction retry', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }), { deferCompletion: true })
  const assistantMessage = (text) => ({
    role: 'assistant',
    content: [{ type: 'text', text }],
    stopReason: 'stop',
    usage: {},
  })

  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'first' } })
  mapper.handle({ type: 'message_end', message: assistantMessage('first') })
  mapper.handle({ type: 'agent_end', messages: [] })
  assert.equal(events.filter((event) => event.type === 'message.completed').length, 0,
    'agent_end must not close the message while deferCompletion is active')

  mapper.handle({ type: 'compaction_start', reason: 'overflow' })
  mapper.handle({ type: 'compaction_end', reason: 'overflow', result: {}, willRetry: true })
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'second' } })
  mapper.handle({ type: 'message_end', message: assistantMessage('second') })
  mapper.handle({ type: 'agent_end', messages: [] })
  assert.equal(events.filter((event) => event.type === 'message.completed').length, 0,
    'the retry must still be able to stream into the same assistant message')

  mapper.finish()
  assert.equal(events.filter((event) => event.type === 'message.started').length, 1,
    'the Host message id is fixed, so the mapper must not start a second message')
  assert.equal(events.filter((event) => event.type === 'message.completed').length, 1)
  const completedAt = events.findIndex((event) => event.type === 'message.completed')
  const terminalAt = events.findIndex((event) => event.type === 'run.completed')
  assert.ok(completedAt >= 0 && completedAt < terminalAt, 'message completion must precede Run completion')
})

test('T1.6c compaction failure still closes the message and Run once', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }), { deferCompletion: true })
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'answer' } })
  mapper.handle({ type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text: 'answer' }], stopReason: 'stop', usage: {} } })
  mapper.handle({ type: 'compaction_start', reason: 'overflow' })
  mapper.handle({ type: 'compaction_end', reason: 'overflow', errorMessage: 'summary failed' })
  mapper.handle({ type: 'agent_end', messages: [] })
  mapper.finish()
  assert.equal(events.filter((event) => event.type === 'context.compaction.failed').length, 1)
  assert.equal(events.filter((event) => event.type === 'message.completed').length, 1)
  assert.equal(events.filter((event) => event.type === 'run.completed').length, 1)
  assert.equal(events.filter((event) => event.type === 'run.failed').length, 0)
})

test('T1.6d cancellation during compaction owns the single terminal', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }), { deferCompletion: true })
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'compaction_start', reason: 'overflow' })
  mapper.cancel()
  mapper.handle({ type: 'compaction_end', reason: 'overflow', aborted: true })
  mapper.handle({ type: 'agent_end', messages: [] })
  mapper.finish()
  assert.equal(events.filter((event) => event.type === 'run.cancelled').length, 1)
  assert.equal(events.filter((event) => event.type === 'run.completed').length, 0)
  assert.equal(events.filter((event) => event.type === 'run.failed').length, 0)
  assert.equal(events.filter((event) => event.type === 'message.completed').length, 0,
    'Host cancellation closes the in-flight message; the mapper must not complete it twice')
})

test('projects knowledge search results into stable source events', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload) => events.push({ type, ...payload }))
  mapper.handle({
    type: 'tool_execution_end',
    toolCallId: 'knowledge-1',
    toolName: 'search_knowledge',
    result: {
      details: {
        kb_id: 'kb-1',
        results: [{
          id: 'chunk-1',
          kb_id: 'kb-1',
          file_id: 'document-1',
          content: 'Fox knowledge source excerpt.',
          metadata: { source: 'architecture.md', chunk_id: 'chunk-1', page_number: 7, section: 'Runtime boundary' },
        }],
      },
    },
    isError: false,
  })
  const source = events.find((event) => event.type === 'source.added')
  assert.deepEqual(source, {
    type: 'source.added',
    id: 'chunk-1',
    title: 'architecture.md',
    knowledgeBaseId: 'kb-1',
    documentId: 'document-1',
    excerpt: 'Fox knowledge source excerpt.',
    chunkId: 'chunk-1',
    page: 7,
    anchor: 'Runtime boundary',
    metadata: { source: 'architecture.md', chunk_id: 'chunk-1', page_number: 7, section: 'Runtime boundary' },
  })
})

test('projects Host local search and document chunks to the same clickable source', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload) => events.push({ type, ...payload }))
  const chunk = { documentId: 'doc-a', chunkId: 'chunk-a', source: 'local', anchor: 'bytes:0-657', content: 'A-7 验收押金 37,500 元', sourceMetadata: { knowledgeBaseId: 'kb-a', displayName: '城北项目.md', pageNumber: null, startOffset: 0, endOffset: 657 } }
  mapper.handle({ type: 'tool_execution_end', toolCallId: 'search', toolName: 'search_knowledge', result: { details: { source: 'local', knowledgeBaseId: 'kb-a', items: [null, chunk] } } })
  mapper.handle({ type: 'tool_execution_end', toolCallId: 'read', toolName: 'read_knowledge_document', result: { details: { reference: { id: 'kb-a', source: 'local' }, result: { documentId: 'doc-a', chunks: [chunk] } } } })
  const sources = events.filter(event => event.type === 'source.added')
  assert.equal(sources.length, 2)
  assert.deepEqual(sources[0], sources[1])
  assert.equal(sources[0].source, 'local')
  assert.equal(sources[0].knowledgeBaseId, 'kb-a')
  assert.equal(sources[0].documentId, 'doc-a')
  assert.equal(sources[0].chunkId, 'chunk-a')
  assert.equal(sources[0].anchor, 'bytes:0-657')
  assert.equal(sources[0].title, '城北项目.md')
})

test('projects mixed local and remote Host result envelopes without mixing targets', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload) => events.push({ type, ...payload }))
  mapper.handle({ type: 'tool_execution_end', toolCallId: 'search', toolName: 'search_knowledge', result: { details: { items: [
    { reference: { id: 'kb-local', source: 'local' }, result: { items: [{ documentId: 'local-doc', chunkId: 'local-chunk', content: '本地内容', sourceMetadata: { displayName: '本地.md' } }] } },
    { reference: { id: 'kb-remote', source: 'remote' }, result: { results: [{ id: 'remote-chunk', file_id: 'remote-doc', content: '远程内容', metadata: { source: '远程.md' } }] } },
  ] } } })
  const sources = events.filter(event => event.type === 'source.added')
  assert.deepEqual(sources.map(({ source, knowledgeBaseId, documentId }) => [source, knowledgeBaseId, documentId]), [
    ['local', 'kb-local', 'local-doc'], ['remote', 'kb-remote', 'remote-doc'],
  ])
})

test('failed and non-document knowledge results do not advertise citations', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload) => events.push({ type, ...payload }))
  mapper.handle({ type: 'tool_execution_end', toolCallId: 'failed', toolName: 'search_knowledge', isError: true, result: { details: { kb_id: 'kb', results: [{ file_id: 'doc', content: 'partial' }] } } })
  mapper.handle({ type: 'tool_execution_end', toolCallId: 'invalid', toolName: 'search_knowledge', result: { details: { items: [{ id: 'not-a-document', name: 'A' }] } } })
  assert.equal(events.filter(event => event.type === 'source.added').length, 0)
})

const MD1_ZH_ANSWER = '我需要先提醒你三点注意事项：\n1. 备份数据库。\n现在需要确认的是磁盘空间是否足够。\n最后重启服务即可。'

test('MD-1 an ordinary Chinese answer that opens with planning-like words stays the answer', () => {
  assert.deepEqual(splitMixedAssistantText(MD1_ZH_ANSWER), { reasoning: '', answer: MD1_ZH_ANSWER })
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  for (const chunk of MD1_ZH_ANSWER.match(/[\s\S]{1,7}/g)) {
    mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: chunk } })
  }
  mapper.handle({ type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text: MD1_ZH_ANSWER }], stopReason: 'stop', usage: {} } })
  mapper.handle({ type: 'agent_end', messages: [] })
  assert.equal(events.some((event) => event.type === 'reasoning.delta'), false)
  assert.equal(events.filter((event) => event.type === 'message.delta').map((event) => event.delta).join(''), MD1_ZH_ANSWER)
  assert.equal(events.some((event) => event.type === 'run.failed'), false)
  assert.equal(events.filter((event) => event.type === 'run.completed').length, 1)
})

test('MD-1 English planning-like openings are not reclassified as reasoning either', () => {
  const text = 'The user wants a CSV.\n\nLet me inspect the files.\n\nWe need to check the header row.'
  assert.deepEqual(splitMixedAssistantText(text), { reasoning: '', answer: text })
  const messages = sanitizeAssistantHistory([{ role: 'assistant', content: [{ type: 'text', text }], stopReason: 'stop' }])
  assert.deepEqual(messages[0].content, [{ type: 'text', text }])
})

test('MD-1 streams explicit provider thinking tags as reasoning, split across chunks', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  for (const delta of ['<thi', 'nk>我先看看目录', '。</th', 'ink>\n\n', '结果如下。']) {
    mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta } })
  }
  const text = '<think>我先看看目录。</think>\n\n结果如下。'
  mapper.handle({ type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text }], stopReason: 'stop', usage: {} } })
  assert.equal(events.filter((event) => event.type === 'reasoning.delta').map((event) => event.delta).join(''), '我先看看目录。')
  assert.equal(events.filter((event) => event.type === 'message.delta').map((event) => event.delta).join(''), '结果如下。')
})

test('MD-1 text before a tool call that was not tagged is kept as visible text', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: '我先检查目录。' } })
  mapper.handle({ type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text: '我先检查目录。' }, { type: 'toolCall', id: 't1', name: 'ls', arguments: {} }], stopReason: 'toolUse', usage: {} } })
  assert.equal(events.filter((event) => event.type === 'message.delta').map((event) => event.delta).join(''), '我先检查目录。')
  assert.equal(events.some((event) => event.type === 'reasoning.delta'), false)
})
