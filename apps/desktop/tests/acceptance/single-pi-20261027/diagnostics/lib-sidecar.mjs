// Minimal driver for the Fox Pi sidecar (`services/agent-runtime/src/pi-runtime.mjs`).
//
// Two modes are used by the diagnostics:
//   * legacy (no flag)      -> the SDK / sidecar path (create_session, prompt, resume_session)
//   * --kernel-worker       -> the authoritative Kernel path (kernel.initialize, kernel.compact_context)
//
// No credential is read here; callers pass an obviously fake apiKey string. The
// envelope is built by the runtime's own `createEnvelope`, so the driver cannot
// drift from the wire contract it is testing.
import { spawn } from 'node:child_process'
import { createInterface } from 'node:readline'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { createEnvelope } from '../../../../../../services/agent-runtime/src/protocol.mjs'

const here = dirname(fileURLToPath(import.meta.url))
export const repoRoot = resolve(here, '../../../../../..')
export const agentRuntimeRoot = resolve(repoRoot, 'services/agent-runtime')
export const runtimeScript = resolve(agentRuntimeRoot, 'src/pi-runtime.mjs')

export const FAKE_API_KEY = 'diagnostics-not-a-credential'

/** Spawn the sidecar and return a promise-friendly client. */
export async function startSidecar({ mode = 'legacy', cwd = process.cwd(), env = {} } = {}) {
  const args = mode === 'kernel' ? [runtimeScript, '--kernel-worker'] : [runtimeScript]
  const child = spawn(process.execPath, args, {
    cwd,
    stdio: ['pipe', 'pipe', 'pipe'],
    env: { ...process.env, ...env },
  })
  const messages = []
  const waiters = new Set()
  let stderr = ''
  let exited = false
  const output = createInterface({ input: child.stdout, crlfDelay: Infinity })
  output.on('line', (line) => {
    let parsed
    try { parsed = JSON.parse(line) } catch { parsed = { type: 'unparsed', line } }
    messages.push(parsed)
    for (const waiter of [...waiters]) waiter()
  })
  child.stderr.on('data', (chunk) => { stderr += chunk.toString() })
  child.once('exit', () => { exited = true; for (const waiter of [...waiters]) waiter() })

  function send(type, fields = {}) {
    const request = createEnvelope('request', type, fields)
    child.stdin.write(`${JSON.stringify(request)}\n`)
    return request
  }

  function waitFor(predicate, { timeoutMs = 30_000, label = 'sidecar message' } = {}) {
    const existing = messages.find(predicate)
    if (existing) return Promise.resolve(existing)
    return new Promise((resolveWait, reject) => {
      const check = () => {
        const message = messages.find(predicate)
        if (message) { cleanup(); resolveWait(message); return }
        if (exited) { cleanup(); reject(new Error(`sidecar exited before ${label}. stderr: ${stderr.trim()}`)) }
      }
      const timer = setTimeout(() => {
        cleanup()
        reject(new Error(`timed out waiting for ${label}. stderr: ${stderr.trim()}`))
      }, timeoutMs)
      const cleanup = () => { clearTimeout(timer); waiters.delete(check) }
      waiters.add(check)
      check()
    })
  }

  async function stop() {
    if (exited) return
    try { child.stdin.write(`${JSON.stringify(createEnvelope('request', 'shutdown'))}\n`) } catch { /* gone */ }
    await new Promise(resolveStop => {
      const timer = setTimeout(() => { try { child.kill() } catch { /* ignore */ } ; resolveStop() }, 3000)
      child.once('exit', () => { clearTimeout(timer); resolveStop() })
    })
  }

  return { child, messages, send, waitFor, stop, stderr: () => stderr }
}

/** The modelService object both paths are configured with. Identical by construction. */
export function modelService(baseUrl, overrides = {}) {
  return {
    baseUrl,
    modelId: 'sensenova-6.8-flash-lite',
    apiType: 'openai-completions',
    contextWindow: 256000,
    maxOutputTokens: 65535,
    supportsImageInput: true,
    ...overrides,
  }
}
