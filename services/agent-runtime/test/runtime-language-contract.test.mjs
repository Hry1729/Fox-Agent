// Item 4 acceptance: the user-facing Simplified Chinese rule must exist in the
// ONE prompt fragment every assembly entry reuses, and it must still be present
// byte-for-byte in the provider request of every later Kernel round (after tool
// execution, after a Host continuation review, and on the post-compaction path).
//
// These tests use the production composers and a real HTTP worker process, so
// they observe the actual outgoing request bodies rather than a re-implementation.
import test from 'node:test'
import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import { createServer } from 'node:http'
import { createEnvelope } from '../src/protocol.mjs'
import { canonicalPermission } from '../src/control-binding.mjs'
import { FOX_LANGUAGE_CONTRACT, composeRuntimePrompt } from '../src/runtime-instructions.mjs'
import { describeKernelRun } from '../src/pi-kernel-description.mjs'

const identity = { runId: 'run-lang', conversationId: 'conversation-lang', runtimeSessionId: 'session-lang' }
const CONTINUATION_PROMPT = '请继续核对剩余工作，并说明是否已经完成。'

function productionPrompt(modelService, { supportedTools = ['read'], workSnapshot = emptyWorkSnapshot() } = {}) {
  const described = describeKernelRun({
    conversationId: identity.conversationId,
    payload: {
      executionProfileId: 'legacy',
      modelService,
      supportedTools,
      prompt: {
        systemPrompt: 'Host-selected base assistant',
        projectContext: { projectRoot: null, permissionMode: 'ask' },
        workSnapshot,
      },
    },
  })
  return described
}

const emptyWorkSnapshot = () => ({ schemaVersion: 1, goal: null, tasks: [], evidence: [] })

function firstSystemText(body) {
  const message = (body.messages ?? []).find(item => typeof item.content === 'string' && item.content.trim())
  return typeof message?.content === 'string' ? message.content : ''
}

test('the production Kernel prompt carries the language contract in its stable prefix', () => {
  const modelService = { apiType: 'openai-completions', modelId: 'language-contract-test',
    baseUrl: 'http://127.0.0.1:1/v1', apiKey: 'local-test-only' }
  const described = productionPrompt(modelService)
  assert.equal(described.systemPrompt.includes(FOX_LANGUAGE_CONTRACT), true,
    'the Kernel describe handshake must ship the rule the Host freezes for the Run')
  assert.match(described.systemPrompt, /Simplified Chinese \(简体中文\)/)

  // The same fragment is first in the stable prefix of every composer entry that
  // renders the shared runtime block (Legacy runtime, evaluation harness).
  const composed = composeRuntimePrompt({ systemPrompt: 'Base' })
  assert.equal(composed.prompt.slice(0, composed.diagnostics.stableChars).includes(FOX_LANGUAGE_CONTRACT), true)
  assert.ok(composed.prompt.startsWith(FOX_LANGUAGE_CONTRACT))
})

