// Stage two of the single Pi mainline: a pi-native session is owned by Pi's
// SessionManager. These tests drive the real runtime child process and a real
// HTTP model endpoint, so "the context came back" means the provider received
// the earlier turn on its own, not that Fox replayed a transcript.
import test from 'node:test'
import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { createServer } from 'node:http'
import { existsSync } from 'node:fs'
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { SessionManager } from '../src/pi-adapter.mjs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'

const runtimePath = fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url))

function startRuntime() {
  const child = spawn(process.execPath, [runtimePath], { stdio: ['pipe', 'pipe', 'pipe'] })
  const childClosed = new Promise((resolve) => child.once('close', resolve))
  child.on('error', () => {})
  const messages = []
  const waiters = []
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
  output.on('line', (line) => {
    messages.push(JSON.parse(line))
    for (const waiter of [...waiters]) waiter()
  })
  return {
    child,
    messages,
    send(type, fields = {}) {
      const request = createEnvelope('request', type, fields)
      child.stdin.write(`${JSON.stringify(request)}\n`)
      return request
    },
    async waitFor(predicate, timeout = 20_000) {
      const existing = messages.find(predicate)
      if (existing) return existing
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error(`timed out: ${JSON.stringify(messages.slice(-6))}`)), timeout)
        const check = () => {
          const message = messages.find(predicate)
          if (!message) return
          clearTimeout(timer)
          resolve(message)
        }
        waiters.push(check)
      })
    },
    async close() {
      output.close()
      if (child.exitCode === null && child.signalCode === null) child.kill()
      await Promise.race([childClosed, new Promise((resolve) => setTimeout(resolve, 5_000))])
    },
  }
}

