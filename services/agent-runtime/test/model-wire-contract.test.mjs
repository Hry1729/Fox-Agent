// O-D-02 — the contract behind C04's `sent` layer.
//
// C04 predicts request parameters from a declared wire vocabulary. A prediction is only worth
// anything if the vocabulary matches what the production adapters really serialize, so this file
// captures the payloads those adapters build and holds the table to them. The capture uses the
// official StreamOptions.onPayload hook plus a stubbed fetch: no socket is opened, no credential
// is used and no provider is contacted. Evidence level: offline_adapter_capture.
//
// If an adapter ever emits a field the table does not declare, these tests fail. That direction
// matters: a table narrower than the real wire would make C04 report a field production sends as
// "unsupported", which is the drift this file exists to prevent.
import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { streamSimple as completionsStreamSimple } from '@earendil-works/pi-ai/api/openai-completions'
import { streamSimple as responsesStreamSimple } from '@earendil-works/pi-ai/api/openai-responses'
import { streamSimple as anthropicStreamSimple } from '@earendil-works/pi-ai/api/anthropic-messages'
import {
  ADAPTER_UNSENT_FIELDS,
  API_PARAMETER_FIELDS,
  CONFIG_EVIDENCE_LEVEL,
  SAMPLING_PASSTHROUGH_FIELDS,
  TOKEN_LIMIT_FIELDS,
  WIRE_VOCABULARY_SOURCES,
  buildRequestParameters,
  resolveModelProfile,
  transportProvider,
} from '../src/model-profile.mjs'
import { openNativeModelProxy } from '../src/native-model-proxy.mjs'

const ADAPTER_VERSION = JSON.parse(
  readFileSync(new URL('../node_modules/@earendil-works/pi-ai/package.json', import.meta.url), 'utf8'),
).version

/** A minimal stream that satisfies every adapter's reader without producing a payload of its own. */
const SSE_BODY =
  'data: {"id":"wire-contract","object":"chat.completion.chunk","created":1,"model":"wire-contract",' +
  '"choices":[{"index":0,"delta":{"content":"ok"},"finish_reason":"stop"}]}\n\ndata: [DONE]\n\n'

const BASE_CONTEXT = Object.freeze({
  messages: [{ role: 'user', content: 'offline wire contract' }],
})

const TOOL_CONTEXT = Object.freeze({
  systemPrompt: 'offline wire contract system prompt',
  messages: [{ role: 'user', content: 'offline wire contract' }],
  tools: [{ name: 'read', description: 'read a file', parameters: { type: 'object', properties: { path: { type: 'string' } } } }],
})

/**
 * Build the model exactly the way production does: pi-runtime.mjs::createModel and
 * pi-kernel-worker.mjs both derive it from resolveModelProfile + transportProvider.
 */
function foxModel(config) {
  const profile = resolveModelProfile(config)
  return {
    profile,
    model: {
      id: config.modelId,
      name: config.modelId,
      api: profile.api,
      provider: transportProvider(profile),
      baseUrl: config.baseUrl,
      reasoning: profile.reasoning,
      input: profile.supportsImageInput ? ['text', 'image'] : ['text'],
      cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
      contextWindow: profile.contextWindow,
      maxTokens: profile.maxOutputTokens,
      ...(Object.keys(profile.compat).length > 0 ? { compat: profile.compat } : {}),
    },
  }
}

async function captureViaAdapter(streamFn, config, { context = BASE_CONTEXT, options = {} } = {}) {
  const { model, profile } = foxModel(config)
  const payloads = []
  const errors = []
  try {
    const stream = streamFn(model, context, {
      apiKey: 'offline-contract-test-not-a-credential',
      maxTokens: 512,
      ...options,
      onPayload: (payload) => { payloads.push(payload); return undefined },
      fetch: async () => new Response(SSE_BODY, { status: 200, headers: { 'content-type': 'text/event-stream' } }),
    })
    for await (const _ of stream) { /* the payload is captured before the request is issued */ }
  } catch (error) {
    errors.push(String(error?.message ?? error))
  }
  return { profile, payloads, errors }
}

function assertDeclared(api, payload, label) {
  const declared = [...API_PARAMETER_FIELDS[api], ...(SAMPLING_PASSTHROUGH_FIELDS[api] ?? [])]
  for (const key of Object.keys(payload)) {
    assert.ok(declared.includes(key), `${label}: the adapter serialized "${key}", which the ${api} table does not declare`)
    assert.ok(
      !(ADAPTER_UNSENT_FIELDS[api] ?? []).includes(key),
      `${label}: "${key}" is listed as never serialized, but the adapter emitted it`,
    )
  }
}

