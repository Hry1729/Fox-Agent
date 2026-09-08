// Controlled transport fixture: real pi-runtime process + real Pi loop,
// deterministic local faux provider. No network provider or mutation tools.
import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { startRuntimeProcess } from '../runtime-contract-suite.mjs'

const [conversationId, runId, scenario = 'read_batch'] = process.argv.slice(2)
if (!conversationId || !runId) throw new Error('conversationId and runId are required')
const directory = await mkdtemp(join(tmpdir(), 'fox-shadow-pi-process-'))
const runtime = startRuntimeProcess(fileURLToPath(new URL('../../src/pi-runtime.mjs', import.meta.url)))
const timer = setTimeout(() => { runtime.close(); process.exitCode = 1 }, 20_000)
const path = fileURLToPath(new URL('../../src/protocol.mjs', import.meta.url))
const call = (id, path) => ({ type: 'toolCall', id, name: 'read', arguments: { path } })
const response = (calls) => ({ content: calls, stopReason: 'toolUse' })
try {
  const responses = scenario === 'no_tool' ? ['A controlled direct answer.'] : [
    response([call('pi-read-1', path), call('pi-read-2', scenario === 'tool_error' ? join(directory, 'missing.txt') : fileURLToPath(new URL('../../src/tool-adapter.mjs', import.meta.url)))]),
    response([call('pi-read-3', fileURLToPath(new URL('../../src/model-profile.mjs', import.meta.url)))]),
    'The controlled tool rounds are complete.',
  ]
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux',
    contextWindow: 8192, maxOutputTokens: 1024, fauxResponses: responses,
  } } })
  await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  const runtimeSessionId = `shadow-session-${runId}`
  const create = runtime.send('create_session', { conversationId, runtimeSessionId, payload: { sessionPath: join(directory, 'session.json') } })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')
  runtime.send('prompt', { conversationId, runtimeSessionId, runId, payload: {
    text: 'Execute the scripted inspection rounds.', messages: [{ role: 'user', content: 'Execute the scripted inspection rounds.' }],
    projectContext: { projectRoot: directory, permissionMode: 'read_only' },
  } })
  if (scenario !== 'no_tool') {
    for (const id of ['pi-read-1', 'pi-read-2', 'pi-read-3']) {
      const request = await runtime.waitFor((message) => message.kind === 'request' && message.type === 'tool.preflight' && message.payload?.toolCallId === id)
      // This fixture stands in for the Legacy read-only preflight boundary.
      // The actual execution stays in Pi's shipped read-only tool implementation.
      const approved = { decision: 'allow', input: request.payload.input }
      runtime.messages.push({ kind: 'shadow_preflight', runId, toolCallId: id, tool: request.payload.tool, approved })
      runtime.respond(request, 'tool.preflight.completed', approved)
    }
  }
  await runtime.waitFor((message) => message.runId === runId && ['run.completed', 'run.failed'].includes(message.payload?.type), 15_000)
  process.stdout.write(JSON.stringify({ scenario, messages: runtime.messages }))
} finally {
  clearTimeout(timer)
  runtime.close()
  await rm(directory, { recursive: true, force: true })
}
