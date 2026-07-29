import test from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { createServer } from 'node:http'
import { mkdtemp, rm } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'

const runtimePath = fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url))

function startRuntime() {
  const child = spawn(process.execPath, [runtimePath], { stdio: ['pipe', 'pipe', 'pipe'] })
  const messages = []
  const waiters = []
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
  output.on('line', (line) => {
    messages.push(JSON.parse(line))
    for (const waiter of [...waiters]) waiter()
  })
  return {
    child,
    send(type, fields = {}) {
      const request = createEnvelope('request', type, fields)
      child.stdin.write(`${JSON.stringify(request)}\n`)
      return request
    },
    async waitFor(predicate, timeout = 4_000) {
      const existing = messages.find(predicate)
      if (existing) return existing
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error(`timed out: ${JSON.stringify(messages)}`)), timeout)
        const check = () => {
          const message = messages.find(predicate)
          if (!message) return
          clearTimeout(timer)
          resolve(message)
        }
        waiters.push(check)
      })
    },
    close() { output.close(); child.kill() },
  }
}

test('runs the real Pi Agent loop through Fox JSONL with a faux provider', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-runtime-'))
  context.after(() => rm(directory, { recursive: true, force: true }))
  const runtime = startRuntime()
  context.after(() => runtime.close())

  const initialize = runtime.send('initialize', {
    payload: { modelService: { baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 4096, maxOutputTokens: 512 } },
  })
  const ready = await runtime.waitFor((message) => message.requestId === initialize.id)
  assert.equal(ready.type, 'ready')
  assert.equal(ready.payload.runtime, 'fox-pi-runtime')

  const sessionId = 'pi-session-1'
  const conversationId = 'conversation-1'
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', { conversationId, runtimeSessionId: sessionId, payload: { sessionPath } })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  runtime.send('prompt', {
    conversationId,
    runtimeSessionId: sessionId,
    runId: 'run-1',
    payload: { text: 'hello Pi', messages: [{ role: 'user', content: 'hello Pi' }] },
  })
  await runtime.waitFor((message) => message.runId === 'run-1' && message.payload?.type === 'message.delta')
  await runtime.waitFor((message) => message.runId === 'run-1' && message.payload?.type === 'run.completed')
})

