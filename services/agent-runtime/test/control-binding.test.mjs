import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { fileURLToPath } from 'node:url'
import { validatePromptControl } from '../src/control-binding.mjs'
import { startRuntimeProcess } from './runtime-contract-suite.mjs'

function requestFixture() {
  const permission = { mode: 'ask', projectRoot: null, grants: [] }
  return {
    runId: 'frozen-run', conversationId: 'frozen-conversation', runtimeSessionId: 'frozen-session',
    payload: { text: 'hello', controlBinding: {
      schemaVersion: 1, runId: 'frozen-run', conversationId: 'frozen-conversation',
      engineId: 'pi', executionProfileId: 'legacy', authority: 'legacy', readOnlyExecutor: 'runtime',
      permissionSnapshotId: `sha256:${createHash('sha256').update(JSON.stringify(permission)).digest('hex')}`,
      permission,
      budgets: { modelRequestMs: 120_000, toolExecutionMs: 600_000, runExecutionMs: 1_800_000, approvalWaitMs: 300_000 },
    } },
  }
}

test('frozen prompt control accepts the matching adapter and protocol v1 absence only', () => {
  assert.deepEqual(validatePromptControl(requestFixture(), 'legacy'), { projectRoot: null, permissionMode: 'ask' })
  assert.equal(validatePromptControl({ payload: {} }, 'legacy'), null)
  assert.throws(() => validatePromptControl({ payload: { controlBinding: null } }, 'legacy'), /Invalid frozen/)
  assert.throws(() => validatePromptControl({ payload: { controlBinding: {} } }, 'legacy'), /Invalid frozen/)
})

test('frozen prompt control rejects identity, schema, policy and budget drift', () => {
  for (const [field, value] of [
    ['engineId', 'codex'], ['engineId', 'deepseek_harness'], ['authority', 'authoritative'],
    ['runId', 'foreign-run'], ['conversationId', 'foreign-conversation'], ['executionProfileId', 'durable_v2'],
    ['schemaVersion', 2], ['permissionSnapshotId', 'tampered'], ['readOnlyExecutor', 'unknown'],
  ]) {
    const request = requestFixture()
    request.payload.controlBinding[field] = value
    assert.throws(() => validatePromptControl(request, 'legacy'), undefined, field)
  }
  for (const budget of [0, -1, 86_400_001, 1.5]) {
    const request = requestFixture()
    request.payload.controlBinding.budgets.approvalWaitMs = budget
    assert.throws(() => validatePromptControl(request, 'legacy'), /budget|invalid type/)
  }
  const request = requestFixture()
  request.payload.projectContext = { projectRoot: null, permissionMode: 'allow' }
  assert.throws(() => validatePromptControl(request, 'legacy'), /disagrees/)
})

test('frozen permission hashing uses Rust field order rather than wire property order', () => {
  const request = requestFixture()
  request.payload.controlBinding.permission = { grants: [], projectRoot: null, mode: 'ask' }
  assert.deepEqual(validatePromptControl(request, 'legacy'), { projectRoot: null, permissionMode: 'ask' })
  request.payload.controlBinding.permission.mode = 'allow'
  assert.throws(() => validatePromptControl(request, 'legacy'), /hash mismatch/)
})

test('real Pi process rejects foreign control before model events and accepts valid frozen control', async context => {
  const runtime = startRuntimeProcess(fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url)))
  context.after(() => runtime.close())
  const initialize = runtime.send('initialize', { payload: { modelService: {
    baseUrl: 'faux://fox', modelId: 'fox-test', apiType: 'faux', contextWindow: 4096, maxOutputTokens: 512,
  } } })
  await runtime.waitFor(message => message.requestId === initialize.id && message.type === 'ready')
  const request = requestFixture()
  const create = runtime.send('create_session', {
    conversationId: request.conversationId, runtimeSessionId: request.runtimeSessionId, payload: {},
  })
  await runtime.waitFor(message => message.requestId === create.id && message.type === 'session_created')
  for (const [field, value] of [['engineId', 'codex'], ['authority', 'authoritative'], ['executionProfileId', 'durable_v2']]) {
    const invalid = requestFixture()
    invalid.payload.controlBinding[field] = value
    const prompt = runtime.send('prompt', invalid)
    const response = await runtime.waitFor(message => message.requestId === prompt.id)
    assert.equal(response.type, 'request_failed')
    assert.equal(runtime.messages.some(message => message.runId === request.runId && message.type === 'runtime_event'), false)
  }
  const wrongSession = requestFixture()
  wrongSession.runtimeSessionId = 'missing-session'
  const invalid = runtime.send('prompt', wrongSession)
  assert.equal((await runtime.waitFor(message => message.requestId === invalid.id)).type, 'request_failed')
  const prompt = runtime.send('prompt', request)
  assert.equal((await runtime.waitFor(message => message.requestId === prompt.id)).type, 'request_succeeded')
  await runtime.waitFor(message => message.runId === request.runId && message.payload?.type === 'run.completed')
})
