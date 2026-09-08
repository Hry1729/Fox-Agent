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

test('moves text from tool-calling turns into the reasoning stream', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: 'Let me inspect the directory first.' } })
  mapper.handle({
    type: 'message_end',
    message: {
      role: 'assistant',
      content: [
        { type: 'text', text: 'Let me inspect the directory first.' },
        { type: 'toolCall', id: 'tool-1', name: 'ls', arguments: { path: '.' } },
      ],
      stopReason: 'toolUse',
      usage: {},
    },
  })

  assert.equal(events.find((event) => event.type === 'reasoning.delta')?.delta, 'Let me inspect the directory first.')
  assert.equal(events.some((event) => event.type === 'message.delta'), false)
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

test('separates a strongly signalled planning preamble from the final answer', () => {
  const text = [
    'The user wants a CSV file. I need to inspect the directory first.',
    '',
    'Let me determine the safest implementation and check the available files.',
    '',
    '好的，脚本已经创建完成，运行后会生成文件统计.csv。',
  ].join('\n')
  const split = splitMixedAssistantText(text)
  assert.match(split.reasoning, /^The user wants/)
  assert.equal(split.answer, '好的，脚本已经创建完成，运行后会生成文件统计.csv。')
})

test('separates Chinese process narration from a concise final answer', () => {
  const text = [
    '我先确认一下当前的工作目录，然后看看项目里到底有哪些文档。',
    '',
    '现在去验证文档里的具体数字和事实声明。',
    '',
    '让我再快速核实最后两个容易出错的点。',
    '',
    '结论如下：README 中的 legacy 目录声明与实际文件系统不一致。',
  ].join('\n')

  const split = splitMixedAssistantText(text)
  assert.match(split.reasoning, /^我先确认一下当前的工作目录/)
  assert.equal(split.answer, '结论如下：README 中的 legacy 目录声明与实际文件系统不一致。')
})

test('keeps a planning-only Chinese turn out of the assistant answer', () => {
  const text = [
    '我来建一个目标，然后拆成可执行的任务。',
    '',
    '先看看当前有没有已激活的目标状态。',
    '',
    '当前对话没有现存的目标快照，那我直接新建一个。',
    '',
    'Gate 仍然要求确认，我需要再检查目标状态。',
  ].join('\n')

  assert.deepEqual(splitMixedAssistantText(text), { reasoning: text, answer: '' })
})

test('keeps Goal gate and Host contract narration out of the assistant answer', () => {
  const text = [
    '好的，本次测试要测的是目标验收：部分子任务完成、部分未完成时，结案目标是否会报错。',
    '',
    '测试设计：创建三个任务，完成前两个，把第三个保持 queued。',
    '',
    '先提议目标并等待 Host 确认。目标已提议，状态 proposed，等待 Host 确认。',
    '',
    '快照里仍是 goal: null，我按守则停在这里。',
    '',
    '我直接调用 task_create_many，让 Host 来判定。',
  ].join('\n')

  assert.deepEqual(splitMixedAssistantText(text), { reasoning: text, answer: '' })
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

test('keeps a planning-only final turn out of the assistant answer', () => {
  const text = [
    'Now I have a clear view of the directory. Let me write a Python script.',
    '',
    'Actually, I do not have a tool to execute scripts.',
    '',
    'Wait, let me re-read the task. I should create the script file.',
  ].join('\n')
  assert.deepEqual(splitMixedAssistantText(text), { reasoning: text, answer: '' })
})

test('maps mixed final text into reasoning and answer streams', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  const text = 'The user asked for a script.\n\nLet me verify the output format.\n\n好的，代码已经写好。'
  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: text } })
  mapper.handle({
    type: 'message_end',
    message: { role: 'assistant', content: [{ type: 'text', text }], stopReason: 'stop', usage: {} },
  })

  assert.match(events.find((event) => event.type === 'reasoning.delta')?.delta, /^The user asked/)
  assert.equal(events.find((event) => event.type === 'message.delta')?.delta, '好的，代码已经写好。')
})

