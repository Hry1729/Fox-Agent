// Real-endpoint route probe, with no credential of any kind.
//
// Both Fox Pi paths are pointed at the *real* configured provider with an
// obviously fake key. Authentication is the first thing an OpenAI-compatible
// gateway checks, so a path reaches the right route when it answers 401 (auth)
// and the wrong route when it answers 404 (no such route).
//
//  * SDK path  : dialled directly (`initialize` -> `create_session` -> `prompt`).
//  * Kernel path: dialled through a local pass-through recorder, so the exact
//    method/path/body the worker sends is also kept as evidence.
//
// The only model input sent is the fixed string "route probe"; no user data and
// no credential are involved. Prints a PASS/FAIL for "both paths dial the same
// provider route".
//
// Usage: node probe-real-endpoint.mjs [baseUrl]   (default https://token.sensenova.cn/v1)
import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { controlBinding } from './lib-capture.mjs'
import { safeRemove } from './lib-cleanup.mjs'
import { startRecordingServer } from './lib-recording-server.mjs'
import { FAKE_API_KEY, modelService, startSidecar } from './lib-sidecar.mjs'

const REAL_BASE_URL = process.argv[2] ?? 'https://token.sensenova.cn/v1'
const REAL_ORIGIN = new URL(REAL_BASE_URL).origin
const PROBE_TEXT = 'route probe'

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))

async function waitForMessage(client, predicate, timeoutMs = 45_000) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const hit = client.messages.find(predicate)
    if (hit) return hit
    await sleep(25)
  }
  return null
}

/** The SDK path, dialled straight at the real provider. */
async function probeSdkDirect(baseUrl) {
  const work = await mkdtemp(join(tmpdir(), 'fox-diag-live-sdk-'))
  const client = await startSidecar({ cwd: work })
  try {
    const init = client.send('initialize', { payload: { modelService: { ...modelService(baseUrl), apiKey: FAKE_API_KEY }, executionProfile: 'legacy' } })
    const ready = await client.waitFor(m => m.requestId === init.id, { label: 'initialize' })
    if (ready.type !== 'ready') return { path: 'sdk', outcome: 'initialize_failed', detail: ready.payload }
    const create = client.send('create_session', { conversationId: 'live-probe', runtimeSessionId: 'live-probe', payload: { sessionFormat: 'pi-native', sessionDir: work, cwd: work } })
    const created = await client.waitFor(m => m.requestId === create.id, { label: 'create_session' })
    if (created.type !== 'session_created') return { path: 'sdk', outcome: 'session_failed', detail: created.payload }
    const prompt = client.send('prompt', { conversationId: 'live-probe', runtimeSessionId: 'live-probe', runId: 'live-probe-run', payload: { text: PROBE_TEXT, messages: [{ role: 'user', content: PROBE_TEXT }] } })
    await client.waitFor(m => m.requestId === prompt.id, { label: 'prompt' })
    const failed = await waitForMessage(client, m => m.type === 'runtime_event' && m.runId === 'live-probe-run' && m.payload?.type === 'run.failed')
    return {
      path: 'sdk',
      outcome: failed ? 'provider_rejected' : 'no_provider_error',
      message: failed?.payload?.message ?? '',
      requests: [],
    }
  } finally {
    await client.stop()
    await safeRemove(work)
  }
}

/** The Kernel path, dialled through the pass-through recorder. */
async function probeKernelThroughRecorder(realOrigin) {
  const work = await mkdtemp(join(tmpdir(), 'fox-diag-live-kernel-'))
  const server = await startRecordingServer({ forwardTo: realOrigin })
  const client = await startSidecar({ mode: 'kernel', cwd: work })
  try {
    const runId = 'live-probe-kernel'
    const conversationId = 'live-probe-kernel'
    const runtimeSessionId = 'live-probe-kernel'
    const init = client.send('kernel.initialize', {
      runId, conversationId, runtimeSessionId,
      payload: { executionProfileId: 'legacy', engineId: 'pi',
        modelService: { ...modelService(server.baseUrl), apiKey: FAKE_API_KEY },
        systemPrompt: 'route probe', proposalTools: [] },
    })
    const ready = await client.waitFor(m => m.requestId === init.id, { label: 'kernel.initialize' })
    if (ready.type !== 'kernel.ready') return { path: 'kernel', outcome: 'initialize_failed', detail: ready.payload, requests: [...server.requests] }
    const round = client.send('kernel.start_initial', {
      runId, conversationId, runtimeSessionId,
      payload: {
        controlBinding: controlBinding({ runId, conversationId }),
        initialModel: {
          schemaVersion: 1, idempotencyKey: 'initial-model-delivery', checkpointSeq: 1,
          input: { schemaVersion: 1, runId, turnId: 'live-turn', promptConfigHash: `sha256:${'1'.repeat(64)}`,
            messages: [{ role: 'user', content: [{ type: 'text', text: PROBE_TEXT }] }] },
        },
      },
    })
    await client.waitFor(m => m.requestId === round.id, { label: 'kernel.start_initial', timeoutMs: 60_000 })
    return { path: 'kernel', outcome: 'model_round_settled', requests: [...server.requests] }
  } finally {
    await client.stop()
    await server.close()
    await safeRemove(work)
  }
}

const sdk = await probeSdkDirect(REAL_BASE_URL)
const kernel = await probeKernelThroughRecorder(REAL_ORIGIN)

const report = {
  baseUrl: REAL_BASE_URL,
  sdk: { outcome: sdk.outcome, message: sdk.message ?? null },
  kernel: {
    outcome: kernel.outcome,
    requests: kernel.requests.map(record => ({
      method: record.method, path: record.path, model: record.body?.model ?? null,
      upstreamStatus: record.upstreamStatus ?? null, upstreamBody: record.upstreamBody ?? record.upstreamError ?? null,
    })),
  },
}

console.log(JSON.stringify(report, null, 2))

const sdkOk = /401/.test(report.sdk.message ?? '')
const kernelOk = report.kernel.requests.length > 0
  && report.kernel.requests.every(request => request.method === 'POST' && request.path === '/v1/chat/completions')
  && report.kernel.requests.every(request => request.upstreamStatus === 401)
const pass = sdkOk && kernelOk
console.log(pass
  ? 'PASS  both paths dial POST <baseUrl>/chat/completions at the real provider; it answers 401 auth, never 404 route'
  : 'FAIL  a path did not reach the same real route; compare the evidence above')
process.exitCode = pass ? 0 : 1