test(`O-D-02: the openai-completions adapter serializes only declared fields (pi-ai ${ADAPTER_VERSION})`, async () => {
  const matrix = [
    { config: { modelId: 'gpt-5', baseUrl: 'https://api.openai.com/v1', apiType: 'openai-completions' }, options: { reasoning: 'high' } },
    { config: { modelId: 'gpt-5', baseUrl: 'https://api.openai.com/v1', apiType: 'openai-completions' }, options: {} },
    { config: { modelId: 'deepseek-reasoner', baseUrl: 'https://api.deepseek.com', apiType: 'openai-completions' }, options: { reasoning: 'medium' } },
    { config: { modelId: 'qwen3-thinking', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', apiType: 'openai-completions' }, options: { reasoning: 'medium' } },
    { config: { modelId: 'some-model', baseUrl: 'https://openrouter.ai/api/v1', apiType: 'openai-completions' }, options: {} },
    { config: { modelId: 'custom-chat-model', baseUrl: 'https://example.test/v1', apiType: 'openai-completions' }, options: { toolChoice: 'auto', samplingParams: { top_p: 0.9 } }, context: TOOL_CONTEXT },
  ]
  const observed = new Set()
  for (const entry of matrix) {
    const label = `${entry.config.modelId} @ ${entry.config.baseUrl}`
    const { profile, payloads, errors } = await captureViaAdapter(completionsStreamSimple, entry.config, { context: entry.context, options: entry.options })
    assert.deepEqual(errors, [], `${label}: the capture must not need a real endpoint`)
    assert.equal(payloads.length, 1, `${label}: exactly one payload must be captured`)
    const payload = payloads[0]
    assertDeclared('openai-completions', payload, label)
    for (const key of Object.keys(payload)) observed.add(key)
    assert.equal(payload.model, entry.config.modelId)
    assert.equal(payload.stream, true)

    // The budget field the adapter really used is the one C04 predicts for this profile.
    const limitField = TOKEN_LIMIT_FIELDS.find((field) => field in payload)
    assert.equal(
      limitField,
      buildRequestParameters(profile, {}).maxTokensField,
      `${label}: C04 predicts ${buildRequestParameters(profile, {}).maxTokensField} but the adapter wrote ${limitField}`,
    )
    // And a value C04 would send is a value the adapter can carry: same name, legal integer.
    const asBuilt = buildRequestParameters(profile, { maxOutputTokens: 512 })
    assert.equal(asBuilt.sent[asBuilt.maxTokensField], 512)
    assert.ok(limitField in { [asBuilt.maxTokensField]: true }, `${label}: field names must agree`)
  }
  // The capture has to be broad enough to be worth something: these shapes must have been seen.
  for (const required of ['max_tokens', 'max_completion_tokens', 'prompt_cache_key', 'reasoning_effort', 'store', 'tools', 'tool_choice', 'thinking', 'top_p']) {
    assert.ok(observed.has(required), `the capture never exercised "${required}", so the table is unproven for it`)
  }
})

test(`O-D-02: the openai-responses adapter serializes only declared fields (pi-ai ${ADAPTER_VERSION})`, async () => {
  const capture = await captureViaAdapter(responsesStreamSimple, {
    modelId: 'gpt-5',
    baseUrl: 'https://api.openai.com/v1',
    apiType: 'openai-responses',
  }, { options: { reasoning: 'high' } })
  assert.deepEqual(capture.errors, [])
  assert.equal(capture.payloads.length, 1)
  const payload = capture.payloads[0]
  assertDeclared('openai-responses', payload, 'gpt-5 @ responses')
  assert.equal(payload.model, 'gpt-5')
  assert.equal(payload.stream, true)
  // Fox's own serializer for this shape picks the same field, which is what C04 predicts.
  assert.equal(buildRequestParameters(capture.profile, {}).maxTokensField, 'max_output_tokens')
})

test(`O-D-02: the anthropic-messages adapter serializes only declared fields (pi-ai ${ADAPTER_VERSION})`, async () => {
  const capture = await captureViaAdapter(anthropicStreamSimple, {
    modelId: 'claude-sonnet-4-5',
    baseUrl: 'https://api.anthropic.com',
    apiType: 'anthropic-messages',
  }, { context: TOOL_CONTEXT, options: { reasoning: 'medium' } })
  assert.deepEqual(capture.errors, [])
  assert.equal(capture.payloads.length, 1)
  const payload = capture.payloads[0]
  assertDeclared('anthropic-messages', payload, 'claude-sonnet-4-5 @ anthropic')
  assert.equal(payload.model, 'claude-sonnet-4-5')
  assert.equal(payload.stream, true)
  assert.equal(buildRequestParameters(capture.profile, {}).maxTokensField, 'max_tokens')
  // `top_p` is not part of this shape's vocabulary and is not a sampling-passthrough channel here.
  assert.equal((SAMPLING_PASSTHROUGH_FIELDS['anthropic-messages'] ?? []).includes('top_p'), false)
  assert.equal(
    buildRequestParameters(capture.profile, { top_p: 0.9 }).rejected.find((entry) => entry.field === 'top_p').reason,
    'unsupported_by_api_shape',
  )
})

test('O-D-02: Fox\'s own responses serializer agrees with the declared vocabulary', async () => {
  const upstream = []
  const server = createServer(async (req, res) => {
    let input = ''
    for await (const chunk of req) input += chunk
    upstream.push(JSON.parse(input))
    res.writeHead(200, { 'content-type': 'text/event-stream' })
    res.end(`data: ${JSON.stringify({ type: 'response.completed', response: { status: 'completed', output: [] } })}\n\n`)
  })
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve))
  const profile = resolveModelProfile({ modelId: 'gpt-5', baseUrl: 'https://api.openai.com/v1', apiType: 'openai-responses' })
  const proxy = await openNativeModelProxy(
    { modelId: 'gpt-5', baseUrl: `http://127.0.0.1:${server.address().port}`, maxOutputTokens: profile.maxOutputTokens },
    'offline-contract-test-not-a-credential',
    undefined,
    [{ name: 'read', description: 'read', parameters: { type: 'object' } }],
  )
  try {
    const response = await fetch(`${proxy.baseUrl}/responses`, {
      method: 'POST',
      headers: { authorization: `Bearer ${proxy.secret}` },
      body: JSON.stringify({ model: 'gpt-5', stream: true }),
    })
    assert.equal(response.status, 200)
    await response.text()
    assert.equal(upstream.length, 1)
    const body = upstream[0]
    // Fields Fox writes itself on this shape must be declared for it.
    assertDeclared('openai-responses', body, 'fox native proxy upstream')
    const budgetField = TOKEN_LIMIT_FIELDS.find((field) => field in body)
    assert.equal(budgetField, buildRequestParameters(profile, {}).maxTokensField)
    assert.ok(Number.isInteger(body[budgetField]) && body[budgetField] > 0, JSON.stringify(body))
    assert.equal(body.store, false)
  } finally {
    await proxy.close()
    server.closeAllConnections()
    await new Promise((resolve) => server.close(resolve))
  }
})

