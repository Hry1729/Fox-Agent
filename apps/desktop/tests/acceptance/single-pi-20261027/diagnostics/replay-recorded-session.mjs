// Replay a *recorded production* Pi-native session against a local stub.
//
// Proves that resuming the exact session file from the failing run still dials
// the configured route: the stub records the request, and this script prints
// PASS/FAIL for "the resumed session sends <baseUrl>/chat/completions with the
// configured model". No credential is used; the session file is copied, never
// written.
//
// Usage:
//   node replay-recorded-session.mjs <path-to-session.jsonl>
//
// Default path (the session the failing SDK run resumed):
//   %TEMP%\fox-acceptance-single-pi\data\runtime-sessions\2026-10-27T15-08-45-763Z_01a17de8-1b43-7443-bfec-5219fa669232.jsonl
import { copyFile, mkdtemp } from 'node:fs/promises'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { basename, join } from 'node:path'
import { safeRemove } from './lib-cleanup.mjs'
import { startRecordingServer } from './lib-recording-server.mjs'
import { FAKE_API_KEY, modelService, startSidecar } from './lib-sidecar.mjs'

const DEFAULT_SESSION = join(process.env.TEMP ?? tmpdir(),
  'fox-acceptance-single-pi', 'data', 'runtime-sessions',
  '2026-10-27T15-08-45-763Z_01a17de8-1b43-7443-bfec-5219fa669232.jsonl')
const source = process.argv[2] ?? DEFAULT_SESSION

if (!existsSync(source)) {
  console.error(`session file not found: ${source}`)
  console.error('Pass the path of a recorded Pi-native session (a copy is made; the file is never modified).')
  process.exit(2)
}

const work = await mkdtemp(join(tmpdir(), 'fox-diag-replay-'))
const copy = join(work, basename(source))
await copyFile(source, copy)
const server = await startRecordingServer()
const client = await startSidecar({ cwd: work })
let outcome = 'no_run'
try {
  const init = client.send('initialize', { payload: { modelService: { ...modelService(server.baseUrl), apiKey: FAKE_API_KEY }, executionProfile: 'legacy' } })
  const ready = await client.waitFor(message => message.requestId === init.id, { label: 'initialize' })
  if (ready.type !== 'ready') throw new Error(`initialize failed: ${JSON.stringify(ready.payload)}`)

  const resume = client.send('resume_session', {
    conversationId: 'diagnostics-replay',
    runtimeSessionId: 'diagnostics-replay',
    payload: { sessionFormat: 'pi-native', nativeSessionPath: copy, sessionDir: work, cwd: work },
  })
  const resumed = await client.waitFor(message => message.requestId === resume.id, { label: 'resume_session' })
  console.log(`resume: ${resumed.type}${resumed.type === 'session_created' ? ` (${resumed.payload.nativeSessionId})` : ` ${JSON.stringify(resumed.payload)}`}`)

  if (resumed.type === 'session_created') {
    const prompt = client.send('prompt', {
      conversationId: 'diagnostics-replay', runtimeSessionId: 'diagnostics-replay', runId: 'diagnostics-replay-run',
      payload: { text: 'diagnostics replay: continue', messages: [{ role: 'user', content: 'diagnostics replay: continue' }] },
    })
    await client.waitFor(message => message.requestId === prompt.id, { label: 'prompt' })
    const terminal = await client.waitFor(
      message => message.type === 'runtime_event' && message.runId === 'diagnostics-replay-run' && ['run.completed', 'run.failed'].includes(message.payload?.type),
      { label: 'terminal event', timeoutMs: 60_000 },
    )
    outcome = terminal.payload.type
  }
} finally {
  await client.stop()
  await server.close()
  await safeRemove(work)
}

const captured = server.requests
for (const record of captured) console.log(`captured ${record.method} ${record.path} model=${record.body?.model} bytes=${record.bodyBytes}`)
const expected = modelService('x')
const pass = outcome === 'run.completed' && captured.length > 0
  && captured.every(record => record.method === 'POST' && record.path === '/v1/chat/completions' && record.body?.model === expected.modelId)
console.log(pass
  ? 'PASS  the recorded session resumes and dials POST <baseUrl>/chat/completions with the configured model'
  : `FAIL  resumed outcome=${outcome}, captured=${captured.length} request(s); compare the lines above`)
process.exitCode = pass ? 0 : 1