/** A real completions endpoint that records what the model was asked. */
async function startProvider(reply) {
  const requests = []
  const server = createServer(async (req, res) => {
    let raw = ''
    for await (const chunk of req) raw += chunk
    requests.push(JSON.parse(raw))
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    for (const choice of [
      { index: 0, delta: { role: 'assistant', content: reply }, finish_reason: null },
      { index: 0, delta: {}, finish_reason: 'stop' },
    ]) {
      res.write(`data: ${JSON.stringify({ id: 'native', object: 'chat.completion.chunk', created: 1, model: 'native-test', choices: [choice] })}\n\n`)
    }
    res.end('data: [DONE]\n\n')
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  return { requests, baseUrl: `http://127.0.0.1:${server.address().port}/v1`, close: () => { server.closeAllConnections(); server.close() } }
}

async function initialize(runtime, baseUrl) {
  const request = runtime.send('initialize', { payload: { modelService: {
    apiType: 'openai-completions', modelId: 'native-test', baseUrl, apiKey: 'local-test', contextWindow: 32768, maxOutputTokens: 1024,
  } } })
  const ready = await runtime.waitFor((message) => message.requestId === request.id)
  assert.equal(ready.type, 'ready', JSON.stringify(ready))
}

function prompt(runtime, { conversationId, runtimeSessionId, runId, text }) {
  runtime.send('prompt', { conversationId, runtimeSessionId, runId, payload: { text, projectContext: { projectRoot: null } } })
  return runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
}

test('a pi-native session writes a real JSONL file and takes over the context', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-native-session-'))
  const provider = await startProvider('第一轮回复：验收押金 37,500 元。')
  const identity = { conversationId: 'conversation-native', runtimeSessionId: 'session-native' }
  const runtime = startRuntime()
  context.after(async () => {
    await runtime.close()
    provider.close()
    await rm(directory, { recursive: true, force: true })
  })
  await initialize(runtime, provider.baseUrl)

  const create = runtime.send('create_session', {
    ...identity,
    payload: { sessionFormat: 'pi-native', cwd: directory, sessionDir: join(directory, 'sessions') },
  })
  const created = await runtime.waitFor((message) => message.requestId === create.id)
  assert.equal(created.type, 'session_created', JSON.stringify(created))
  assert.equal(created.payload.format, 'pi-native')
  assert.match(created.payload.nativeSessionPath, /\.jsonl$/)
  assert.ok(created.payload.nativeSessionId)
  const sessionPath = created.payload.nativeSessionPath

  await prompt(runtime, { ...identity, runId: 'run-native-1', text: '城北项目验收押金多少？' })
  assert.ok(existsSync(sessionPath), 'Pi wrote its own session file')
  const firstFile = await readFile(sessionPath, 'utf8')
  assert.ok(firstFile.includes('城北项目验收押金多少'), 'the user turn is in the Pi session file')
  assert.ok(firstFile.trim().split('\n').length >= 3, 'header plus entries')

  // A NEW process opens the exact file and continues: the earlier turn reaches
  // the provider without Fox replaying any transcript.
  const second = startRuntime()
  context.after(async () => { await second.close() })
  await initialize(second, provider.baseUrl)
  const resume = second.send('resume_session', {
    ...identity,
    payload: { sessionFormat: 'pi-native', nativeSessionPath: sessionPath, nativeSessionId: created.payload.nativeSessionId },
  })
  const resumed = await second.waitFor((message) => message.requestId === resume.id)
  assert.equal(resumed.type, 'session_created', JSON.stringify(resumed))
  assert.equal(resumed.payload.nativeSessionPath, sessionPath)
  assert.equal(resumed.payload.nativeSessionId, created.payload.nativeSessionId)

  await prompt(second, { ...identity, runId: 'run-native-2', text: '刚才那个押金是多少？' })
  const secondRequest = provider.requests.at(-1)
  const sent = JSON.stringify(secondRequest.messages ?? secondRequest)
  assert.ok(sent.includes('城北项目验收押金多少'), 'the restored context carries the first turn')
  assert.ok(sent.includes('第一轮回复'), 'and the first answer')
  const afterTwo = await readFile(sessionPath, 'utf8')
  assert.ok(afterTwo.split('\n').filter(Boolean).length > firstFile.split('\n').filter(Boolean).length, 'both turns live in one Pi session file')
})

test('a damaged, missing or foreign session file is a distinguishable failure', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-native-broken-'))
  const provider = await startProvider('ok')
  const runtime = startRuntime()
  context.after(async () => {
    await runtime.close()
    provider.close()
    await rm(directory, { recursive: true, force: true })
  })
  await initialize(runtime, provider.baseUrl)
  const identity = { conversationId: 'conversation-broken', runtimeSessionId: 'session-broken' }

  async function resumeWith(payload) {
    const request = runtime.send('resume_session', { ...identity, payload })
    return runtime.waitFor((message) => message.requestId === request.id)
  }

  const missingPath = await resumeWith({ sessionFormat: 'pi-native' })
  assert.equal(missingPath.type, 'request_failed')
  assert.equal(missingPath.payload.code, 'session.native_path_required')

  const absent = await resumeWith({ sessionFormat: 'pi-native', nativeSessionPath: join(directory, 'nope.jsonl') })
  assert.equal(absent.type, 'request_failed')
  assert.equal(absent.payload.code, 'session.native_missing', JSON.stringify(absent.payload))

  const damagedPath = join(directory, 'damaged.jsonl')
  await writeFile(damagedPath, '{not json at all\n第二行也不是 JSON\n', 'utf8')
  const damaged = await resumeWith({ sessionFormat: 'pi-native', nativeSessionPath: damagedPath })
  assert.equal(damaged.type, 'request_failed')
  assert.equal(damaged.payload.code, 'session.native_corrupt', JSON.stringify(damaged.payload))
  assert.ok(!/created/i.test(damaged.payload.message), 'a failed resume never reports a new session')
})

test('a legacy fox-snapshot session keeps its exact behaviour', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-legacy-session-'))
  const provider = await startProvider('legacy ok')
  const runtime = startRuntime()
  context.after(async () => {
    await runtime.close()
    provider.close()
    await rm(directory, { recursive: true, force: true })
  })
  await initialize(runtime, provider.baseUrl)
  const sessionPath = join(directory, 'session.json')
  const request = runtime.send('create_session', { conversationId: 'c-legacy', runtimeSessionId: 's-legacy', payload: { sessionPath } })
  const created = await runtime.waitFor((message) => message.requestId === request.id)
  assert.equal(created.type, 'session_created')
  assert.equal(created.payload.format, undefined, 'no native metadata on the legacy path')
  assert.ok(!existsSync(join(directory, 'sessions')), 'the legacy path creates no Pi session directory')
})

