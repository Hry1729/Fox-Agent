// Capture the exact provider request each Fox Pi path sends, without credentials.
//
//   captureSdkPath()    -> legacy/SDK sidecar (pi-runtime.mjs): a fresh pi-native
//                          session in one process, then a *second* process that
//                          resumes the same session file
//   captureKernelPath() -> authoritative Kernel worker (pi-runtime.mjs
//                          --kernel-worker), driven through kernel.initialize +
//                          kernel.start_initial, i.e. the very model client a real
//                          Kernel round uses
//
// Both are pointed at one local recording stub with the same base URL, model id,
// protocol and (fake) credential, so any captured difference is a difference in
// the two code paths, not in their configuration.
import { createHash } from 'node:crypto'
import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { canonicalPermission } from '../../../../../../services/agent-runtime/src/control-binding.mjs'
import { safeRemove } from './lib-cleanup.mjs'
import { startRecordingServer } from './lib-recording-server.mjs'
import { FAKE_API_KEY, modelService, startSidecar } from './lib-sidecar.mjs'

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))

/** Wait until the recording stub has seen at least `count` requests. */
async function waitForRequests(server, count, timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs
  while (server.requests.length < count && Date.now() < deadline) await sleep(25)
  return server.requests.length >= count
}

function permissionHash(permission) {
  return `sha256:${createHash('sha256').update(JSON.stringify(canonicalPermission(permission))).digest('hex')}`
}

/** A valid frozen control binding for the Kernel worker's own validator. */
export function controlBinding({ runId, conversationId, authority = 'authoritative' }) {
  const permission = { mode: 'ask', projectRoot: null, grants: [] }
  return {
    schemaVersion: 1,
    runId,
    conversationId,
    engineId: 'pi',
    executionProfileId: 'legacy',
    authority,
    readOnlyExecutor: 'runtime',
    permission,
    permissionSnapshotId: permissionHash(permission),
    budgets: {
      modelRequestMs: 120_000,
      toolExecutionMs: 600_000,
      runExecutionMs: 1_800_000,
      approvalWaitMs: 300_000,
      modelFirstResponseMs: 120_000,
      modelIdleMs: 120_000,
      runExecutionLimited: false,
    },
  }
}

function terminalPredicate(runId) {
  return message => message.type === 'runtime_event' && message.runId === runId
    && ['run.completed', 'run.failed'].includes(message.payload?.type)
}

async function driveLegacyRun(client, { conversationId, runtimeSessionId, runId, text }) {
  const send = client.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: { text, messages: [{ role: 'user', content: text }] },
  })
  const accepted = await client.waitFor(message => message.requestId === send.id, { label: `prompt ${runId}` })
  if (accepted.type === 'request_failed') return { outcome: 'request_failed', detail: accepted.payload }
  const terminal = await client.waitFor(terminalPredicate(runId), { label: `terminal event for ${runId}`, timeoutMs: 60_000 })
  return { outcome: terminal.payload.type, detail: terminal.payload }
}

function tag(requests, from, phase) {
  for (const record of requests.slice(from)) record.phase = phase
}

/**
 * SDK path: a fresh pi-native session, then a *new process* that resumes the
 * same session file — the production "restart then continue" shape.
 */