test('streams Chinese planning as reasoning before its final answer', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  const planning = '我先确认当前目录。\n\n现在去核对项目文件。\n\n让我再验证一次。\n\n'
  const answer = '最终结果：项目结构已经核对完成。'

  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: planning } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: answer } })
  mapper.handle({
    type: 'message_end',
    message: { role: 'assistant', content: [{ type: 'text', text: planning + answer }], stopReason: 'stop', usage: {} },
  })

  assert.equal(events.filter((event) => event.type === 'message.delta').map((event) => event.delta).join(''), answer)
  assert.match(events.filter((event) => event.type === 'reasoning.delta').map((event) => event.delta).join(''), /^我先确认当前目录/)
})

test('keeps a tool-testing planning list out of the final answer', () => {
  const text = [
    'This is the **third round** of tool testing. Let me try some more creative and different tool calls to demonstrate variety.',
    '',
    "What's interesting to try:",
    '',
    'Grep with different patterns',
    'Find with different patterns',
    'Try to read with offsets',
    '',
    'Let me think about what would be novel:',
    '',
    'Try grep with case-sensitive vs case-insensitive',
    'Try read with offset to get middle of CSV file',
    '',
    'Let me do a parallel batch of calls.',
    '',
    'Let me make calls好嘞！第三轮工具测试开始，这次试试**不同写法**和**参数边界**：',
  ].join('\n')

  const split = splitMixedAssistantText(text)
  assert.match(split.reasoning, /^This is the \*\*third round\*\*/)
  assert.equal(split.answer, '好嘞！第三轮工具测试开始，这次试试**不同写法**和**参数边界**：')
})

test('streams a tool-testing planning list as reasoning before its Chinese answer', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  const planning = 'This is the third round of tool testing. Let me try more calls.\n\nLet me inspect different patterns.\n\n'
  const answer = '好嘞！第三轮工具测试开始。'

  mapper.handle({ type: 'message_start', message: { role: 'assistant', content: [] } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: planning } })
  mapper.handle({ type: 'message_update', assistantMessageEvent: { type: 'text_delta', delta: answer } })
  mapper.handle({
    type: 'message_end',
    message: { role: 'assistant', content: [{ type: 'text', text: planning + answer }], stopReason: 'stop', usage: {} },
  })

  assert.equal(events.filter((event) => event.type === 'message.delta').map((event) => event.delta).join(''), answer)
  assert.match(events.filter((event) => event.type === 'reasoning.delta').map((event) => event.delta).join(''), /^This is the third round/)
})

test('moves leaked planning into thinking blocks before session persistence', () => {
  const text = 'The user wants a CSV.\n\nLet me inspect the files.\n\n好的，脚本已经完成。'
  const messages = sanitizeAssistantHistory([{
    role: 'assistant',
    content: [{ type: 'text', text }],
    stopReason: 'stop',
  }])

  assert.deepEqual(messages[0].content.map((block) => block.type), ['thinking', 'text'])
  assert.equal(messages[0].content[1].text, '好的，脚本已经完成。')
})

test('sanitizes leaked planning from legacy string history', () => {
  const text = 'The user wants a CSV.\n\nLet me inspect the files.\n\n好的，脚本已经完成。'
  const messages = sanitizeAssistantHistory([{ role: 'assistant', content: text }])

  assert.deepEqual(messages[0].content.map((block) => block.type), ['thinking', 'text'])
  assert.equal(messages[0].content[1].text, '好的，脚本已经完成。')
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

test('maps context compaction without creating assistant text', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'compaction_start', reason: 'threshold' })
  mapper.handle({ type: 'compaction_end', reason: 'threshold', aborted: false, willRetry: false })

  assert.deepEqual(events.map((event) => event.type), [
    'context.compaction.started',
    'run.phase',
    'context.compaction.completed',
    'run.phase',
  ])
  assert.equal(events.some((event) => event.type.startsWith('message.')), false)
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
