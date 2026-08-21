import assert from 'node:assert/strict'
import test from 'node:test'

import MessageProcessor from '../src/utils/messageProcessor.js'
import { getConversationDisplayItems } from '../src/utils/messageGrouping.js'
import { toElementsMessages } from '../src/utils/chat/elementsAdapter.js'
import { useStreamSmoother } from '../src/hooks/agent/useStreamSmoother.js'
import { normalizeKbListResult } from '../src/components/ToolCallingResult/tools/listKbsResult.js'
import { resolveRetryUserMessage } from '../src/utils/chat/retryMessage.js'

test('reads streamed reasoning fields and removes reasoning tags from the answer', () => {
  const parsed = MessageProcessor.parseAssistantMessageBody({
    content: '<think>检查知识库范围</think>最终答案',
    reasoning_content: '识别用户问题'
  })

  assert.equal(parsed.content, '最终答案')
  assert.equal(parsed.reasoningContent, '识别用户问题\n\n检查知识库范围')
})

test('groups all assistant messages in one user turn while preserving part order', () => {
  const toolCalls = [{ id: 'call-1', name: 'query_kb', args: { query: '轨道吊起升赋值' } }]
  const items = getConversationDisplayItems(
    {
      messages: [
        { id: 'user-1', type: 'human', content: '轨道吊起升如何赋值？' },
        {
          id: 'assistant-1',
          type: 'ai',
          content: '我先查询相关知识库。',
          reasoning_content: '判断需要检索设备知识',
          tool_calls: toolCalls
        },
        {
          id: 'assistant-2',
          type: 'ai',
          content: '根据资料，起升机构主要包含两类赋值操作。'
        }
      ]
    },
    { enrichToolCalls: (message) => message.tool_calls || [] }
  )

  assert.equal(items.length, 2)
  assert.equal(items[0].type, 'message')
  assert.equal(items[1].type, 'assistant-turn')
  assert.equal(items[1].messages.length, 2)
  assert.deepEqual(
    items[1].parts.map((part) => part.type),
    ['reasoning', 'text', 'tool-group', 'text']
  )
})

test('passes one ordered assistant turn to AI Elements', () => {
  const [message] = toElementsMessages([
    {
      key: 'assistant-turn-1',
      placement: 'start',
      _parts: [
        { type: 'reasoning', payload: { reasoning: '先检索知识库' } },
        { type: 'text', payload: { content: '正在查询。' } },
        { type: 'tool-group', payload: { toolCalls: [{ id: 'call-1' }] } },
        { type: 'text', payload: { content: '这是最终答复。' } }
      ],
      _sender: '智能助手'
    }
  ])

  assert.equal(message.role, 'assistant')
  assert.deepEqual(
    message.parts.map((part) => part.type),
    ['reasoning', 'text', 'tool-group', 'text']
  )
})

test('preserves tool results already attached by the history endpoint', () => {
  const content = JSON.stringify([
    { kb_id: 'kb-1', name: '轨道吊赋值限速知识库', description: '轨道吊操作手册' }
  ])
  const [message] = MessageProcessor.convertToolResultToMessages([
    {
      id: 'assistant-1',
      type: 'ai',
      tool_calls: [
        {
          id: 'call-1',
          name: 'list_kbs',
          status: 'success',
          tool_call_result: { content }
        }
      ]
    }
  ])

  assert.equal(message.tool_calls[0].tool_call_result.content, content)
  assert.equal(normalizeKbListResult(content)[0].name, '轨道吊赋值限速知识库')
})

test('materializes a tool call from the first streamed chunk', () => {
  const message = MessageProcessor.mergeMessageChunk([
    {
      id: 'assistant-stream-1',
      type: 'AIMessageChunk',
      content: '',
      tool_call_chunks: [{ index: 0, id: 'call-list-kbs', name: 'list_kbs', args: '' }]
    }
  ])

  assert.equal(message.type, 'ai')
  assert.equal(message.tool_calls[0].id, 'call-list-kbs')
  assert.equal(message.tool_calls[0].function.name, 'list_kbs')
})

test('emits tool call metadata immediately without waiting for the text smoother', () => {
  const threadState = { onGoingConv: { msgChunks: {} } }
  const smoother = useStreamSmoother({ getThreadState: () => threadState })

  smoother.pushChunk(
    {
      id: 'assistant-stream-2',
      type: 'AIMessageChunk',
      content: '我先查询知识库。',
      tool_call_chunks: [
        {
          index: 0,
          id: 'call-query-kb',
          name: 'query_kb',
          args: '{"query_text":"起升赋值"}'
        }
      ]
    },
    'thread-1'
  )

  const firstChunk = threadState.onGoingConv.msgChunks['assistant-stream-2'][0]
  assert.equal(firstChunk.content, '')
  assert.equal(firstChunk.tool_call_chunks[0].name, 'query_kb')
  assert.equal(firstChunk.tool_call_chunks[0].args, '{"query_text":"起升赋值"}')

  smoother.resetThread('thread-1')
})

test('normalizes wrapped knowledge-base tool results', () => {
  const wrapped = [
    {
      type: 'text',
      text: JSON.stringify({
        data: {
          items: [{ id: 'kb-2', kb_name: '安全规程知识库', desc: '安全操作边界' }]
        }
      })
    }
  ]

  assert.deepEqual(normalizeKbListResult(wrapped), [
    {
      id: 'kb-2',
      kb_name: '安全规程知识库',
      desc: '安全操作边界',
      kb_id: 'kb-2',
      name: '安全规程知识库',
      description: '安全操作边界'
    }
  ])
})

test('resolves retry content from an assistant turn conversation', () => {
  const source = resolveRetryUserMessage({
    target: { key: 'assistant-turn-1', _convIdx: 0 },
    conversations: [
      {
        messages: [
          { id: 'user-1', type: 'human', content: '如何设置轨道吊起升参数？' },
          { id: 'assistant-1', type: 'ai', content: '让我查询知识库。' }
        ]
      }
    ]
  })

  assert.equal(source.content, '如何设置轨道吊起升参数？')
  assert.equal(source.message.id, 'user-1')
})

test('falls back to the previous user bubble when retry conversation metadata is missing', () => {
  const source = resolveRetryUserMessage({
    target: { key: 'assistant-turn-2' },
    bubbleItems: [
      { key: 'user-2', placement: 'end', content: '查询故障码 E101' },
      { key: 'assistant-turn-2', placement: 'start', content: '正在查询。' }
    ]
  })

  assert.equal(source.content, '查询故障码 E101')
})
