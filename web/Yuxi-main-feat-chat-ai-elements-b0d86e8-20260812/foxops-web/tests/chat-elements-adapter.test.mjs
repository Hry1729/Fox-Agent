import assert from 'node:assert/strict'
import test from 'node:test'

import { toElementsMessages } from '../src/utils/chat/elementsAdapter.js'

test('maps user text and assistant reasoning', () => {
  const views = toElementsMessages([
    { key: 'u1', placement: 'end', content: '你好', _sender: '我' },
    { key: 'a1', placement: 'start', content: '答案', _reasoning: '思考中', _sender: 'Bot' }
  ])

  assert.equal(views.length, 2)
  assert.equal(views[0].id, 'u1')
  assert.equal(views[0].role, 'user')
  assert.deepEqual(
    views[0].parts.map((part) => part.type),
    ['text']
  )
  assert.equal(views[0].parts[0].payload.content, '你好')

  assert.equal(views[1].id, 'a1')
  assert.equal(views[1].role, 'assistant')
  assert.deepEqual(
    views[1].parts.map((part) => part.type),
    ['reasoning', 'text']
  )
  assert.equal(views[1].parts[0].payload.reasoning, '思考中')
  assert.equal(views[1].parts[1].payload.content, '答案')
})

test('maps tool-group and extra assistant parts', () => {
  const toolCalls = [{ id: 'call-1', toolName: 'search' }]
  const sources = [{ id: 'source-1', title: '文档' }]
  const artifacts = [{ id: 'artifact-1', name: 'report.md' }]
  const attachments = [{ id: 'file-1', name: '附件.txt' }]

  const [view] = toElementsMessages([
    {
      key: 'a2',
      placement: 'start',
      content: '',
      _type: 'tool-group',
      _toolCalls: toolCalls,
      _sources: sources,
      _artifacts: artifacts,
      _sender: 'Bot',
      _time: '10:00',
      _avatar: '/bot.png',
      _modelName: 'gpt-test',
      _msgId: 'msg-2',
      _feedback: { score: 1 },
      _userAttachments: attachments,
      _imageContent: 'base64-image',
      _imageMime: 'image/png',
      _errorType: 'tool_error',
      _errorMessage: '工具失败',
      _isStoppedByUser: true
    }
  ])

  assert.equal(view.role, 'assistant')
  assert.deepEqual(
    view.parts.map((part) => part.type),
    ['tool-group', 'sources', 'artifacts', 'attachments', 'image', 'error']
  )
  assert.equal(view.parts[0].payload.toolCalls, toolCalls)
  assert.equal(view.parts[1].payload.sources, sources)
  assert.equal(view.parts[2].payload.artifacts, artifacts)
  assert.equal(view.parts[3].payload.attachments, attachments)
  assert.equal(view.parts[4].payload.content, 'base64-image')
  assert.equal(view.parts[4].payload.mime, 'image/png')
  assert.equal(view.parts[5].payload.errorType, 'tool_error')
  assert.equal(view.parts[5].payload.errorMessage, '工具失败')
  assert.equal(view.parts[5].payload.isStoppedByUser, true)
  assert.deepEqual(view.meta, {
    sender: 'Bot',
    time: '10:00',
    avatar: '/bot.png',
    modelName: 'gpt-test',
    msgId: 'msg-2',
    feedback: { score: 1 }
  })
})

test('returns empty list for empty input', () => {
  assert.deepEqual(toElementsMessages([]), [])
})
