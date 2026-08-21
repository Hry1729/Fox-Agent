import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, readdir, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { createSessionState, loadSessionState, sanitizeProviderHistory, saveSessionState, transcriptFromSession } from '../src/runtime-session.mjs'

test('persists and restores Fox-owned Pi session state', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-session-'))
  context.after(() => rm(directory, { recursive: true, force: true }))
  const path = join(directory, 'session.json')
  const session = createSessionState('session-1', 'conversation-1')
  session.messages = [{ role: 'user', content: 'hello', timestamp: Date.now() }]
  await saveSessionState(path, session)
  const restored = await loadSessionState(path, 'session-1')
  assert.equal(restored.messages[0].content, 'hello')
  assert.deepEqual(await readdir(directory), ['session.json'])
})

test('strips provider-only fields before replaying history', () => {
  const messages = sanitizeProviderHistory([{
    role: 'assistant',
    namespace: 'provider-private',
    api: 'anthropic-messages',
    provider: 'minimax',
    content: [
      { type: 'thinking', thinking: 'plan', thinkingSignature: 'sig', namespace: 'private' },
      { type: 'toolCall', id: 'call-1', name: 'read', input: { path: 'a.txt' }, namespace: 'private' },
      { type: 'text', text: 'done', arbitrary: true },
    ],
    timestamp: 1,
  }, {
    role: 'toolResult',
    toolCallId: 'call-1',
    toolName: 'read',
    content: [{ type: 'text', text: 'body', namespace: 'private' }],
    namespace: 'provider-private',
    details: { path: 'a.txt', namespace: 'private' },
    isError: false,
    timestamp: 2,
  }])

  assert.equal(JSON.stringify(messages).includes('namespace'), false)
  assert.deepEqual(messages[0].content[1], {
    type: 'toolCall', id: 'call-1', name: 'read', arguments: { path: 'a.txt' },
  })
  assert.deepEqual(messages[1].content, [{ type: 'text', text: 'body' }])
  assert.deepEqual(messages[1].details, {})
})

test('uses Pi content blocks when replaying a legacy assistant string', () => {
  const messages = sanitizeProviderHistory([
    { role: 'user', content: '同意目标', timestamp: 1 },
    { role: 'assistant', content: '目标已提交，等待确认。', timestamp: 2 },
  ])

  assert.equal(messages[0].content, '同意目标')
  assert.deepEqual(messages[1].content, [{ type: 'text', text: '目标已提交，等待确认。' }])
})

test('falls back to SQLite history when a runtime transcript is empty', () => {
  const messages = transcriptFromSession({ messages: [] }, [{ role: 'assistant', content: 'restored reply' }])
  assert.equal(messages[0].content, 'restored reply')
})

test('prefers SQLite truth over a stale runtime transcript', () => {
  const messages = transcriptFromSession(
    { messages: [{ role: 'assistant', content: 'stale reply' }] },
    [{ role: 'assistant', content: 'database reply' }],
  )
  assert.equal(messages[0].content, 'database reply')
})

test('keeps Pi tool context when its text projection matches SQLite', () => {
  const toolResult = { role: 'toolResult', toolCallId: '1', toolName: 'read', content: [{ type: 'text', text: 'file body' }], details: {}, isError: false, timestamp: 1 }
  const assistant = { role: 'assistant', content: [{ type: 'text', text: 'final reply' }], api: 'faux', provider: 'faux', model: 'test', usage: {}, stopReason: 'stop', timestamp: 2 }
  const messages = transcriptFromSession(
    { messages: [{ role: 'user', content: 'read it', timestamp: 0 }, toolResult, assistant] },
    [{ role: 'user', content: 'read it' }, { role: 'assistant', content: 'final reply' }],
  )
  assert.equal(messages.length, 3)
  assert.equal(messages[1].role, 'toolResult')
})