test('every Kernel round re-sends the identical language contract: tool follow-up, continuation, final', { timeout: 60000 }, async t => {
  const requests = []
  const server = createServer(async (req, res) => {
    let raw = ''
    for await (const chunk of req) raw += chunk
    const body = JSON.parse(raw)
    const round = requests.push(body)
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    const send = delta => res.write(`data: ${JSON.stringify({ id: `lang-${round}`, object: 'chat.completion.chunk',
      created: 1, model: body.model, choices: [{ index: 0, delta, finish_reason: null }] })}\n\n`)
    // Rounds 1 and 2 propose a tool so the follow-up requests carry settled tool
    // results; round 3 stops without tools and earns a Host continuation review;
    // round 4 is the final answer.
    if (round <= 2) {
      send({ role: 'assistant', content: '正在读取文件。' })
      send({ role: 'assistant', tool_calls: [{ index: 0, id: `read-${round}`, type: 'function',
        function: { name: 'read', arguments: '{"path":"a.txt"}' } }] })
    } else {
      send({ role: 'assistant', content: round === 3 ? '已读取文件，准备核对。' : '核对完成，未发现异常。' })
    }
    res.write(`data: ${JSON.stringify({ id: `lang-${round}`, object: 'chat.completion.chunk', created: 1, model: body.model,
      choices: [{ index: 0, delta: {}, finish_reason: round <= 2 ? 'tool_calls' : 'stop' }] })}\n\n`)
    res.end('data: [DONE]\n\n')
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { server.closeAllConnections(); server.close() })

  const modelService = { apiType: 'openai-completions', modelId: 'language-contract-test',
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`, apiKey: 'local-test-only' }
  const described = productionPrompt(modelService)

  const onRoundOutput = frame => {
    const round = child.roundOutputs.length
    if (frame.assistantMessage.stopReason === 'toolUse') {
      const call = frame.assistantMessage.content.find(block => block.type === 'toolCall')
      return { schemaVersion: 1, kind: 'batch', batchId: `host-batch-${round}`, checkpointSeq: 20 + round, tools: [
        { toolCallId: call.id, tool: 'read', sourceOrder: 0, canonicalInput: call.arguments,
          state: 'completed', result: { content: [{ type: 'text', text: '主机已结算的文件内容' }] } }] }
    }
    return round === 3
      ? { schemaVersion: 1, kind: 'continuation', prompt: CONTINUATION_PROMPT, previewSeq: 40 }
      : { schemaVersion: 1, kind: 'final' }
  }

  const child = await worker(t, { onRoundOutput })
  const ready = await child.request('kernel.initialize', {
    executionProfileId: 'legacy', systemPrompt: described.systemPrompt, proposalTools: described.proposalTools,
    modelService, execution: 'loop',
  })
  assert.equal(ready.type, 'kernel.ready')
  const payload = resumePayload()
  const result = await child.request('kernel.resume_batch', payload)
  assert.equal(result.type, 'kernel.model_response', JSON.stringify(result))
  assert.equal(result.payload.response.assistantMessage.content.find(block => block.type === 'text').text, '核对完成，未发现异常。')
  assert.equal(requests.length, 4, 'two tool rounds, one Host continuation review, one final round')

  // Every provider request of the Run carries the frozen rule verbatim, and the
  // stable prefix does not drift between rounds.
  const prefixes = requests.map(firstSystemText)
  for (const [index, prefix] of prefixes.entries()) {
    assert.equal(prefix.includes(FOX_LANGUAGE_CONTRACT), true, `round ${index + 1} must carry the language contract`)
  }
  assert.equal(new Set(prefixes).size, 1, 'the stable prefix must be identical in every round')

  // Prove the rounds really are the post-tool and post-continuation requests.
  // Rounds 2 and 3 run after a settled Host tool result; round 4 is the Host
  // continuation review call that carries the reviewer prompt.
  assert.ok(requests[1].messages.some(message => message.role === 'tool'),
    'round 2 is the tool follow-up call that carries the settled Host result')
  assert.ok(requests[2].messages.some(message => message.role === 'tool'),
    'round 3 is the second tool follow-up call')
  assert.ok(JSON.stringify(requests[3].messages).includes(CONTINUATION_PROMPT),
    'round 4 is the Host continuation review call')
})

test('the compaction request and the post-compaction describe keep the same rule', { timeout: 60000 }, async t => {
  const requests = []
  const server = createServer(async (req, res) => {
    let raw = ''
    for await (const chunk of req) raw += chunk
    const body = JSON.parse(raw)
    requests.push(body)
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    res.write(`data: ${JSON.stringify({ id: 'summary-1', object: 'chat.completion.chunk', created: 1, model: body.model,
      choices: [{ index: 0, delta: { role: 'assistant', content: '保留约束与尚未验证的执行状态。' }, finish_reason: null }] })}\n\n`)
    res.write(`data: ${JSON.stringify({ id: 'summary-1', object: 'chat.completion.chunk', created: 1, model: body.model,
      choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] })}\n\n`)
    res.end('data: [DONE]\n\n')
  })
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  t.after(() => { server.closeAllConnections(); server.close() })

  const modelService = { apiType: 'openai-completions', modelId: 'language-compaction-test',
    baseUrl: `http://127.0.0.1:${server.address().port}/v1`, apiKey: 'local-test-only' }
  const described = productionPrompt(modelService, { supportedTools: [] })

  const child = await worker(t, {})
  const ready = await child.request('kernel.initialize', {
    executionProfileId: 'legacy', systemPrompt: described.systemPrompt, proposalTools: [], modelService,
  })
  assert.equal(ready.type, 'kernel.ready')
  const compacted = await child.request('kernel.compact_context', compactionPayload())
  assert.equal(compacted.type, 'kernel.compaction_result')
  assert.equal(requests.length, 1)
  // The summarizer round runs inside the same frozen-prompt session: the rule is
  // still delivered, and the notes it produces inherit the same conversation
  // language. A Post-compaction round in the Host starts a fresh worker from a
  // new describe handshake, which productionPrompt above proves carries it too.
  assert.equal(firstSystemText(requests[0]).includes(FOX_LANGUAGE_CONTRACT), true)
})

function compactionPayload() {
  return { controlBinding: resumePayload().controlBinding, compaction: { schemaVersion: 1, runId: identity.runId,
    turnId: 'turn-lang', compactionId: 'summary-lang', inputHash: `sha256:${'b'.repeat(64)}`,
    messages: [{ role: 'user', content: 'Earlier request: keep the constraints.' }], maxSummaryBytes: 1024 } }
}

function resumePayload() {
  const permission = { mode: 'read_only', projectRoot: null, grants: [] }
  return { controlBinding: { schemaVersion: 1, runId: identity.runId, conversationId: identity.conversationId,
    engineId: 'pi', executionProfileId: 'legacy', authority: 'authoritative', readOnlyExecutor: 'rust', permission,
    permissionSnapshotId: `sha256:${createHash('sha256').update(JSON.stringify(canonicalPermission(permission))).digest('hex')}`,
    budgets: { modelRequestMs: 120000, toolExecutionMs: 600000, runExecutionMs: 1800000, approvalWaitMs: 300000 } },
    batchResume: { schemaVersion: 1, turnId: 'turn-lang', batchId: 'batch-lang',
      idempotencyKey: 'tool-batch-delivery:batch-lang', checkpointSeq: 8,
      history: [{ role: 'user', content: '请读取 a.txt 并核对内容。', timestamp: 1 }],
      assistantMessage: { role: 'assistant', stopReason: 'toolUse', timestamp: 2,
        content: [{ type: 'toolCall', id: 'read-0', name: 'read', arguments: { path: 'a.txt' } }] },
      tools: [{ toolCallId: 'read-0', tool: 'read', sourceOrder: 0, canonicalInput: { path: 'a.txt' },
        state: 'completed', result: { content: [{ type: 'text', text: '主机已结算的文件内容' }] } }] } }
}

// Minimal Kernel worker harness: a real isolated process over the wire protocol.
async function worker(t, { onRoundOutput } = {}) {
  const child = spawn(process.execPath, [fileURLToPath(new URL('../src/pi-runtime.mjs', import.meta.url)), '--kernel-worker'],
    { stdio: ['pipe', 'pipe', 'pipe'], windowsHide: true })
  const pending = new Map()
  const events = []
  const roundOutputs = []
  let output = ''
  let stderr = ''
  const answerRoundOutput = onRoundOutput ?? (() => ({ schemaVersion: 1, kind: 'final' }))
  child.stderr.on('data', chunk => { stderr += chunk })
  child.stdout.on('data', chunk => {
    output += chunk
    let end
    while ((end = output.indexOf('\n')) >= 0) {
      const line = output.slice(0, end)
      output = output.slice(end + 1)
      const message = JSON.parse(line)
      events.push(message)
      if (message.kind === 'request' && message.type === 'kernel.round_output') {
        roundOutputs.push(message.payload)
        const directive = answerRoundOutput(message.payload)
        child.stdin.write(`${JSON.stringify(createEnvelope('response', 'kernel.round_directive', {
          requestId: message.id, runId: message.runId, conversationId: message.conversationId,
          runtimeSessionId: message.runtimeSessionId, payload: directive }))}\n`)
        continue
      }
      if (message.type === 'kernel.model_preview' || message.type === 'kernel.usage_record') continue
      pending.get(message.requestId)?.(message)
      pending.delete(message.requestId)
    }
  })
  const exited = new Promise(resolve => child.once('exit', resolve))
  child.on('error', error => { stderr += error.message })
  t.after(async () => {
    child.stdin.end()
    child.kill()
    await exited
    assert.equal(stderr, '')
  })
  return {
    events,
    roundOutputs,
    request(type, payload = {}) {
      const message = createEnvelope('request', type, { ...identity, payload })
      return new Promise((resolve, reject) => {
        const timeout = setTimeout(() => { pending.delete(message.id); reject(new Error(`No response for ${type}: ${stderr}`)) }, 30000)
        pending.set(message.id, result => { clearTimeout(timeout); resolve(result) })
        child.stdin.write(`${JSON.stringify(message)}\n`)
      })
    },
  }
}
