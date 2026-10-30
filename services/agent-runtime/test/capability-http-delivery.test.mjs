import test from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash } from 'node:crypto'
import { createInterface } from 'node:readline'
import { createServer } from 'node:http'
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'
import { canonicalPermission } from '../src/control-binding.mjs'

const runtimePath = fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url))

function sse(res, text) {
  const write = (value) => res.write(`data: ${JSON.stringify(value)}\n\n`)
  res.writeHead(200, { 'content-type': 'text/event-stream', 'cache-control': 'no-store' })
  write({ id: 'cap-http', object: 'chat.completion.chunk', created: 1, model: 'cap-http', choices: [{ index: 0, delta: { role: 'assistant', content: '' } }] })
  write({ id: 'cap-http', object: 'chat.completion.chunk', created: 1, model: 'cap-http', choices: [{ index: 0, delta: { content: text } }] })
  write({ id: 'cap-http', object: 'chat.completion.chunk', created: 1, model: 'cap-http', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }], usage: { prompt_tokens: 100, completion_tokens: 2, total_tokens: 102 } })
  res.end('data: [DONE]\n\n')
}

function binding(conversationId, runId, projectRoot) {
  const permission = { mode: 'ask', projectRoot, grants: [], approvalEpoch: null }
  return {
    schemaVersion: 1, runId, conversationId, engineId: 'pi', executionProfileId: 'legacy',
    authority: 'authoritative', readOnlyExecutor: 'runtime',
    permissionSnapshotId: `sha256:${createHash('sha256').update(JSON.stringify(canonicalPermission(permission))).digest('hex')}`,
    permission,
    budgets: { modelRequestMs: 120000, toolExecutionMs: 120000, runExecutionMs: 600000, approvalWaitMs: 60000 },
    executorRoute: 'pi_native',
  }
}

test('the real Pi SDK extension serializes Host capability facts into the actual HTTP system prompt', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-capability-http-'))
  let captured = null
  const server = createServer(async (req, res) => {
    let raw = ''
    req.setEncoding('utf8')
    for await (const chunk of req) raw += chunk
    const body = JSON.parse(raw)
    const system = (body.messages ?? [])
      .filter((message) => message.role === 'system')
      .map((message) => typeof message.content === 'string' ? message.content : '')
      .join('\n')
    captured = { system, tools: (body.tools ?? []).map((tool) => tool.function?.name ?? tool.name) }
    sse(res, 'CAP_HTTP_OK')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const { port } = server.address()
  const child = spawn(process.execPath, [runtimePath], { stdio: ['pipe', 'pipe', 'pipe'] })
  const childClosed = new Promise((resolve) => child.once('close', resolve))
  const messages = []
  const waiters = []
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
  output.on('line', (line) => {
    const message = JSON.parse(line)
    messages.push(message)
    for (const waiter of [...waiters]) waiter()
    if (message.kind === 'request' && message.type === 'model_request.admit') {
      child.stdin.write(`${JSON.stringify(createEnvelope('response', 'model_request.admitted', {
        requestId: message.id, conversationId: message.conversationId, runtimeSessionId: message.runtimeSessionId, runId: message.runId,
        payload: { allowed: true, timeoutMs: 120000, firstResponseMs: 120000, idleMs: 120000, maxOutputTokens: message.payload.maxOutputTokens },
      }))}\n`)
    }
    if (message.kind === 'request' && message.type === 'model_request.complete') {
      child.stdin.write(`${JSON.stringify(createEnvelope('response', 'model_request.completed', {
        requestId: message.id, conversationId: message.conversationId, runtimeSessionId: message.runtimeSessionId, runId: message.runId,
        payload: { completed: true },
      }))}\n`)
    }
  })
  const send = (type, fields) => {
    const request = createEnvelope('request', type, fields)
    child.stdin.write(`${JSON.stringify(request)}\n`)
    return request
  }
  const waitFor = async (predicate, timeout = 15000) => {
    const existing = messages.find(predicate)
    if (existing) return existing
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error(`timed out: ${JSON.stringify(messages.slice(-8))}`)), timeout)
      const check = () => {
        const found = messages.find(predicate)
        if (!found) return
        clearTimeout(timer)
        resolve(found)
      }
      waiters.push(check)
    })
  }
  try {
    const initialize = send('initialize', { payload: { modelService: {
      baseUrl: `http://127.0.0.1:${port}/v1`, modelId: 'cap-http', apiType: 'openai-completions',
      contextWindow: 65536, maxOutputTokens: 1024,
    } } })
    await waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
    const conversationId = 'cap-http-conversation'
    const runtimeSessionId = 'cap-http-session'
    const create = send('create_session', { conversationId, runtimeSessionId, payload: { sessionPath: join(directory, 'session.json') } })
    await waitFor((message) => message.requestId === create.id && message.type === 'session_created')
    const runId = 'cap-http-run'
    const controlBinding = binding(conversationId, runId, directory)
    const facts = {
      schemaVersion: 1,
      capabilityManifestHash: 'sha256:capability-http-manifest',
      executionProfileId: 'legacy',
      tools: { run_command: { availability: 'available', reasonCode: null, reason: null, actions: { start: 'available', status: 'available', output: 'available', cancel: 'available' } } },
    }
    send('prompt', {
      conversationId, runtimeSessionId, runId,
      payload: {
        text: 'Return the deterministic marker.',
        controlBinding,
        systemPrompt: 'You are Fox. Follow the Host runtime contract.',
        projectContext: { projectRoot: directory, permissionMode: 'ask' },
        hostRuntimeCapabilities: facts,
      },
    })
    await waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
    assert.ok(captured, 'the real SDK must issue an HTTP request')
    assert.match(captured.system, /Host runtime availability for this frozen manifest\/profile:/)
    assert.match(captured.system, /<fox_context_block kind="runtime"/)
    assert.match(captured.system, /"executionProfileId":"legacy"/)
    assert.match(captured.system, /"capabilityManifestHash":"sha256:capability-http-manifest"/)
    assert.ok(captured.tools.includes('run_command'), 'available run_command must remain in the provider tool list')
  } finally {
    output.close()
    if (child.exitCode === null && child.signalCode === null) child.kill()
    await Promise.race([childClosed, new Promise((resolve) => setTimeout(resolve, 5000))])
    await new Promise((resolve) => server.close(resolve))
    await rm(directory, { recursive: true, force: true })
  }
})