test('ps4_resume_of_another_sessions_file_is_refused_before_any_truncation', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-ps4-identity-'))
  const provider = await startProvider('ok')
  const runtime = startRuntime()
  context.after(async () => {
    await runtime.close()
    provider.close()
    await rm(directory, { recursive: true, force: true })
  })
  await initialize(runtime, provider.baseUrl)
  const sessionPath = join(directory, 'other.jsonl')
  const header = '{"type":"session","version":3,"id":"session-real","timestamp":"2026-01-01T00:00:00.000Z","cwd":"/tmp"}\n'
  // 末尾半行是 prepareNativeSessionFileForResume 会截掉的那种损坏。身份不对时一个字节都不能动。
  await writeFile(sessionPath, `${header}{"type":"message","id":"partial"`)
  const before = await readFile(sessionPath)
  const request = runtime.send('resume_session', {
    conversationId: 'conversation-ps4',
    runtimeSessionId: 'session-ps4-identity',
    payload: {
      sessionFormat: 'pi-native',
      nativeSessionPath: sessionPath,
      nativeSessionId: 'session-other',
    },
  })
  const failed = await runtime.waitFor((message) => message.requestId === request.id)
  assert.equal(failed.type, 'request_failed')
  assert.equal(failed.payload.code, 'session.native_identity_mismatch', JSON.stringify(failed.payload))
  assert.deepEqual(await readFile(sessionPath), before)
})

test('ps4_resume_from_another_conversation_is_refused', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-ps4-conversation-'))
  const provider = await startProvider('ok')
  const owner = { conversationId: 'conversation-ps4-owner', runtimeSessionId: 'session-ps4-owner' }
  const runtime = startRuntime()
  context.after(async () => {
    await runtime.close()
    provider.close()
    await rm(directory, { recursive: true, force: true })
  })
  await initialize(runtime, provider.baseUrl)
  const create = runtime.send('create_session', {
    ...owner,
    payload: { sessionFormat: 'pi-native', cwd: directory, sessionDir: join(directory, 'sessions') },
  })
  const created = await runtime.waitFor((message) => message.requestId === create.id)
  assert.equal(created.type, 'session_created', JSON.stringify(created))
  await prompt(runtime, { ...owner, runId: 'run-ps4-owner', text: '记下这个会话属于原对话。' })
  const sessionPath = created.payload.nativeSessionPath
  const stored = await readFile(sessionPath, 'utf8')
  assert.ok(stored.includes('"customType":"fox.conversation"'), 'create_session 应写入会话归属')
  assert.ok(stored.includes(owner.conversationId))

  const other = startRuntime()
  context.after(async () => { await other.close() })
  await initialize(other, provider.baseUrl)
  const resume = other.send('resume_session', {
    conversationId: 'conversation-ps4-other',
    runtimeSessionId: 'session-ps4-other',
    payload: {
      sessionFormat: 'pi-native',
      nativeSessionPath: sessionPath,
      nativeSessionId: created.payload.nativeSessionId,
    },
  })
  const failed = await other.waitFor((message) => message.requestId === resume.id)
  assert.equal(failed.type, 'request_failed')
  assert.equal(failed.payload.code, 'session.native_conversation_mismatch', JSON.stringify(failed.payload))
})

test('ps4_sessions_created_before_this_change_still_resume', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-ps4-legacy-native-'))
  const provider = await startProvider('ok')
  const runtime = startRuntime()
  context.after(async () => {
    await runtime.close()
    provider.close()
    await rm(directory, { recursive: true, force: true })
  })
  await initialize(runtime, provider.baseUrl)
  // 改动之前的文件只有 Pi 自己的头和消息，没有 fox.conversation。
  const manager = SessionManager.create(directory, join(directory, 'sessions'))
  manager.appendMessage({
    role: 'user',
    content: [{ type: 'text', text: '这是改动前就存在的会话。' }],
    timestamp: Date.now(),
  })
  const sessionPath = manager.getSessionFile()
  const nativeSessionId = manager.getSessionId()
  const stored = await readFile(sessionPath, 'utf8')
  assert.equal(stored.includes('fox.conversation'), false)
  const resume = runtime.send('resume_session', {
    conversationId: 'conversation-ps4-legacy',
    runtimeSessionId: 'session-ps4-legacy',
    payload: { sessionFormat: 'pi-native', nativeSessionPath: sessionPath, nativeSessionId },
  })
  const resumed = await runtime.waitFor((message) => message.requestId === resume.id)
  assert.equal(resumed.type, 'session_created', JSON.stringify(resumed))
  assert.equal(resumed.payload.nativeSessionId, nativeSessionId)
})