test('maps Anthropic thinking blocks separately from the final answer', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-anthropic-'))
  context.after(() => rm(directory, { recursive: true, force: true }))
  const server = createServer((request, response) => {
    assert.equal(request.url, '/v1/messages')
    response.writeHead(200, { 'content-type': 'text/event-stream' })
    const events = [
      ['message_start', { type: 'message_start', message: { id: 'msg-1', type: 'message', role: 'assistant', model: 'MiniMax-M3', content: [], stop_reason: null, stop_sequence: null, usage: { input_tokens: 4, output_tokens: 0 } } }],
      ['content_block_start', { type: 'content_block_start', index: 0, content_block: { type: 'thinking', thinking: '', signature: '' } }],
      ['content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'thinking_delta', thinking: '检查任务。' } }],
      ['content_block_delta', { type: 'content_block_delta', index: 0, delta: { type: 'signature_delta', signature: 'sig' } }],
      ['content_block_stop', { type: 'content_block_stop', index: 0 }],
      ['content_block_start', { type: 'content_block_start', index: 1, content_block: { type: 'text', text: '' } }],
      ['content_block_delta', { type: 'content_block_delta', index: 1, delta: { type: 'text_delta', text: '正式回答。' } }],
      ['content_block_stop', { type: 'content_block_stop', index: 1 }],
      ['message_delta', { type: 'message_delta', delta: { stop_reason: 'end_turn', stop_sequence: null }, usage: { output_tokens: 6 } }],
      ['message_stop', { type: 'message_stop' }],
    ]
    for (const [name, data] of events) response.write(`event: ${name}\ndata: ${JSON.stringify(data)}\n\n`)
    response.end()
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  context.after(() => server.close())
  const address = server.address()
  assert.ok(address && typeof address === 'object')

  const runtime = startRuntime()
  context.after(() => runtime.close())
  const initialize = runtime.send('initialize', {
    payload: { modelService: { baseUrl: `http://127.0.0.1:${address.port}`, modelId: 'MiniMax-M3', apiType: 'anthropic-messages', apiKey: 'test-key', contextWindow: 4096, maxOutputTokens: 512 } },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  const sessionId = 'pi-anthropic-1'
  const conversationId = 'conversation-anthropic-1'
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', { conversationId, runtimeSessionId: sessionId, payload: { sessionPath } })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  runtime.send('prompt', { conversationId, runtimeSessionId: sessionId, runId: 'run-anthropic-1', payload: { text: 'test', messages: [{ role: 'user', content: 'test' }] } })
  const reasoning = await runtime.waitFor((message) => message.runId === 'run-anthropic-1' && message.payload?.type === 'reasoning.delta')
  const answer = await runtime.waitFor((message) => message.runId === 'run-anthropic-1' && message.payload?.type === 'message.delta')
  await runtime.waitFor((message) => message.runId === 'run-anthropic-1' && message.payload?.type === 'run.completed')
  assert.equal(reasoning.payload.delta, '检查任务。')
  assert.equal(reasoning.payload.source, 'provider')
  assert.equal(answer.payload.delta, '正式回答。')
})

test('streams OpenAI-compatible reasoning_content separately from the final answer', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-openai-'))
  context.after(() => rm(directory, { recursive: true, force: true }))
  const server = createServer((request, response) => {
    assert.equal(request.url, '/v1/chat/completions')
    response.writeHead(200, { 'content-type': 'text/event-stream' })
    const chunks = [
      { id: 'chatcmpl-1', object: 'chat.completion.chunk', created: 1, model: 'MiniMax-M3', choices: [{ index: 0, delta: { role: 'assistant', reasoning_content: '检查项目文件。' }, finish_reason: null }] },
      { id: 'chatcmpl-1', object: 'chat.completion.chunk', created: 1, model: 'MiniMax-M3', choices: [{ index: 0, delta: { content: '正式回答。' }, finish_reason: null }] },
      { id: 'chatcmpl-1', object: 'chat.completion.chunk', created: 1, model: 'MiniMax-M3', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 4, completion_tokens: 6, total_tokens: 10 } },
    ]
    for (const chunk of chunks) response.write(`data: ${JSON.stringify(chunk)}\n\n`)
    response.write('data: [DONE]\n\n')
    response.end()
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  context.after(() => server.close())
  const address = server.address()
  assert.ok(address && typeof address === 'object')

  const runtime = startRuntime()
  context.after(() => runtime.close())
  const initialize = runtime.send('initialize', {
    payload: { modelService: { baseUrl: `http://127.0.0.1:${address.port}/v1`, modelId: 'MiniMax-M3', apiType: 'openai-completions', apiKey: 'test-key', contextWindow: 4096, maxOutputTokens: 512 } },
  })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  const sessionId = 'pi-openai-1'
  const conversationId = 'conversation-openai-1'
  const sessionPath = join(directory, 'session.json')
  const create = runtime.send('create_session', { conversationId, runtimeSessionId: sessionId, payload: { sessionPath } })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')

  runtime.send('prompt', { conversationId, runtimeSessionId: sessionId, runId: 'run-openai-1', payload: { text: 'test', messages: [{ role: 'user', content: 'test' }] } })
  const reasoning = await runtime.waitFor((message) => message.runId === 'run-openai-1' && message.payload?.type === 'reasoning.delta')
  const answer = await runtime.waitFor((message) => message.runId === 'run-openai-1' && message.payload?.type === 'message.delta')
  await runtime.waitFor((message) => message.runId === 'run-openai-1' && message.payload?.type === 'run.completed')
  assert.equal(reasoning.payload.delta, '检查项目文件。')
  assert.equal(reasoning.payload.source, 'provider')
  assert.equal(reasoning.payload.providerField, 'reasoning_content')
  assert.equal(answer.payload.delta, '正式回答。')
})