export async function captureSdkPath() {
  const workDir = await mkdtemp(join(tmpdir(), 'fox-diag-sdk-'))
  const sessionDir = join(workDir, 'sessions')
  const server = await startRecordingServer()
  const runs = []
  let session = null
  try {
    const conversationId = 'diagnostics-conversation-sdk'

    // Phase 1: fresh session in one process.
    let mark = server.requests.length
    const clientA = await startSidecar({ cwd: workDir })
    try {
      const initialize = clientA.send('initialize', {
        payload: { modelService: { ...modelService(server.baseUrl), apiKey: FAKE_API_KEY }, executionProfile: 'legacy' },
      })
      const ready = await clientA.waitFor(message => message.requestId === initialize.id, { label: 'SDK initialize' })
      if (ready.type !== 'ready') throw new Error(`SDK initialize failed: ${JSON.stringify(ready.payload)}`)
      const create = clientA.send('create_session', {
        conversationId,
        runtimeSessionId: 'diagnostics-sdk-session',
        payload: { sessionFormat: 'pi-native', sessionDir, cwd: workDir },
      })
      const created = await clientA.waitFor(message => message.requestId === create.id, { label: 'SDK create_session' })
      if (created.type !== 'session_created') throw new Error(`SDK create_session failed: ${JSON.stringify(created.payload)}`)
      session = created.payload
      const run = await driveLegacyRun(clientA, { conversationId, runtimeSessionId: 'diagnostics-sdk-session', runId: 'diagnostics-sdk-run-1', text: 'diagnostics capture: fresh session' })
      await waitForRequests(server, mark + 1)
      tag(server.requests, mark, 'sdk-fresh-session')
      runs.push({ label: 'sdk-fresh-session', ...run })
    } finally {
      await clientA.stop()
    }

    // Phase 2: a second process resumes the session file the first one wrote.
    mark = server.requests.length
    const clientB = await startSidecar({ cwd: workDir })
    try {
      const initialize = clientB.send('initialize', {
        payload: { modelService: { ...modelService(server.baseUrl), apiKey: FAKE_API_KEY }, executionProfile: 'legacy' },
      })
      const ready = await clientB.waitFor(message => message.requestId === initialize.id, { label: 'SDK initialize (resume)' })
      if (ready.type !== 'ready') throw new Error(`SDK initialize failed: ${JSON.stringify(ready.payload)}`)
      const resume = clientB.send('resume_session', {
        conversationId,
        runtimeSessionId: 'diagnostics-sdk-resumed',
        payload: { sessionFormat: 'pi-native', nativeSessionPath: session.nativeSessionPath, sessionDir, cwd: workDir },
      })
      const resumed = await clientB.waitFor(message => message.requestId === resume.id, { label: 'SDK resume_session' })
      if (resumed.type !== 'session_created') throw new Error(`SDK resume failed: ${JSON.stringify(resumed.payload)}`)
      const run = await driveLegacyRun(clientB, { conversationId, runtimeSessionId: 'diagnostics-sdk-resumed', runId: 'diagnostics-sdk-run-2', text: 'diagnostics capture: resumed session' })
      await waitForRequests(server, mark + 1)
      tag(server.requests, mark, 'sdk-resumed-session')
      runs.push({ label: 'sdk-resumed-session', ...run })
    } finally {
      await clientB.stop()
    }

    // Phase 3: a fresh legacy Fox snapshot session (the pre-pi-native format).
    mark = server.requests.length
    const clientC = await startSidecar({ cwd: workDir })
    try {
      const initialize = clientC.send('initialize', {
        payload: { modelService: { ...modelService(server.baseUrl), apiKey: FAKE_API_KEY }, executionProfile: 'legacy' },
      })
      const ready = await clientC.waitFor(message => message.requestId === initialize.id, { label: 'SDK initialize (snapshot)' })
      if (ready.type !== 'ready') throw new Error(`SDK initialize failed: ${JSON.stringify(ready.payload)}`)
      const create = clientC.send('create_session', {
        conversationId,
        runtimeSessionId: 'diagnostics-sdk-snapshot',
        payload: { sessionFormat: 'fox-snapshot', sessionPath: join(workDir, 'snapshot-session.json') },
      })
      const created = await clientC.waitFor(message => message.requestId === create.id, { label: 'SDK create_session (snapshot)' })
      if (created.type !== 'session_created') throw new Error(`SDK create_session failed: ${JSON.stringify(created.payload)}`)
      const run = await driveLegacyRun(clientC, { conversationId, runtimeSessionId: 'diagnostics-sdk-snapshot', runId: 'diagnostics-sdk-run-3', text: 'diagnostics capture: legacy snapshot session' })
      await waitForRequests(server, mark + 1)
      tag(server.requests, mark, 'sdk-legacy-snapshot')
      runs.push({ label: 'sdk-legacy-snapshot', ...run })
    } finally {
      await clientC.stop()
    }

    return { path: 'sdk', baseUrl: server.baseUrl, sessionPath: session?.nativeSessionPath ?? null,
      requests: [...server.requests], runs }
  } finally {
    await server.close()
    await safeRemove(workDir)
  }
}

/**
 * Kernel path: the same model client, driven by the authoritative worker.
 * `kernel.start_initial` is the real single-round model delivery: the request it
 * sends is built by the same createFoxModelRuntime/openai-completions path a
 * live Kernel round uses.
 */
export async function captureKernelPath() {
  const workDir = await mkdtemp(join(tmpdir(), 'fox-diag-kernel-'))
  const server = await startRecordingServer()
  try {
    const runId = 'diagnostics-kernel-run'
    const conversationId = 'diagnostics-conversation-kernel'
    const runtimeSessionId = 'diagnostics-kernel-session'
    const client = await startSidecar({ mode: 'kernel', cwd: workDir })
    try {
      const initialize = client.send('kernel.initialize', {
        runId, conversationId, runtimeSessionId,
        payload: {
          executionProfileId: 'legacy',
          engineId: 'pi',
          modelService: { ...modelService(server.baseUrl), apiKey: FAKE_API_KEY },
          systemPrompt: 'Diagnostics host instructions.',
          proposalTools: [],
        },
      })
      const ready = await client.waitFor(message => message.requestId === initialize.id, { label: 'Kernel initialize' })
      if (ready.type !== 'kernel.ready') throw new Error(`Kernel initialize failed: ${JSON.stringify(ready.payload)}`)

      const round = client.send('kernel.start_initial', {
        runId, conversationId, runtimeSessionId,
        payload: {
          controlBinding: controlBinding({ runId, conversationId }),
          initialModel: {
            schemaVersion: 1,
            idempotencyKey: 'initial-model-delivery',
            checkpointSeq: 1,
            input: {
              schemaVersion: 1,
              runId,
              turnId: 'diagnostics-turn',
              promptConfigHash: `sha256:${createHash('sha256').update('diagnostics-prompt').digest('hex')}`,
              messages: [{ role: 'user', content: [{ type: 'text', text: 'diagnostics capture: kernel round' }] }],
            },
          },
        },
      })
      const result = await client.waitFor(message => message.requestId === round.id, { label: 'Kernel model round', timeoutMs: 60_000 })
      await waitForRequests(server, 1)
      tag(server.requests, 0, 'kernel-round')
      return { path: 'kernel', baseUrl: server.baseUrl,
        outcome: result.type, detail: result.payload,
        requests: [...server.requests],
        runs: [{ label: 'kernel-round', outcome: result.type }] }
    } finally {
      await client.stop()
    }
  } finally {
    await server.close()
    await safeRemove(workDir)
  }
}
