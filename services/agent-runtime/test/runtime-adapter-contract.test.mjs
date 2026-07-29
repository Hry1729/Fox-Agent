import test from 'node:test'
import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'
import {
  runCapabilityDegradationContract,
  runCancellationContract,
  runRuntimeAdapterContract,
  runSessionResumeContract,
  runToolApprovalContract,
  startRuntimeProcess,
} from './runtime-contract-suite.mjs'

const fakeRuntimePath = fileURLToPath(new URL('../src/fake-runtime.mjs', import.meta.url))
const piRuntimePath = fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url))

function piConfig(overrides = {}) {
  return {
    modelService: {
      baseUrl: 'faux://fox',
      modelId: 'fox-contract',
      apiType: 'faux',
      contextWindow: 4096,
      maxOutputTokens: 512,
      supportsImageInput: false,
      ...overrides,
    },
  }
}

test('Fake Runtime satisfies the Fox Runtime Adapter contract', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-fake-contract-'))
  const runtime = startRuntimeProcess(fakeRuntimePath)
  context.after(() => runtime.close())
  context.after(() => rm(directory, { recursive: true, force: true }))
  const ready = await runRuntimeAdapterContract({
    runtime,
    initializePayload: {},
    conversationId: 'fake-contract-conversation',
    runtimeSessionId: 'fake-contract-session',
    sessionPath: join(directory, 'session.json'),
    runId: 'fake-contract-run',
  })
  assert.equal(ready.capabilities.reasoning, false)
  assert.deepEqual(ready.capabilities.tools, [])
})

test('Pi Runtime satisfies the Fox Runtime Adapter contract', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-pi-contract-'))
  const runtime = startRuntimeProcess(piRuntimePath)
  context.after(() => runtime.close())
  context.after(() => rm(directory, { recursive: true, force: true }))
  const ready = await runRuntimeAdapterContract({
    runtime,
    initializePayload: piConfig(),
    conversationId: 'pi-contract-conversation',
    runtimeSessionId: 'pi-contract-session',
    sessionPath: join(directory, 'session.json'),
    runId: 'pi-contract-run',
  })
  assert.equal(ready.capabilities.reasoning, true)
  assert.ok(ready.capabilities.tools.some(({ name }) => name === 'read_attachment'))
})

for (const adapter of [
  { name: 'Fake Runtime', runtimePath: fakeRuntimePath, initializePayload: {} },
  {
    name: 'Pi Runtime',
    runtimePath: piRuntimePath,
    initializePayload: piConfig({
      fauxTokensPerSecond: 5,
      fauxResponses: ['This deliberately long response gives the cancellation contract enough time to stop the active runtime before completion.'],
    }),
  },
]) {
  test(`${adapter.name} cancels an active run with one terminal event`, async (context) => {
    const directory = await mkdtemp(join(tmpdir(), 'fox-cancel-contract-'))
    const runtime = startRuntimeProcess(adapter.runtimePath)
    context.after(() => runtime.close())
    context.after(() => rm(directory, { recursive: true, force: true }))
    await runCancellationContract({
      runtime,
      initializePayload: adapter.initializePayload,
      conversationId: `${adapter.name}-cancel-conversation`,
      runtimeSessionId: `${adapter.name}-cancel-session`,
      sessionPath: join(directory, 'session.json'),
      runId: `${adapter.name}-cancel-run`,
    })
  })

  test(`${adapter.name} resumes a persisted session after process restart`, async (context) => {
    const directory = await mkdtemp(join(tmpdir(), 'fox-resume-contract-'))
    context.after(() => rm(directory, { recursive: true, force: true }))
    await runSessionResumeContract({
      startRuntime: () => startRuntimeProcess(adapter.runtimePath),
      initializePayload: adapter.name === 'Pi Runtime' ? piConfig() : adapter.initializePayload,
      conversationId: `${adapter.name}-resume-conversation`,
      runtimeSessionId: `${adapter.name}-resume-session`,
      sessionPath: join(directory, 'session.json'),
    })
  })
}

test('Pi Runtime pauses a protected tool until Fox approves it', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-approval-contract-'))
  const approvedFile = join(directory, 'approved.txt')
  await writeFile(approvedFile, 'approved contract content', 'utf8')
  const runtime = startRuntimeProcess(piRuntimePath)
  context.after(() => runtime.close())
  context.after(() => rm(directory, { recursive: true, force: true }))
  await runToolApprovalContract({
    runtime,
    initializePayload: piConfig({
      fauxResponses: [
        {
          content: [{ type: 'toolCall', id: 'contract-read', name: 'read', arguments: { path: approvedFile } }],
          stopReason: 'toolUse',
        },
        'The approved file was read.',
      ],
    }),
    conversationId: 'pi-approval-conversation',
    runtimeSessionId: 'pi-approval-session',
    sessionPath: join(directory, 'session.json'),
    runId: 'pi-approval-run',
    expectedPath: approvedFile,
  })
})

test('Fake Runtime degrades cleanly when optional capabilities are absent', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-degraded-contract-'))
  const runtime = startRuntimeProcess(fakeRuntimePath)
  context.after(() => runtime.close())
  context.after(() => rm(directory, { recursive: true, force: true }))
  await runCapabilityDegradationContract({
    runtime,
    initializePayload: {},
    conversationId: 'fake-degraded-conversation',
    runtimeSessionId: 'fake-degraded-session',
    sessionPath: join(directory, 'session.json'),
    runId: 'fake-degraded-run',
  })
})

test('Pi Runtime advertises and accepts image input only when the model enables it', async (context) => {
  const directory = await mkdtemp(join(tmpdir(), 'fox-image-contract-'))
  const runtime = startRuntimeProcess(piRuntimePath)
  context.after(() => runtime.close())
  context.after(() => rm(directory, { recursive: true, force: true }))
  const initialize = runtime.send('initialize', {
    payload: piConfig({ supportsImageInput: true, fauxResponses: ['Image input accepted.'] }),
  })
  const ready = await runtime.waitFor((message) => message.requestId === initialize.id && message.type === 'ready')
  assert.equal(ready.payload.capabilities.imageInput, true)
  const runtimeSessionId = 'pi-image-session'
  const conversationId = 'pi-image-conversation'
  const create = runtime.send('create_session', {
    conversationId,
    runtimeSessionId,
    payload: { sessionPath: join(directory, 'session.json') },
  })
  await runtime.waitFor((message) => message.requestId === create.id && message.type === 'session_created')
  const runId = 'pi-image-run'
  runtime.send('prompt', {
    conversationId,
    runtimeSessionId,
    runId,
    payload: {
      text: 'describe this image',
      images: [{ type: 'image', mimeType: 'image/png', data: 'iVBORw0KGgo=' }],
      messages: [{ role: 'user', content: 'describe this image' }],
    },
  })
  await runtime.waitFor((message) => message.runId === runId && message.payload?.type === 'run.completed')
})
