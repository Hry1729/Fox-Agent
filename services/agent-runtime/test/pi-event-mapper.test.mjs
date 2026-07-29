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
  mapper.handle({ type: 'message_end', message: { role: 'assistant', content: [{ type: 'text', text: 'hello' }], stopReason: 'stop', usage: { input: 2, output: 3, totalTokens: 5 } } })
  mapper.handle({ type: 'agent_end', messages: [] })
  assert.deepEqual(events.map((event) => event.type), [
    'message.started', 'message.delta', 'reasoning.delta', 'tool.started', 'tool.completed',
    'usage.updated', 'message.completed', 'run.completed',
  ])
  assert.equal(events.find((event) => event.type === 'reasoning.delta')?.providerField, 'reasoning_content')
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

test('maps Pi errors and aborts to terminal Fox events only once', () => {
  const events = []
  const mapper = createPiEventMapper((type, payload = {}) => events.push({ type, ...payload }))
  mapper.handle({ type: 'message_end', message: { role: 'assistant', stopReason: 'aborted', usage: {} } })
  mapper.handle({ type: 'agent_end', messages: [] })
  assert.equal(events.filter((event) => event.type === 'run.cancelled').length, 1)
  assert.equal(events.filter((event) => event.type === 'run.completed').length, 0)
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
