import test from 'node:test'
import assert from 'node:assert/strict'
import {
  API_PARAMETER_FIELDS,
  CONFIG_EVIDENCE_LEVEL,
  CONFIG_SOURCE,
  EXECUTION_AFFECTING_FIELDS,
  OUTPUT_BUDGET_PRECEDENCE,
  PROFILE_OVERRIDE_ORDER,
  WIRE_VOCABULARY_SOURCES,
  buildRequestParameters,
  describeModelProfile,
  modelProfilePrompt,
  modelProfileSnapshot,
  resolveModelProfile,
  transportProvider,
  unverifiedCapabilities,
} from '../src/model-profile.mjs'

const cases = [
  { name: 'MiniMax M3', config: { modelId: 'MiniMax-M3', baseUrl: 'https://api.minimaxi.com/anthropic', apiType: 'anthropic-messages' }, expected: { id: 'minimax-cn/minimax', reasoning: true, thinkingLevel: 'medium', supportsPromptCache: true } },
  { name: 'DeepSeek Reasoner', config: { modelId: 'deepseek-reasoner', baseUrl: 'https://api.deepseek.com/v1', apiType: 'openai-completions' }, expected: { id: 'deepseek/deepseek', reasoning: true, thinkingLevel: 'medium' } },
  { name: 'DeepSeek Chat', config: { modelId: 'deepseek-chat', baseUrl: 'https://api.deepseek.com/v1', apiType: 'openai-completions' }, expected: { id: 'deepseek/deepseek', reasoning: false, thinkingLevel: 'off' } },
  { name: 'OpenAI GPT-5', config: { modelId: 'gpt-5.4', baseUrl: 'https://api.openai.com/v1', apiType: 'openai-responses' }, expected: { id: 'openai/openai', reasoning: true, supportsPromptCache: true } },
  { name: 'Claude 4', config: { modelId: 'claude-sonnet-4-6', baseUrl: 'https://api.anthropic.com', apiType: 'anthropic-messages' }, expected: { id: 'anthropic/claude', reasoning: true, supportsPromptCache: true } },
  { name: 'Qwen Coder', config: { modelId: 'qwen3-coder-plus', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', apiType: 'openai-completions' }, expected: { id: 'alibaba/qwen', reasoning: false } },
  { name: 'GLM 5', config: { modelId: 'glm-5', baseUrl: 'https://open.bigmodel.cn/api/paas/v4', apiType: 'openai-completions' }, expected: { id: 'zhipu/glm', reasoning: true } },
  { name: 'Kimi K2.5', config: { modelId: 'kimi-k2.5', baseUrl: 'https://api.moonshot.cn/v1', apiType: 'openai-completions' }, expected: { id: 'moonshot/kimi', reasoning: true } },
  { name: 'Local generic', config: { modelId: 'my-local-model', baseUrl: 'http://127.0.0.1:11434/v1', apiType: 'openai-completions' }, expected: { id: 'local/generic', reasoning: false, supportsPromptCache: false } },
]

for (const fixture of cases) {
  test(`resolves ${fixture.name} capabilities`, () => {
    const profile = resolveModelProfile({ contextWindow: 128_000, maxOutputTokens: 8_192, ...fixture.config })
    for (const [key, value] of Object.entries(fixture.expected)) assert.equal(profile[key], value, key)
    assert.equal(profile.supportsTools, true)
    assert.ok(profile.runtime.reserveTokens > 0)
    assert.ok(profile.runtime.keepRecentTokens > 0)
  })
}

test('applies explicit safe overrides without changing the detected family', () => {
  const profile = resolveModelProfile({
    modelId: 'MiniMax-M3',
    baseUrl: 'https://api.minimax.io/anthropic',
    apiType: 'anthropic-messages',
    modelProfile: { reasoning: false, supportsParallelTools: false, plannerEnabled: false },
  })
  assert.equal(profile.id, 'minimax/minimax')
  assert.equal(profile.reasoning, false)
  assert.equal(profile.thinkingLevel, 'off')
  assert.equal(profile.supportsParallelTools, false)
  assert.equal(profile.planner.enabled, false)
})

test('emits only serializable runtime diagnostics and model-specific prompt guidance', () => {
  const profile = resolveModelProfile({ modelId: 'glm-5', baseUrl: 'https://api.z.ai/v1', apiType: 'openai-completions' })
  const snapshot = modelProfileSnapshot(profile)
  assert.equal(JSON.parse(JSON.stringify(snapshot)).id, 'zhipu/glm')
  assert.match(modelProfilePrompt(profile), /native structured tool calls/)
  assert.match(modelProfilePrompt(profile), /Never put <think>/)
  assert.equal(transportProvider(profile), 'fox-openai-compatible')
})

test('uses native Pi provider identities where they improve compatibility detection', () => {
  assert.equal(transportProvider(resolveModelProfile({ modelId: 'deepseek-reasoner', baseUrl: 'https://api.deepseek.com/v1', apiType: 'openai-completions' })), 'deepseek')
  assert.equal(transportProvider(resolveModelProfile({ modelId: 'gpt-5.4', baseUrl: 'https://api.openai.com/v1', apiType: 'openai-responses' })), 'openai')
  assert.equal(transportProvider(resolveModelProfile({ modelId: 'claude-sonnet-4-6', baseUrl: 'https://api.anthropic.com', apiType: 'anthropic-messages' })), 'anthropic')
})

test('does not infer the model family from a version segment in the provider URL', () => {
  const profile = resolveModelProfile({
    modelId: 'custom-chat-model',
    baseUrl: 'https://example.test/api/v4',
    apiType: 'openai-completions',
  })
  assert.equal(profile.family, 'generic')
})

/* ---------------------------------------------------------------------------
 * C04 — offline configuration verification. Evidence level: resolution and
 * serialization only; no provider was contacted and nothing here proves an
 * endpoint accepted what we would send.
 * ------------------------------------------------------------------------- */

const DEEPSEEK = { modelId: 'deepseek-reasoner', baseUrl: 'https://api.deepseek.com/v1', apiType: 'openai-completions' }
const DEEPSEEK_CHAT = { modelId: 'deepseek-chat', baseUrl: 'https://api.deepseek.com/v1', apiType: 'openai-completions' }

test('C04: raw, resolved and sent stay three layers, and precedence is declared', () => {
  const description = describeModelProfile({ ...DEEPSEEK, thinkingLevel: 'low', modelProfile: { thinkingLevel: 'high' } })
  // raw is exactly what the caller supplied, including the nested layer
  assert.equal(description.raw.thinkingLevel, 'low')
  assert.equal(description.raw.modelProfile.thinkingLevel, 'high')
  assert.equal(description.raw.modelId, 'deepseek-reasoner')
  // resolved honours the documented order: modelProfile beats the top level
  assert.equal(description.resolved.thinkingLevel, 'high')
  assert.equal(description.provenance.thinkingLevel, CONFIG_SOURCE.EXPLICIT_PROFILE)
  assert.equal(description.provenance.modelId, CONFIG_SOURCE.EXPLICIT_REQUEST)
  assert.deepEqual(description.overrideOrder, PROFILE_OVERRIDE_ORDER)
  assert.equal(description.schemaVersion, 'model-config-v1')
  assert.equal(description.evidenceLevel, 'offline_config_resolution_only')
  // the resolved layer is directly usable as the sent layer's input
  const wire = buildRequestParameters(description.resolved, { model: 'deepseek-reasoner', messages: [] })
  assert.equal(wire.profileId, description.resolved.id)
  assert.deepEqual(wire.rejected, [])

  // without the nested override the top level wins
  const topLevelOnly = describeModelProfile({ ...DEEPSEEK, thinkingLevel: 'low' })
  assert.equal(topLevelOnly.resolved.thinkingLevel, 'low')
  assert.equal(topLevelOnly.provenance.thinkingLevel, CONFIG_SOURCE.EXPLICIT_REQUEST)
})

test('C04: an unsupported thinking level is rejected and reported, not quietly replaced', () => {
  const description = describeModelProfile({ ...DEEPSEEK, modelProfile: { thinkingLevel: 'ultra' } })
  assert.equal(description.resolved.thinkingLevel, 'medium')
  const rejection = description.rejected.find((entry) => entry.field === 'thinkingLevel')
  assert.equal(rejection.reason, 'unsupported_thinking_level')
  assert.equal(rejection.requested, 'ultra')
  assert.equal(rejection.applied, 'medium')
  // the applied value came from the heuristic, so it must not read as configured
  assert.equal(description.provenance.thinkingLevel, CONFIG_SOURCE.HEURISTIC)
  assert.ok(description.unverified.includes('thinkingLevel'))
})

test('C04: a thinking level supplied while reasoning is off is rejected with a reason', () => {
  const description = describeModelProfile({ ...DEEPSEEK_CHAT, thinkingLevel: 'high' })
  assert.equal(description.resolved.reasoning, false)
  assert.equal(description.resolved.thinkingLevel, 'off')
  assert.equal(description.provenance.thinkingLevel, CONFIG_SOURCE.DERIVED)
  assert.equal(description.rejected.find((entry) => entry.field === 'thinkingLevel').reason, 'reasoning_disabled')
})

test('C04: context and output limits are clamped with the envelope reported', () => {
  const description = describeModelProfile({ ...DEEPSEEK_CHAT, contextWindow: 100, maxOutputTokens: 999_999 })
  assert.equal(description.resolved.contextWindow, 4_096)
  assert.equal(description.resolved.maxOutputTokens, 4_096)
  const context = description.clamped.find((entry) => entry.field === 'contextWindow')
  assert.equal(context.reason, 'below_minimum')
  assert.equal(context.requested, 100)
  assert.equal(context.applied, 4_096)
  assert.equal(context.maximum, 4_000_000)
  const output = description.clamped.find((entry) => entry.field === 'maxOutputTokens')
  assert.equal(output.reason, 'above_maximum')
  assert.equal(output.maximum, 4_096)
})

test('C04: unconfigured limits use a conservative default and are marked as assumptions', () => {
  const description = describeModelProfile(DEEPSEEK_CHAT)
  assert.equal(description.resolved.contextWindow, 128_000)
  assert.notEqual(description.resolved.contextWindow, 4_000_000)
  assert.equal(description.provenance.contextWindow, CONFIG_SOURCE.ASSUMED)
  assert.ok(description.assumptions.some((entry) => entry.field === 'contextWindow'))
  assert.equal(description.clamped.length, 0)
})

test('C04: the two retry layers stay independent and bounded', () => {
  const defaults = describeModelProfile(DEEPSEEK_CHAT)
  // turn-level retry stays off; only the bounded provider HTTP retry is on
  assert.equal(defaults.resolved.runtime.maxRetries, 0)
  assert.equal(defaults.resolved.runtime.providerMaxRetries, 2)
  assert.equal(defaults.provenance['runtime.maxRetries'], CONFIG_SOURCE.ASSUMED)

  const asked = describeModelProfile({ ...DEEPSEEK_CHAT, modelProfile: { maxRetries: 3, providerMaxRetries: 99 } })
  assert.equal(asked.resolved.runtime.maxRetries, 3)
  assert.equal(asked.resolved.runtime.providerMaxRetries, 5)
  assert.equal(asked.provenance['runtime.maxRetries'], CONFIG_SOURCE.EXPLICIT_PROFILE)
  assert.equal(asked.clamped.find((entry) => entry.field === 'runtime.providerMaxRetries').reason, 'above_maximum')

  // raising one layer must not touch the other
  const turnOnly = describeModelProfile({ ...DEEPSEEK_CHAT, modelProfile: { maxRetries: 5 } })
  assert.equal(turnOnly.resolved.runtime.providerMaxRetries, 2)
})

test('C04: a field the resolver never reads from the top level is reported as ignored', () => {
  const ignored = describeModelProfile({ ...DEEPSEEK_CHAT, supportsTools: false, maxRetries: 4 })
  assert.equal(ignored.resolved.supportsTools, true)
  assert.equal(ignored.resolved.runtime.maxRetries, 0)
  for (const field of ['supportsTools', 'maxRetries']) {
    const entry = ignored.rejected.find((item) => item.field === field)
    assert.equal(entry.reason, 'ignored_by_resolver_supply_it_under_modelProfile', field)
  }
  // ...and the same fields do take effect from the documented layer
  const honoured = describeModelProfile({ ...DEEPSEEK_CHAT, modelProfile: { supportsTools: false, maxRetries: 4 } })
  assert.equal(honoured.resolved.supportsTools, false)
  assert.equal(honoured.resolved.runtime.maxRetries, 4)
})

test('C04: an unsupported apiType is reported instead of being silently coerced', () => {
  const description = describeModelProfile({ modelId: 'custom-model', baseUrl: 'https://example.test/v1', apiType: 'gemini-native' })
  assert.equal(description.resolved.api, 'openai-completions')
  const rejection = description.rejected.find((entry) => entry.field === 'apiType')
  assert.equal(rejection.reason, 'unsupported_api_type')
  assert.equal(rejection.requested, 'gemini-native')
  assert.ok(description.warnings.some((warning) => /family not recognised/.test(warning)))
})

test('C04: the sent layer carries only what the API shape and profile support', () => {
  const reasoning = resolveModelProfile({ modelId: 'gpt-5.4', baseUrl: 'https://api.openai.com/v1', apiType: 'openai-completions' })
  const sent = buildRequestParameters(reasoning, {
    model: 'gpt-5.4',
    messages: [],
    maxOutputTokens: 999_999,
    reasoning_effort: 'high',
    store: false,
    temperature: 0.2,
    parallel_tool_calls: true,
    thinking: { type: 'enabled' },
    gemini_top_k: 5,
  })
  assert.equal(sent.maxTokensField, 'max_completion_tokens')
  assert.equal(sent.sent.max_completion_tokens, reasoning.maxOutputTokens)
  assert.equal(sent.sent.reasoning_effort, 'high')
  assert.equal(sent.sent.store, false)
  assert.equal(sent.sent.temperature, 0.2)
  assert.equal('thinking' in sent.sent, false)
  assert.equal('gemini_top_k' in sent.sent, false)
  assert.equal(sent.clamped.find((entry) => entry.field === 'maxOutputTokens').reason, 'above_profile_cap')
  // `thinking` is part of the openai-completions wire vocabulary (the DeepSeek/zai/qwen formats use
  // it) but no provider-specific payload has been verified offline, so it is refused by name.
  assert.equal(sent.rejected.find((entry) => entry.field === 'thinking').reason, 'no_verified_thinking_mapping')
  assert.equal(sent.rejected.find((entry) => entry.field === 'gemini_top_k').reason, 'unsupported_by_api_shape')
  // The adapters never emit this one: Fox enforces single-tool behaviour through the compat prompt.
  assert.equal(sent.rejected.find((entry) => entry.field === 'parallel_tool_calls').reason, 'not_serialized_by_the_adapter')
  assert.equal('max_tokens' in sent.sent, false)
})

test('C04: parameters a non-reasoning profile cannot carry are rejected with reasons', () => {
  const profile = resolveModelProfile(DEEPSEEK_CHAT)
  const sent = buildRequestParameters(profile, {
    model: 'deepseek-chat',
    messages: [],
    max_tokens: 1_024,
    max_completion_tokens: 1_024,
    reasoning_effort: 'high',
    store: true,
  })
  assert.equal(sent.maxTokensField, 'max_tokens')
  assert.equal(sent.sent.max_tokens, 1_024)
  assert.equal(sent.rejected.find((entry) => entry.field === 'reasoning_effort').reason, 'unsupported_by_profile')
  assert.equal(sent.rejected.find((entry) => entry.field === 'store').reason, 'unsupported_by_profile')
  assert.equal(
    sent.rejected.find((entry) => entry.field === 'max_completion_tokens').reason,
    'wrong_token_limit_field_for_profile',
  )
})

test('C04: each API shape exposes its own parameter set', () => {
  const anthropic = resolveModelProfile({ modelId: 'claude-sonnet-4-6', baseUrl: 'https://api.anthropic.com', apiType: 'anthropic-messages' })
  const anthropicWire = buildRequestParameters(anthropic, { model: 'claude-sonnet-4-6', messages: [], system: 's', max_tokens: 64, reasoning_effort: 'high', thinking: { type: 'enabled' } })
  assert.equal(anthropicWire.sent.system, 's')
  assert.equal(anthropicWire.sent.max_tokens, 64)
  assert.equal(anthropicWire.rejected.find((entry) => entry.field === 'reasoning_effort').reason, 'unsupported_by_api_shape')
  assert.equal(anthropicWire.rejected.find((entry) => entry.field === 'thinking').reason, 'no_verified_thinking_mapping')

  const faux = resolveModelProfile({ modelId: 'faux-model', apiType: 'faux' })
  const fauxWire = buildRequestParameters(faux, { model: 'faux-model', messages: [], temperature: 1 })
  assert.deepEqual(fauxWire.allowedFields, [...API_PARAMETER_FIELDS.faux])
  assert.equal(fauxWire.rejected.find((entry) => entry.field === 'temperature').reason, 'unsupported_by_api_shape')
})

test('C04: an unknown model is described by assumptions, with no capability claimed as verified', () => {
  const description = describeModelProfile({ modelId: 'my-local-model', baseUrl: 'http://127.0.0.1:11434/v1', apiType: 'openai-completions' })
  assert.equal(description.resolved.family, 'generic')
  assert.equal(description.resolved.thinkingLevel, 'off')
  assert.equal(description.provenance.family, CONFIG_SOURCE.HEURISTIC)
  assert.equal(description.provenance.supportsTools, CONFIG_SOURCE.ASSUMED)
  // nothing claimed from a name may be reported as configured
  for (const field of description.unverified) {
    assert.notEqual(description.provenance[field], CONFIG_SOURCE.EXPLICIT_REQUEST)
    assert.notEqual(description.provenance[field], CONFIG_SOURCE.EXPLICIT_PROFILE)
  }
  assert.deepEqual(description.unverified, unverifiedCapabilities(description))
  assert.ok(description.unverified.includes('supportsTools'))
  assert.ok(EXECUTION_AFFECTING_FIELDS.every((field) => field in description.provenance))
  // and the new layer does not change the resolved provider identity or API
  assert.equal(description.resolved.provider, 'local')
  assert.equal(description.resolved.api, 'openai-completions')
  assert.equal(transportProvider(description.resolved), 'fox-openai-compatible')
})

/* ===========================================================================
 * O-D-02 — the review's second round
 *
 * (1) an invalid budget must never reach `sent`: -1, 0, 'bogus' and friends have to come back as
 *     a rejection reason plus a legal positive integer, never as the raw value;
 * (2) the neutral budget field and the wire budget fields need a stated precedence instead of the
 *     accident of TOKEN_LIMIT_FIELDS ordering.
 * ======================================================================== */

const COMPLETIONS_REASONING = Object.freeze({
  modelId: 'gpt-5',
  baseUrl: 'https://api.openai.com/v1',
  apiType: 'openai-completions',
})

const isLegalBudget = (value) => Number.isInteger(value) && value > 0

test('O-D-02(1): an invalid output budget never reaches the wire', () => {
  const profile = resolveModelProfile(COMPLETIONS_REASONING)
  const field = buildRequestParameters(profile, {}).maxTokensField
  const cases = [
    { budget: -1, reason: 'below_minimum' },
    { budget: 0, reason: 'below_minimum' },
    { budget: 0.4, reason: 'below_minimum' },
    { budget: -0.5, reason: 'below_minimum' },
    { budget: 'bogus', reason: 'not_a_number' },
    { budget: Number.NaN, reason: 'not_a_number' },
    { budget: Number.POSITIVE_INFINITY, reason: 'not_a_number' },
    { budget: {}, reason: 'not_a_number' },
    { budget: 1.4, reason: 'rounded' },
    { budget: '1024', reason: 'coerced_to_number' },
    { budget: 9_999_999, reason: 'above_profile_cap' },
  ]
  for (const { budget, reason } of cases) {
    const built = buildRequestParameters(profile, { maxOutputTokens: budget })
    const applied = built.sent[field]
    assert.ok(isLegalBudget(applied), `${JSON.stringify(budget)} must not serialize as ${JSON.stringify(applied)}`)
    assert.equal('maxOutputTokens' in built.sent, false, 'the neutral field must not leak onto the wire')
    assert.equal(built.budget.applied, applied)
    assert.equal(built.budget.changed, true)
    const entry = built.clamped.find((clamp) => clamp.field === 'maxOutputTokens')
    assert.equal(entry?.reason, reason, `wrong reason for ${JSON.stringify(budget)}: ${JSON.stringify(built.clamped)}`)
    assert.equal(entry.applied, applied)
    // The same reading of the contract O used: every key of `sent` is a declared field of the shape.
    assert.ok(Object.keys(built.sent).every((key) => built.allowedFields.includes(key)), JSON.stringify(built))
  }
})

test('O-D-02(1): a legal positive budget is passed through unchanged, including below the profile floor', () => {
  const profile = resolveModelProfile(COMPLETIONS_REASONING)
  const field = buildRequestParameters(profile, {}).maxTokensField
  for (const budget of [1, 64, 256, 1024, profile.maxOutputTokens]) {
    const built = buildRequestParameters(profile, { maxOutputTokens: budget })
    assert.equal(built.sent[field], budget)
    assert.equal(built.budget.changed, false)
    assert.deepEqual(built.clamped, [])
  }
})

test('O-D-02(1): a wire budget name is validated exactly like the neutral one', () => {
  const profile = resolveModelProfile(COMPLETIONS_REASONING)
  for (const budget of [-1, 0, 'bogus']) {
    const built = buildRequestParameters(profile, { max_completion_tokens: budget })
    assert.ok(isLegalBudget(built.sent.max_completion_tokens), JSON.stringify(built))
    assert.equal(
      built.clamped.find((entry) => entry.field === 'maxOutputTokens')?.applied,
      built.sent.max_completion_tokens,
    )
  }
})

test('O-D-02(2): the neutral budget supersedes the profile wire field, and says so', () => {
  const profile = resolveModelProfile(COMPLETIONS_REASONING)
  const built = buildRequestParameters(profile, {
    maxOutputTokens: 1024,
    max_completion_tokens: 777,
    max_tokens: 555,
  })
  assert.equal(built.maxTokensField, 'max_completion_tokens')
  assert.equal(built.sent.max_completion_tokens, 1024)
  assert.equal('max_tokens' in built.sent, false)
  assert.equal(built.budget.source, 'maxOutputTokens')
  assert.equal(
    built.rejected.find((entry) => entry.field === 'max_completion_tokens').reason,
    'superseded_by_neutral_output_budget',
  )
  assert.equal(
    built.rejected.find((entry) => entry.field === 'max_tokens').reason,
    'wrong_token_limit_field_for_profile',
  )
  assert.equal(OUTPUT_BUDGET_PRECEDENCE.length, 3)
  assert.equal(OUTPUT_BUDGET_PRECEDENCE[1], "requested[<profile.compat.maxTokensField>] — this profile's own wire field")
})

test('O-D-02(2): without the neutral field the profile wire field wins and nothing is invented', () => {
  const profile = resolveModelProfile(COMPLETIONS_REASONING)
  const own = buildRequestParameters(profile, { max_completion_tokens: 777 })
  assert.equal(own.sent.max_completion_tokens, 777)
  assert.equal(own.budget.source, 'max_completion_tokens')

  // The caller named the other wire field: it is reported, and the sent layer does not quietly
  // rewrite it into the field this profile uses — that would be inventing a request they never made.
  const wrong = buildRequestParameters(profile, { max_tokens: 555 })
  assert.deepEqual(wrong.sent, {})
  assert.equal(
    wrong.rejected.find((entry) => entry.field === 'max_tokens').reason,
    'wrong_token_limit_field_for_profile',
  )
  assert.equal(wrong.budget.source, 'not_requested')
  assert.equal(wrong.budget.applied, null)

  const none = buildRequestParameters(profile, {})
  assert.equal(none.budget.source, 'not_requested')
  assert.deepEqual(none.sent, {})
})

test('O-D-02(1): a prediction may never claim a verified send', () => {
  const built = buildRequestParameters(resolveModelProfile(COMPLETIONS_REASONING), { maxOutputTokens: 512 })
  assert.equal(built.evidenceLevel, CONFIG_EVIDENCE_LEVEL.SERIALIZATION_PREDICTION)
  assert.notEqual(built.evidenceLevel, CONFIG_EVIDENCE_LEVEL.APPLIED_VERIFIED)
  assert.match(built.evidenceNote, /NOT a capture/)
  const described = describeModelProfile(COMPLETIONS_REASONING)
  assert.equal(described.evidenceLevel, CONFIG_EVIDENCE_LEVEL.CONFIG_RESOLUTION)
  assert.notEqual(described.evidenceLevel, CONFIG_EVIDENCE_LEVEL.APPLIED_VERIFIED)
})

test('O-D-02(2): a shape does not carry every token-limit name, and the fallback field is declared', () => {
  // A compat override naming a field the shape cannot carry is reported, not honoured.
  const responses = resolveModelProfile({
    modelId: 'gpt-5',
    baseUrl: 'https://api.openai.com/v1',
    apiType: 'openai-responses',
    modelProfile: { compat: { maxTokensField: 'max_tokens' } },
  })
  const built = buildRequestParameters(responses, { maxOutputTokens: 512 })
  assert.equal(built.maxTokensField, 'max_output_tokens')
  assert.equal(built.maxTokensFieldSource, 'api_shape_default')
  assert.equal(
    built.rejected.find((entry) => entry.field === 'compat.maxTokensField').reason,
    'compat_names_a_field_the_api_shape_cannot_carry',
  )
  assert.equal(built.sent.max_output_tokens, 512)

  // Where the profile declares nothing, the reader can tell the field was the shape's default.
  const openrouter = resolveModelProfile({
    modelId: 'some-model',
    baseUrl: 'https://openrouter.ai/api/v1',
    apiType: 'openai-completions',
  })
  const openrouterWire = buildRequestParameters(openrouter, { maxOutputTokens: 300 })
  assert.equal(openrouterWire.maxTokensFieldSource, 'api_shape_default')
  assert.ok(openrouterWire.allowedFields.includes(openrouterWire.maxTokensField))

  const declared = buildRequestParameters(resolveModelProfile(COMPLETIONS_REASONING), {})
  assert.equal(declared.maxTokensFieldSource, 'profile_compat')
})

test('O-D-02: the declared vocabulary names the serializer it was taken from', () => {
  for (const api of Object.keys(API_PARAMETER_FIELDS)) {
    assert.ok(WIRE_VOCABULARY_SOURCES[api]?.serializer, `${api} must name its serializer`)
  }
  // Sampling passthrough is a channel, not a first-class field: the caller is told which channel.
  const profile = resolveModelProfile(COMPLETIONS_REASONING)
  const built = buildRequestParameters(profile, { top_p: 0.9 })
  assert.equal(built.sent.top_p, 0.9)
  assert.equal(built.passthrough.find((entry) => entry.field === 'top_p').channel, 'samplingParams')
})