test('O-D-02: a field the adapter never sends is reported as such, not silently allowed', async () => {
  const capture = await captureViaAdapter(completionsStreamSimple, {
    modelId: 'gpt-5',
    baseUrl: 'https://api.openai.com/v1',
    apiType: 'openai-completions',
  }, { options: { reasoning: 'high' } })
  const payload = capture.payloads[0]
  for (const field of ADAPTER_UNSENT_FIELDS['openai-completions']) {
    assert.equal(field in payload, false, `"${field}" is listed as never serialized but the adapter emitted it`)
    const built = buildRequestParameters(capture.profile, { [field]: true })
    assert.equal(built.rejected.find((entry) => entry.field === field).reason, 'not_serialized_by_the_adapter')
    assert.equal(field in built.sent, false)
  }
})

test('O-D-02: the shape carries a thinking vocabulary this module still refuses to invent', async () => {
  const capture = await captureViaAdapter(completionsStreamSimple, {
    modelId: 'deepseek-reasoner',
    baseUrl: 'https://api.deepseek.com',
    apiType: 'openai-completions',
  }, { options: { reasoning: 'medium' } })
  // Evidence both ways: the field is real on the wire, and C04 refuses to synthesize one.
  assert.ok('thinking' in capture.payloads[0], JSON.stringify(capture.payloads[0]))
  assert.ok(API_PARAMETER_FIELDS['openai-completions'].includes('thinking'))
  const built = buildRequestParameters(capture.profile, { thinking: { type: 'enabled' } })
  assert.equal(built.rejected.find((entry) => entry.field === 'thinking').reason, 'no_verified_thinking_mapping')
  assert.equal('thinking' in built.sent, false)
})

test('O-D-02: the vocabulary names its source and this file claims capture, not verification', () => {
  for (const api of Object.keys(API_PARAMETER_FIELDS)) {
    assert.ok(WIRE_VOCABULARY_SOURCES[api]?.serializer, `${api} must name the serializer it came from`)
  }
  assert.equal(WIRE_VOCABULARY_SOURCES['openai-responses'].foxOverrides !== undefined, true)
  assert.equal(CONFIG_EVIDENCE_LEVEL.ADAPTER_CAPTURE, 'offline_adapter_capture')
  assert.notEqual(CONFIG_EVIDENCE_LEVEL.ADAPTER_CAPTURE, CONFIG_EVIDENCE_LEVEL.APPLIED_VERIFIED)
})
