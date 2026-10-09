import { access, mkdtemp, rm } from 'node:fs/promises'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { spawn } from 'node:child_process'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'
import { canonicalPermission } from '../src/control-binding.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const runtimePath = resolve(root, 'dist', 'fox-agent-runtime-x86_64-pc-windows-msvc.exe')
const COLD_START_TIMEOUT = 90_000
await access(runtimePath)

const directory = await mkdtemp(join(tmpdir(), 'fox-runtime-smoke-'))
const child = spawn(runtimePath, [], { stdio: ['pipe', 'pipe', 'pipe'] })
const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
const messages = []
const waiters = new Set()
let stderr = ''
let exited = false

child.stderr.on('data', (chunk) => { stderr += chunk.toString() })
child.once('exit', () => { exited = true })
output.on('line', (line) => {
  messages.push(JSON.parse(line))
  for (const waiter of [...waiters]) waiter()
})

function send(type, fields = {}) {
  const request = createEnvelope('request', type, fields)
  child.stdin.write(`${JSON.stringify(request)}\n`)
  return request
}

async function waitFor(predicate, timeout = 30_000) {
  const existing = messages.find(predicate)
  if (existing) return existing

  return new Promise((resolveWait, reject) => {
    const check = () => {
      const message = messages.find(predicate)
      if (!message) return
      clearTimeout(timer)
      waiters.delete(check)
      resolveWait(message)
    }
    const timer = setTimeout(() => {
      waiters.delete(check)
      reject(new Error(`Timed out waiting for Sidecar output. stderr: ${stderr}`))
    }, timeout)
    waiters.add(check)
  })
}

try {
  const initialize = send('initialize', {
    payload: {
      modelService: {
        baseUrl: 'faux://fox',
        modelId: 'fox-sidecar-smoke',
        apiType: 'faux',
        contextWindow: 4096,
        maxOutputTokens: 512,
      },
    },
  })
  await waitFor((message) => message.requestId === initialize.id && message.type === 'ready', COLD_START_TIMEOUT)

  const conversationId = 'sidecar-smoke-conversation'
  const runtimeSessionId = 'sidecar-smoke-session'
  const createSession = send('create_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await waitFor((message) => message.requestId === createSession.id && message.type === 'session_created')

  const runId = 'sidecar-smoke-run'
  const permission = canonicalPermission({ mode: 'ask', projectRoot: null, grants: [] })
  const controlBinding = {
    schemaVersion: 1, runId, conversationId, engineId: 'pi', executionProfileId: 'legacy',
    authority: 'legacy', readOnlyExecutor: 'runtime', permission,
    permissionSnapshotId: `sha256:${createHash('sha256').update(JSON.stringify(permission)).digest('hex')}`,
    budgets: { modelRequestMs: 120_000, toolExecutionMs: 600_000, runExecutionMs: 1_800_000, approvalWaitMs: 300_000 },
  }
  const rejected = send('prompt', {
    conversationId, runtimeSessionId, runId,
    payload: { text: 'Must not execute with a different engine.', controlBinding: { ...controlBinding, engineId: 'codex' } },
  })
  const rejection = await waitFor(message => message.requestId === rejected.id)
  assert.equal(rejection.type, 'request_failed')
  assert.equal(messages.some(message => message.runId === runId && message.type === 'runtime_event'), false)
  const acceptedPrompt = send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: {
      text: 'Verify the compiled Fox Runtime.',
      controlBinding,
      messages: [{ role: 'user', content: 'Verify the compiled Fox Runtime.' }],
    },
  })
  const acceptance = await waitFor(message => message.requestId === acceptedPrompt.id)
  assert.equal(acceptance.type, 'request_succeeded', acceptance.payload?.message)
  await waitFor((message) => message.runId === runId && message.payload?.type === 'message.delta')
  await waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')

  const shutdown = send('shutdown')
  await waitFor((message) => message.requestId === shutdown.id && message.type === 'request_succeeded')
  console.log('Fox Runtime Sidecar smoke test passed.')
} finally {
  output.close()
  child.stdin.end()
  if (!exited) child.kill()
  await rm(directory, { recursive: true, force: true })
}
