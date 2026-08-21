import { access, mkdtemp, rm } from 'node:fs/promises'
import { spawn } from 'node:child_process'
import { tmpdir } from 'node:os'
import { dirname, join, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../src/protocol.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const runtimePath = resolve(root, 'dist', 'fox-agent-runtime-x86_64-pc-windows-msvc.exe')
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
  await waitFor((message) => message.requestId === initialize.id && message.type === 'ready')

  const conversationId = 'sidecar-smoke-conversation'
  const runtimeSessionId = 'sidecar-smoke-session'
  const createSession = send('create_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await waitFor((message) => message.requestId === createSession.id && message.type === 'session_created')

  const runId = 'sidecar-smoke-run'
  send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: {
      text: 'Verify the compiled Fox Runtime.',
      messages: [{ role: 'user', content: 'Verify the compiled Fox Runtime.' }],
    },
  })
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
