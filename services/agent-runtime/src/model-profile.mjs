const THINKING_LEVELS = new Set(['off', 'minimal', 'low', 'medium', 'high', 'xhigh'])

function normalizedConfig(config = {}) {
  const modelId = String(config.modelId || '').trim()
  const baseUrl = String(config.baseUrl || '').trim()
  return {
    modelId,
    baseUrl,
    apiType: String(config.apiType || 'openai-completions'),
    modelSignature: modelId.toLowerCase(),
    signature: `${modelId} ${baseUrl}`.toLowerCase(),
  }
}

function providerFamily({ apiType, signature }) {
  if (apiType === 'faux') return 'faux'
  if (/minimaxi\.com/.test(signature)) return 'minimax-cn'
  if (/minimax\.io/.test(signature)) return 'minimax'
  if (/openrouter\.ai/.test(signature)) return 'openrouter'
  if (/api\.openai\.com/.test(signature)) return 'openai'
  if (/anthropic\.com/.test(signature)) return 'anthropic'
  if (/deepseek\.com/.test(signature)) return 'deepseek'
  if (/api\.x\.ai/.test(signature)) return 'xai'
  if (/dashscope|aliyuncs/.test(signature)) return 'alibaba'
  if (/bigmodel|z\.ai/.test(signature)) return 'zhipu'
  if (/moonshot|kimi\.com/.test(signature)) return 'moonshot'
  if (/generativelanguage|googleapis/.test(signature)) return 'google'
  if (/localhost|127\.0\.0\.1|0\.0\.0\.0|ollama/.test(signature)) return 'local'
  return apiType === 'anthropic-messages' ? 'anthropic-compatible' : 'openai-compatible'
}

function modelFamily(signature) {
  if (/minimax/.test(signature)) return 'minimax'
  if (/deepseek|\b(?:r1|v3|v4)(?:[-_.:]|\b)/.test(signature)) return 'deepseek'
  if (/claude/.test(signature)) return 'claude'
  if (/\b(?:gpt|o1|o3|o4)(?:[-_.:]|\b)/.test(signature)) return 'openai'
  if (/qwen|qwq/.test(signature)) return 'qwen'
  if (/\bglm[-_.:]/.test(signature)) return 'glm'
  if (/kimi|moonshot|\bk[23](?:[-_.:]|\b)/.test(signature)) return 'kimi'
  if (/gemini|gemma/.test(signature)) return 'gemini'
  if (/grok/.test(signature)) return 'grok'
  return 'generic'
}

function familyDefaults(family, signature, apiType) {
  switch (family) {
    case 'minimax':
      return { reasoning: true, thinkingLevel: 'medium', promptCache: apiType === 'anthropic-messages', reasoningTransport: 'mixed-text' }
    case 'deepseek': {
      const reasoning = /reasoner|deepseek-r1|\br1(?:[-_.:]|\b)|deepseek-v4/.test(signature)
      return { reasoning, thinkingLevel: reasoning ? 'medium' : 'off', promptCache: true, reasoningTransport: reasoning ? 'native' : 'none' }
    }
    case 'claude': {
      const reasoning = /claude-(?:(?:3[-_.]?7|4)(?:[-_.:]|\b)|(?:opus|sonnet|haiku)-4)/.test(signature)
      return { reasoning, thinkingLevel: reasoning ? 'medium' : 'off', promptCache: true, reasoningTransport: reasoning ? 'native' : 'none' }
    }
    case 'openai': {
      const reasoning = /\b(?:gpt-5|o1|o3|o4)(?:[-_.:]|\b)/.test(signature)
      return { reasoning, thinkingLevel: reasoning ? 'medium' : 'off', promptCache: apiType === 'openai-responses', reasoningTransport: reasoning ? 'native' : 'none' }
    }
    case 'qwen': {
      const reasoning = /qwq|thinking|reasoner/.test(signature)
      return { reasoning, thinkingLevel: reasoning ? 'medium' : 'off', promptCache: false, reasoningTransport: reasoning ? 'mixed-text' : 'none' }
    }
    case 'glm': {
      const reasoning = /glm-(?:4[-_.]?[67]|5)/.test(signature)
      return { reasoning, thinkingLevel: reasoning ? 'medium' : 'off', promptCache: false, reasoningTransport: reasoning ? 'mixed-text' : 'none' }
    }
    case 'kimi': {
      const reasoning = /kimi-k2\.[5-9]|kimi-k[3-9]|\bk[3-9](?:[-_.:]|\b)|thinking|reasoner/.test(signature)
      return { reasoning, thinkingLevel: reasoning ? 'medium' : 'off', promptCache: false, reasoningTransport: reasoning ? 'mixed-text' : 'none' }
    }
    case 'gemini': {
      const reasoning = /gemini-(?:2[-_.]?5|3)/.test(signature)
      return { reasoning, thinkingLevel: reasoning ? 'medium' : 'off', promptCache: true, reasoningTransport: reasoning ? 'native' : 'none' }
    }
    case 'grok': {
      const reasoning = !/non-reasoning/.test(signature)
      return { reasoning, thinkingLevel: reasoning ? 'medium' : 'off', promptCache: false, reasoningTransport: reasoning ? 'native' : 'none' }
    }
    default:
      return { reasoning: false, thinkingLevel: 'off', promptCache: false, reasoningTransport: 'none' }
  }
}

function compatFor({ apiType, provider, family, reasoning }) {
  if (apiType === 'openai-responses') {
    return {
      supportsDeveloperRole: provider === 'openai',
      supportsLongCacheRetention: provider === 'openai',
    }
  }
  if (apiType !== 'openai-completions') return {}
  if (provider === 'openrouter') return {}
  if (provider === 'openai') {
    return {
      supportsDeveloperRole: reasoning,
      supportsReasoningEffort: reasoning,
      maxTokensField: reasoning ? 'max_completion_tokens' : 'max_tokens',
    }
  }
  const compat = {
    supportsStore: false,
    supportsDeveloperRole: false,
    supportsReasoningEffort: false,
    maxTokensField: 'max_tokens',
  }
  if (family === 'deepseek' && reasoning) {
    compat.requiresReasoningContentOnAssistantMessages = true
    compat.thinkingFormat = 'deepseek'
  }
  return compat
}

function boundedNumber(value, fallback, minimum, maximum) {
  const number = Number(value)
  return Number.isFinite(number) ? Math.min(maximum, Math.max(minimum, Math.round(number))) : fallback
}

/**
 * The numeric envelope, named once so the resolver and the C04 reporting layer can never drift
 * apart. Reporting a clamp that the resolver does not apply (or vice versa) would be worse than
 * not reporting at all.
 */
export const CONFIG_BOUNDS = Object.freeze({
  contextWindow: Object.freeze({ minimum: 4_096, maximum: 4_000_000, fallback: 128_000 }),
  maxOutputTokens: Object.freeze({ minimum: 256, maximum: 131_072, fallback: 8_192 }),
  plannerMaxOutputTokens: Object.freeze({ minimum: 512, maximum: 8_192, fallback: 2_048 }),
  maxRetries: Object.freeze({ minimum: 0, maximum: 5, fallback: 0 }),
  providerMaxRetries: Object.freeze({ minimum: 0, maximum: 5, fallback: 2 }),
  providerMaxRetryDelayMs: Object.freeze({ minimum: 500, maximum: 60_000, fallback: 8_000 }),
})

function validThinkingLevel(value, fallback) {
  return THINKING_LEVELS.has(value) ? value : fallback
}

export function resolveModelProfile(config = {}) {
  const normalized = normalizedConfig(config)
  const provider = providerFamily(normalized)
  const family = modelFamily(normalized.modelSignature)
  const defaults = familyDefaults(family, normalized.modelSignature, normalized.apiType)
  const overrides = config.modelProfile && typeof config.modelProfile === 'object' && !Array.isArray(config.modelProfile)
    ? config.modelProfile
    : {}
  const reasoning = typeof overrides.reasoning === 'boolean'
    ? overrides.reasoning
    : typeof config.reasoning === 'boolean' ? config.reasoning : defaults.reasoning
  const thinkingLevel = reasoning
    ? validThinkingLevel(overrides.thinkingLevel ?? config.thinkingLevel, defaults.thinkingLevel === 'off' ? 'medium' : defaults.thinkingLevel)
    : 'off'
  const contextWindow = boundedNumber(
    config.contextWindow,
    CONFIG_BOUNDS.contextWindow.fallback,
    CONFIG_BOUNDS.contextWindow.minimum,
    CONFIG_BOUNDS.contextWindow.maximum,
  )
  const maxOutputTokens = boundedNumber(
    config.maxOutputTokens,
    CONFIG_BOUNDS.maxOutputTokens.fallback,
    CONFIG_BOUNDS.maxOutputTokens.minimum,
    Math.min(contextWindow, CONFIG_BOUNDS.maxOutputTokens.maximum),
  )
  const supportsImageInput = overrides.supportsImageInput ?? config.supportsImageInput === true
  const supportsTools = overrides.supportsTools !== false
  const plannerEnabled = supportsTools && overrides.plannerEnabled !== false
  const plannerThinkingLevel = reasoning
    ? validThinkingLevel(overrides.plannerThinkingLevel, thinkingLevel === 'xhigh' ? 'medium' : thinkingLevel === 'high' ? 'medium' : 'low')
    : 'off'
  const api = normalized.apiType === 'anthropic-messages'
    ? 'anthropic-messages'
    : normalized.apiType === 'openai-responses' ? 'openai-responses' : normalized.apiType === 'faux' ? 'faux' : 'openai-completions'
  const profile = {
    schemaVersion: 1,
    id: `${provider}/${family}`,
    family,
    provider,
    api,
    reasoning,
    reasoningTransport: overrides.reasoningTransport || defaults.reasoningTransport,
    thinkingLevel,
    supportsImageInput: Boolean(supportsImageInput),
    supportsTools,
    supportsParallelTools: overrides.supportsParallelTools !== false,
    supportsPromptCache: overrides.supportsPromptCache ?? defaults.promptCache,
    contextWindow,
    maxOutputTokens,
    planner: {
      enabled: plannerEnabled,
      thinkingLevel: plannerThinkingLevel,
      maxOutputTokens: boundedNumber(
        overrides.plannerMaxOutputTokens,
        CONFIG_BOUNDS.plannerMaxOutputTokens.fallback,
        CONFIG_BOUNDS.plannerMaxOutputTokens.minimum,
        CONFIG_BOUNDS.plannerMaxOutputTokens.maximum,
      ),
    },
    runtime: {
      // Turn-level retry: re-running a whole agent turn after a terminal turn
      // failure. Owned independently from the provider HTTP retry below so that a
      // rate-limited request is never retried by both layers (double cost / 429
      // amplification). The Fox Kernel is the authority for turn-retry policy; the
      // sidecar defaults it off and only enables it when the Host configures it.
      maxRetries: boundedNumber(
        overrides.maxRetries,
        CONFIG_BOUNDS.maxRetries.fallback,
        CONFIG_BOUNDS.maxRetries.minimum,
        CONFIG_BOUNDS.maxRetries.maximum,
      ),
      // Provider HTTP retry: bounded retries of a single request for transient
      // transport/5xx/429 failures, honouring the server's Retry-After header.
      providerMaxRetries: boundedNumber(
        overrides.providerMaxRetries,
        CONFIG_BOUNDS.providerMaxRetries.fallback,
        CONFIG_BOUNDS.providerMaxRetries.minimum,
        CONFIG_BOUNDS.providerMaxRetries.maximum,
      ),
      providerMaxRetryDelayMs: boundedNumber(
        overrides.providerMaxRetryDelayMs,
        CONFIG_BOUNDS.providerMaxRetryDelayMs.fallback,
        CONFIG_BOUNDS.providerMaxRetryDelayMs.minimum,
        CONFIG_BOUNDS.providerMaxRetryDelayMs.maximum,
      ),
      reserveTokens: Math.min(16_384, Math.max(2_048, Math.floor(contextWindow * 0.12))),
      keepRecentTokens: Math.min(24_000, Math.max(4_096, Math.floor(contextWindow * 0.18))),
    },
  }
  return {
    ...profile,
    compat: { ...compatFor({ apiType: api, provider, family, reasoning }), ...(overrides.compat || {}) },
  }
}

export function modelProfilePrompt(profile) {
  if (!profile) return ''
  const lines = ['Model compatibility rules:']
  if (profile.reasoningTransport === 'mixed-text') {
    lines.push('- Use the provider reasoning channel when available. Never put <think>, <analysis>, or private planning in the final answer.')
  }
  if (profile.supportsTools) {
    lines.push('- Use native structured tool calls. Never print tool-call JSON as if it had executed.')
  }
  if (!profile.supportsParallelTools) {
    lines.push('- Call at most one tool at a time and wait for its result before the next tool call.')
  }
  return lines.length > 1 ? lines.join('\n') : ''
}

export function modelProfileSnapshot(profile) {
  return {
    schemaVersion: profile.schemaVersion,
    id: profile.id,
    family: profile.family,
    provider: profile.provider,
    api: profile.api,
    reasoning: profile.reasoning,
    reasoningTransport: profile.reasoningTransport,
    thinkingLevel: profile.thinkingLevel,
    supportsImageInput: profile.supportsImageInput,
    supportsTools: profile.supportsTools,
    supportsParallelTools: profile.supportsParallelTools,
    supportsPromptCache: profile.supportsPromptCache,
    planner: profile.planner,
    contextWindow: profile.contextWindow,
    maxOutputTokens: profile.maxOutputTokens,
  }
}

export function transportProvider(profile) {
  if (profile.provider === 'faux') return 'faux'
  if (profile.provider === 'minimax-cn' || profile.provider === 'minimax') return profile.provider
  if (['openai', 'anthropic', 'deepseek', 'openrouter', 'xai'].includes(profile.provider)) return profile.provider
  return profile.api === 'anthropic-messages' ? 'fox-anthropic-compatible' : 'fox-openai-compatible'
}

/* ===========================================================================
 * C04 — offline model-configuration verification: raw / resolved / sent
 *
 * `resolveModelProfile` stays deliberately total: it always returns a usable profile and never
 * throws. That is right for a live sidecar, but it also means an invalid, unsupported or ignored
 * input can vanish without a trace — "we asked for high thinking" becomes "thinking is medium"
 * in the record and nobody notices. The functions below resolve the profile exactly as
 * production does and then report what actually happened: which layer every applied value came
 * from, which values were clamped, which inputs were rejected, and which capabilities are still
 * only assumed rather than verified.
 *
 * Evidence level: offline resolution and serialization only. No provider was contacted, so this
 * proves what we *would* send — never that an endpoint accepted it. `applied_verified` is not
 * reachable here and must not be recorded offline.
 * ======================================================================== */

export const MODEL_CONFIG_SCHEMA_VERSION = 'model-config-v1'

/** Where an applied value came from. A value is "configured" only when a caller supplied it. */
export const CONFIG_SOURCE = Object.freeze({
  EXPLICIT_PROFILE: 'explicit_model_profile',
  EXPLICIT_REQUEST: 'explicit_request',
  API_SHAPE: 'api_shape',
  DERIVED: 'derived',
  HEURISTIC: 'heuristic',
  ASSUMED: 'assumed',
})

/** API shapes this module claims to understand; anything else is reported, not silently coerced. */
export const SUPPORTED_API_TYPES = Object.freeze([
  'openai-completions',
  'openai-responses',
  'anthropic-messages',
  'faux',
])

/** Most authoritative first. A lower source may never be reported as if the caller asked for it. */
export const PROFILE_OVERRIDE_ORDER = Object.freeze([
  'config.modelProfile.<field>',
  'config.<field>',
  'api-shape fact from apiType',
  'derived from another resolved value',
  'model/provider name pattern',
  'assumed fallback',
])

/** Fields whose value changes what the model is asked to do; unverified ones must be visible. */
export const EXECUTION_AFFECTING_FIELDS = Object.freeze([
  'reasoning',
  'thinkingLevel',
  'reasoningTransport',
  'supportsTools',
  'supportsParallelTools',
  'supportsPromptCache',
  'supportsImageInput',
  'contextWindow',
  'maxOutputTokens',
  'planner.enabled',
  'planner.thinkingLevel',
  'planner.maxOutputTokens',
  'runtime.maxRetries',
  'runtime.providerMaxRetries',
])

/** Read by resolveModelProfile from the top level of the config object. */
const TOP_LEVEL_FIELDS = Object.freeze([
  'modelId',
  'baseUrl',
  'apiType',
  'contextWindow',
  'maxOutputTokens',
  'thinkingLevel',
  'reasoning',
  'supportsImageInput',
])

/**
 * Read by resolveModelProfile only from config.modelProfile. Supplying one of these at the top
 * level has no effect at all, which a caller must be told rather than left to discover.
 */
const MODEL_PROFILE_ONLY_FIELDS = Object.freeze([
  'supportsTools',
  'supportsParallelTools',
  'supportsPromptCache',
  'plannerEnabled',
  'plannerThinkingLevel',
  'plannerMaxOutputTokens',
  'maxRetries',
  'providerMaxRetries',
  'providerMaxRetryDelayMs',
  'reasoningTransport',
  'compat',
])

const RAW_CONFIG_FIELDS = Object.freeze([...TOP_LEVEL_FIELDS, 'modelProfile'])

function isPlainObject(value) {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}

/** Report every value the resolver moved, so a clamp can never be mistaken for acceptance. */
function reportClamp(collection, field, requested, applied, bounds) {
  if (requested === undefined || requested === null || requested === '') return
  const number = Number(requested)
  const entry = { field, requested, applied, minimum: bounds.minimum, maximum: bounds.maximum }
  if (!Number.isFinite(number)) {
    collection.push({ ...entry, reason: 'not_a_number' })
    return
  }
  const rounded = Math.round(number)
  if (rounded < bounds.minimum) collection.push({ ...entry, reason: 'below_minimum' })
  else if (rounded > bounds.maximum) collection.push({ ...entry, reason: 'above_maximum' })
  else if (rounded !== number) collection.push({ ...entry, reason: 'rounded' })
}

/**
 * Capabilities that were neither configured by the caller nor verified: for an unknown model this
 * is exactly the list a caller must look at before claiming the model supports something.
 */
export function unverifiedCapabilities(description, fields = EXECUTION_AFFECTING_FIELDS) {
  const provenance = description?.provenance ?? {}
  return fields.filter((field) => {
    const source = provenance[field]
    return source === CONFIG_SOURCE.HEURISTIC || source === CONFIG_SOURCE.ASSUMED
  })
}

/**
 * Resolve the profile and describe the configuration in three layers:
 *   raw       — what the caller supplied, verbatim, including fields we do not read
 *   resolved  — what capability resolution produced (identical to resolveModelProfile)
 *   sent      — see buildRequestParameters: the parameters that would go on the wire
 * Plus provenance per field, and explicit `clamped`, `rejected`, `assumptions`, `unverified` and
 * `warnings` lists. Nothing is dropped silently and nothing heuristic is labelled configured.
 */
export function describeModelProfile(config = {}) {
  const safeConfig = config ?? {}
  const overrides = isPlainObject(safeConfig.modelProfile) ? safeConfig.modelProfile : {}
  const profile = resolveModelProfile(safeConfig)

  const raw = {}
  for (const field of RAW_CONFIG_FIELDS) {
    if (safeConfig[field] !== undefined) raw[field] = safeConfig[field]
  }

  const provenance = {}
  const clamped = []
  const rejected = []
  const assumptions = []
  const warnings = []

  for (const field of ['modelId', 'baseUrl']) {
    if (safeConfig[field] !== undefined) provenance[field] = CONFIG_SOURCE.EXPLICIT_REQUEST
  }

  const assume = (field, note) => {
    provenance[field] = CONFIG_SOURCE.ASSUMED
    assumptions.push({ field, source: CONFIG_SOURCE.ASSUMED, note })
  }
  const infer = (field, note) => {
    provenance[field] = CONFIG_SOURCE.HEURISTIC
    assumptions.push({ field, source: CONFIG_SOURCE.HEURISTIC, note })
  }

  // --- routing: api shape, provider, family ----------------------------------
  const requestedApiType = safeConfig.apiType === undefined ? null : String(safeConfig.apiType)
  if (requestedApiType === null) {
    assume('apiType', `no apiType supplied; assumed ${profile.api}`)
  } else if (!SUPPORTED_API_TYPES.includes(requestedApiType)) {
    assume('apiType', `unsupported apiType; assumed ${profile.api}`)
    rejected.push({
      field: 'apiType',
      requested: requestedApiType,
      applied: profile.api,
      reason: 'unsupported_api_type',
    })
  } else {
    provenance.apiType = CONFIG_SOURCE.EXPLICIT_REQUEST
  }
  infer('provider', `provider family "${profile.provider}" matched from the model id / base URL pattern`)
  infer('family', `model family "${profile.family}" matched from the model id pattern`)
  if (profile.family === 'generic') {
    warnings.push('model family not recognised: unconfigured capabilities are assumptions, not verified support')
  }
  if (String(safeConfig.modelId ?? '').trim() === '') {
    warnings.push('modelId is empty: no capability can be attributed to a model that was not named')
  }

  // --- reasoning and thinking level ------------------------------------------
  if (typeof overrides.reasoning === 'boolean') {
    provenance.reasoning = CONFIG_SOURCE.EXPLICIT_PROFILE
  } else if (typeof safeConfig.reasoning === 'boolean') {
    provenance.reasoning = CONFIG_SOURCE.EXPLICIT_REQUEST
  } else {
    infer('reasoning', `reasoning support inferred from the ${profile.provider}/${profile.family} name patterns`)
  }

  const askedThinkingLevel = overrides.thinkingLevel ?? safeConfig.thinkingLevel
  if (profile.reasoning === false) {
    // thinkingLevel is forced to 'off' by reasoning: the caller's value is not honoured, and any
    // value they supplied must be reported rather than quietly replaced.
    provenance.thinkingLevel = CONFIG_SOURCE.DERIVED
    if (askedThinkingLevel !== undefined && askedThinkingLevel !== null && askedThinkingLevel !== 'off') {
      rejected.push({
        field: 'thinkingLevel',
        requested: askedThinkingLevel,
        applied: 'off',
        reason: 'reasoning_disabled',
      })
    }
  } else if (overrides.thinkingLevel !== undefined && overrides.thinkingLevel !== null) {
    if (THINKING_LEVELS.has(overrides.thinkingLevel)) {
      provenance.thinkingLevel = CONFIG_SOURCE.EXPLICIT_PROFILE
    } else {
      infer('thinkingLevel', `unsupported thinking level rejected; fell back to the ${profile.family} default`)
      rejected.push({
        field: 'thinkingLevel',
        requested: overrides.thinkingLevel,
        applied: profile.thinkingLevel,
        reason: 'unsupported_thinking_level',
      })
    }
  } else if (safeConfig.thinkingLevel !== undefined && safeConfig.thinkingLevel !== null) {
    if (THINKING_LEVELS.has(safeConfig.thinkingLevel)) {
      provenance.thinkingLevel = CONFIG_SOURCE.EXPLICIT_REQUEST
    } else {
      infer('thinkingLevel', `unsupported thinking level rejected; fell back to the ${profile.family} default`)
      rejected.push({
        field: 'thinkingLevel',
        requested: safeConfig.thinkingLevel,
        applied: profile.thinkingLevel,
        reason: 'unsupported_thinking_level',
      })
    }
  } else {
    infer('thinkingLevel', `thinking level defaulted to "${profile.thinkingLevel}" by the model family heuristic`)
  }

  // --- context and output limits ---------------------------------------------
  provenance.contextWindow =
    safeConfig.contextWindow === undefined ? CONFIG_SOURCE.ASSUMED : CONFIG_SOURCE.EXPLICIT_REQUEST
  if (safeConfig.contextWindow === undefined) {
    assume('contextWindow', `context window defaulted to ${profile.contextWindow}; it was not configured`)
  }
  reportClamp(clamped, 'contextWindow', safeConfig.contextWindow, profile.contextWindow, CONFIG_BOUNDS.contextWindow)

  const outputBounds = {
    minimum: CONFIG_BOUNDS.maxOutputTokens.minimum,
    maximum: Math.min(profile.contextWindow, CONFIG_BOUNDS.maxOutputTokens.maximum),
  }
  provenance.maxOutputTokens =
    safeConfig.maxOutputTokens === undefined ? CONFIG_SOURCE.ASSUMED : CONFIG_SOURCE.EXPLICIT_REQUEST
  if (safeConfig.maxOutputTokens === undefined) {
    assume('maxOutputTokens', `output budget defaulted to ${profile.maxOutputTokens}; it was not configured`)
  }
  reportClamp(clamped, 'maxOutputTokens', safeConfig.maxOutputTokens, profile.maxOutputTokens, outputBounds)

  // --- capability flags ------------------------------------------------------
  const flag = (field, note, { heuristic = false } = {}) => {
    if (overrides[field] !== undefined) provenance[field] = CONFIG_SOURCE.EXPLICIT_PROFILE
    else if (heuristic) infer(field, note)
    else assume(field, note)
  }
  flag('supportsTools', 'tools are assumed supported unless modelProfile.supportsTools === false')
  flag('supportsParallelTools', 'parallel tool calls are assumed supported')
  flag('supportsPromptCache', 'prompt caching inferred from the provider/model family', { heuristic: true })
  if (overrides.supportsImageInput !== undefined) {
    provenance.supportsImageInput = CONFIG_SOURCE.EXPLICIT_PROFILE
  } else if (safeConfig.supportsImageInput === true) {
    provenance.supportsImageInput = CONFIG_SOURCE.EXPLICIT_REQUEST
  } else {
    assume('supportsImageInput', 'image input assumed unsupported; a model may in fact accept images')
  }
  if (overrides.reasoningTransport !== undefined) {
    provenance.reasoningTransport = CONFIG_SOURCE.EXPLICIT_PROFILE
  } else {
    infer('reasoningTransport', `reasoning transport "${profile.reasoningTransport}" inferred from the model family`)
  }

  // --- planner ---------------------------------------------------------------
  provenance['planner.enabled'] =
    overrides.plannerEnabled !== undefined ? CONFIG_SOURCE.EXPLICIT_PROFILE : CONFIG_SOURCE.DERIVED
  provenance['planner.thinkingLevel'] =
    overrides.plannerThinkingLevel !== undefined ? CONFIG_SOURCE.EXPLICIT_PROFILE : CONFIG_SOURCE.DERIVED
  if (overrides.plannerMaxOutputTokens !== undefined) {
    provenance['planner.maxOutputTokens'] = CONFIG_SOURCE.EXPLICIT_PROFILE
  } else {
    assume('planner.maxOutputTokens', `planner output budget defaulted to ${profile.planner.maxOutputTokens}`)
  }
  reportClamp(
    clamped,
    'planner.maxOutputTokens',
    overrides.plannerMaxOutputTokens,
    profile.planner.maxOutputTokens,
    CONFIG_BOUNDS.plannerMaxOutputTokens,
  )

  // --- retry policy ----------------------------------------------------------
  for (const [field, bounds] of Object.entries({
    maxRetries: CONFIG_BOUNDS.maxRetries,
    providerMaxRetries: CONFIG_BOUNDS.providerMaxRetries,
    providerMaxRetryDelayMs: CONFIG_BOUNDS.providerMaxRetryDelayMs,
  })) {
    const key = `runtime.${field}`
    if (overrides[field] !== undefined) provenance[key] = CONFIG_SOURCE.EXPLICIT_PROFILE
    else assume(key, `retry setting defaulted to ${profile.runtime[field]}`)
    reportClamp(clamped, key, overrides[field], profile.runtime[field], bounds)
  }
  provenance['runtime.reserveTokens'] = CONFIG_SOURCE.DERIVED
  provenance['runtime.keepRecentTokens'] = CONFIG_SOURCE.DERIVED

  // --- compat ----------------------------------------------------------------
  for (const key of Object.keys(profile.compat ?? {})) {
    provenance[`compat.${key}`] =
      isPlainObject(overrides.compat) && key in overrides.compat
        ? CONFIG_SOURCE.EXPLICIT_PROFILE
        : CONFIG_SOURCE.API_SHAPE
  }

  // --- supplied but never read ----------------------------------------------
  for (const field of MODEL_PROFILE_ONLY_FIELDS) {
    if (safeConfig[field] === undefined) continue
    rejected.push({
      field,
      requested: safeConfig[field],
      applied: null,
      reason: 'ignored_by_resolver_supply_it_under_modelProfile',
    })
  }

  return Object.freeze({
    schemaVersion: MODEL_CONFIG_SCHEMA_VERSION,
    raw,
    resolved: {
      ...modelProfileSnapshot(profile),
      compat: profile.compat,
      planner: profile.planner,
      runtime: profile.runtime,
    },
    provenance,
    clamped,
    rejected,
    assumptions,
    unverified: unverifiedCapabilities({ provenance }),
    warnings,
    overrideOrder: PROFILE_OVERRIDE_ORDER,
    evidenceLevel: CONFIG_EVIDENCE_LEVEL.CONFIG_RESOLUTION,
  })
}

/* ---------------------------------------------------------------------------
 * The wire vocabulary, and why it is not invented here
 *
 * `API_PARAMETER_FIELDS` is the union of the fields the production serializers really emit. It
 * is built from two independent sources, both offline:
 *   1. the adapter sources under @earendil-works/pi-ai/dist/api/*.js (plus Fox's own
 *      src/native-model-proxy.mjs for the responses shape, which rewrites the budget and store);
 *   2. a capture of the payloads those adapters actually build — the official
 *      StreamOptions.onPayload hook plus a stubbed fetch, so no socket is opened.
 * test/model-wire-contract.test.mjs re-runs that capture and fails if the adapter ever emits a
 * field this table does not declare: a table narrower than the real wire would make us report a
 * field production sends as "unsupported", which is exactly the drift this is here to stop.
 *
 * Re-check the table whenever the adapter version in services/agent-runtime/package.json moves.
 * ------------------------------------------------------------------------- */

/** Which serializer owns each shape's vocabulary, so drift can be traced to a version bump. */
export const WIRE_VOCABULARY_SOURCES = Object.freeze({
  'openai-completions': Object.freeze({
    serializer: '@earendil-works/pi-ai/api/openai-completions',
  }),
  'openai-responses': Object.freeze({
    serializer: '@earendil-works/pi-ai/api/openai-responses',
    foxOverrides: 'src/native-model-proxy.mjs sets max_output_tokens and store on the upstream body',
  }),
  'anthropic-messages': Object.freeze({
    serializer: '@earendil-works/pi-ai/api/anthropic-messages',
  }),
  faux: Object.freeze({
    serializer: 'src/pi-adapter.mjs (faux provider, Fox-owned)',
  }),
})

/** Request fields each API shape can carry. Anything else is rejected, never dropped silently. */
export const API_PARAMETER_FIELDS = Object.freeze({
  'openai-completions': Object.freeze([
    'model',
    'messages',
    'stream',
    'stream_options',
    'store',
    'max_tokens',
    'max_completion_tokens',
    'temperature',
    'tools',
    'tool_choice',
    'tool_stream',
    'reasoning_effort',
    'reasoning',
    'prompt_cache_key',
    'prompt_cache_retention',
    'thinking',
    'thinking_token_budget',
    'enable_thinking',
    'chat_template_kwargs',
  ]),
  'openai-responses': Object.freeze([
    'model',
    'input',
    'stream',
    'store',
    'max_output_tokens',
    'temperature',
    'tools',
    'tool_choice',
    'reasoning',
    'include',
    'prompt_cache_key',
    'prompt_cache_retention',
    'prompt_cache_options',
    'service_tier',
  ]),
  'anthropic-messages': Object.freeze([
    'model',
    'messages',
    'system',
    'stream',
    'max_tokens',
    'temperature',
    'tools',
    'tool_choice',
    'thinking',
    'metadata',
    'output_config',
  ]),
  faux: Object.freeze(['model', 'messages', 'max_tokens']),
})

/**
 * Fields the OpenAI-compatible adapters merge into the body as-is from `samplingParams` rather
 * than as named request fields. They do reach the wire, but only through that channel, so a
 * caller asking for one here is told which channel has to carry it.
 */
export const SAMPLING_PASSTHROUGH_FIELDS = Object.freeze({
  'openai-completions': Object.freeze(['top_p', 'top_k', 'min_p', 'repetition_penalty']),
  'openai-responses': Object.freeze(['top_p', 'top_k', 'min_p', 'repetition_penalty']),
  'anthropic-messages': Object.freeze([]),
  faux: Object.freeze([]),
})

/**
 * Fields a caller can ask for that the current adapters do NOT put on the wire at all. Reporting
 * these as "allowed" would be a fiction; reporting them as rejected with this reason is the truth.
 * `parallel_tool_calls` in particular is enforced by the compatibility prompt
 * (modelProfilePrompt), not by a request parameter.
 */
export const ADAPTER_UNSENT_FIELDS = Object.freeze({
  'openai-completions': Object.freeze(['parallel_tool_calls']),
  'openai-responses': Object.freeze(['parallel_tool_calls']),
  'anthropic-messages': Object.freeze(['parallel_tool_calls']),
  faux: Object.freeze(['parallel_tool_calls']),
})

/** A parameter is only serialized when the resolved profile says the endpoint accepts it. */
export const PARAMETER_GATES = Object.freeze({
  reasoning_effort: (profile) => profile?.compat?.supportsReasoningEffort === true,
  store: (profile) => profile?.compat?.supportsStore !== false,
  tools: (profile) => profile?.supportsTools !== false,
  tool_choice: (profile) => profile?.supportsTools !== false,
  thinking: () => false,
})

/** The caller-facing output-budget field, and the wire names it can map onto. */
export const NEUTRAL_OUTPUT_BUDGET_FIELD = 'maxOutputTokens'
export const TOKEN_LIMIT_FIELDS = Object.freeze(['max_tokens', 'max_completion_tokens', 'max_output_tokens'])
/** A wire budget must be a positive integer; zero and negatives are not reservable amounts. */
export const LEGAL_BUDGET_FLOOR = 1

/**
 * The wire budget field a shape uses when the resolved profile declares none. This mirrors each
 * serializer's own default branch rather than a convenient guess: the openai-completions adapter
 * writes max_completion_tokens for every compat that does not explicitly name max_tokens, which is
 * why a provider whose Fox compat stays empty (openrouter) still gets max_completion_tokens.
 * The offline capture in test/model-wire-contract.test.mjs is what holds this to the adapter.
 */
export const API_DEFAULT_MAX_TOKENS_FIELD = Object.freeze({
  'openai-completions': 'max_completion_tokens',
  'openai-responses': 'max_output_tokens',
  'anthropic-messages': 'max_tokens',
  faux: 'max_tokens',
})

/**
 * Which budget wins when a caller supplies more than one. This used to be decided accidentally by
 * the order of TOKEN_LIMIT_FIELDS; it is now a stated rule, and every losing field is reported.
 */
export const OUTPUT_BUDGET_PRECEDENCE = Object.freeze([
  'requested.maxOutputTokens — neutral, profile-independent',
  'requested[<profile.compat.maxTokensField>] — this profile\'s own wire field',
  'no budget requested: nothing is serialized, and resolved.maxOutputTokens applies at the adapter',
])

/**
 * Evidence levels. `applied_verified` is reserved for an observation made against a real provider
 * and is NOT reachable from any offline path in this module — a pure-function prediction may never
 * be recorded as a verified send.
 */
export const CONFIG_EVIDENCE_LEVEL = Object.freeze({
  CONFIG_RESOLUTION: 'offline_config_resolution_only',
  SERIALIZATION_PREDICTION: 'offline_serialization_prediction',
  ADAPTER_CAPTURE: 'offline_adapter_capture',
  APPLIED_VERIFIED: 'applied_verified',
})

/** Pick the one token-limit field this profile serializes, validated against the shape's table. */
function resolveMaxTokensField(profile, api, allowed) {
  const compatible = TOKEN_LIMIT_FIELDS.filter((field) => allowed.includes(field))
  const declared = profile?.compat?.maxTokensField ?? null
  if (declared !== null && allowed.includes(declared)) {
    return { field: declared, source: 'profile_compat', rejectedField: null }
  }
  const preferred = API_DEFAULT_MAX_TOKENS_FIELD[api] ?? compatible[0] ?? null
  const field = compatible.includes(preferred) ? preferred : compatible[0] ?? null
  const rejectedField =
    declared !== null && !allowed.includes(declared)
      ? {
          field: 'compat.maxTokensField',
          requested: declared,
          applied: field,
          reason: 'compat_names_a_field_the_api_shape_cannot_carry',
        }
      : null
  return { field, source: compatible.length === 0 ? 'none' : 'api_shape_default', rejectedField }
}

/**
 * Build the "sent" layer: the parameters this profile would serialize for this request.
 *
 * This is a PREDICTION, not a capture — `evidenceLevel` says so and the wire vocabulary it
 * predicts from is pinned to the real adapters by test/model-wire-contract.test.mjs. It must
 * never be reported as `applied_verified`: only a real provider response can establish that.
 *
 * Requested fields the API shape cannot carry, that the shape's adapter never emits, or that the
 * profile says the endpoint will not accept come back in `rejected` with a reason. A value that
 * had to be changed to be legal comes back in `clamped` — a budget is never passed through as a
 * negative, zero, non-integer or non-numeric value.
 */
export function buildRequestParameters(profile, requested = {}) {
  const api = API_PARAMETER_FIELDS[profile?.api] ? profile.api : 'openai-completions'
  const allowed = API_PARAMETER_FIELDS[api]
  const sampling = SAMPLING_PASSTHROUGH_FIELDS[api] ?? []
  const unsent = ADAPTER_UNSENT_FIELDS[api] ?? []
  const { field: maxTokensField, source: maxTokensFieldSource, rejectedField } = resolveMaxTokensField(profile, api, allowed)
  const sent = {}
  const rejected = []
  const clamped = []
  const passthrough = []
  if (rejectedField) rejected.push(rejectedField)

  for (const [field, value] of Object.entries(requested ?? {})) {
    // Neutral budget field and both wire budget names are decided together, below.
    if (field === NEUTRAL_OUTPUT_BUDGET_FIELD || TOKEN_LIMIT_FIELDS.includes(field)) continue
    if (unsent.includes(field)) {
      rejected.push({ field, requested: value, reason: 'not_serialized_by_the_adapter' })
      continue
    }
    if (sampling.includes(field)) {
      // Not a named field of the shape: it reaches the body through the adapter's sampling-params
      // channel, and only if the caller also sets it there. Recorded as passthrough, not as syntax.
      sent[field] = value
      passthrough.push({
        field,
        channel: 'samplingParams',
        note: 'reaches the body only if the caller also sets it as a sampling parameter',
      })
      continue
    }
    if (!allowed.includes(field)) {
      rejected.push({ field, requested: value, reason: 'unsupported_by_api_shape' })
      continue
    }
    if (field === 'thinking') {
      // No per-provider thinking payload has been verified offline, so we refuse to invent one.
      rejected.push({ field, requested: value, reason: 'no_verified_thinking_mapping' })
      continue
    }
    const gate = PARAMETER_GATES[field]
    if (gate && !gate(profile)) {
      rejected.push({ field, requested: value, reason: 'unsupported_by_profile' })
      continue
    }
    sent[field] = value
  }

  // --- output budget: one field wins, everything else is reported ------------------------------
  const neutral = requested?.[NEUTRAL_OUTPUT_BUDGET_FIELD]
  const hasNeutral = neutral !== undefined && neutral !== null
  const ownWire = maxTokensField === null ? undefined : requested?.[maxTokensField]
  const hasOwnWire = ownWire !== undefined && ownWire !== null

  for (const field of TOKEN_LIMIT_FIELDS) {
    if (requested?.[field] === undefined) continue
    if (field === maxTokensField) {
      if (hasNeutral) {
        rejected.push({ field, requested: requested[field], reason: 'superseded_by_neutral_output_budget' })
      }
      continue
    }
    rejected.push({
      field,
      requested: requested[field],
      reason: allowed.includes(field) ? 'wrong_token_limit_field_for_profile' : 'unsupported_by_api_shape',
    })
  }

  // The profile's own resolved budget is both the ceiling and the fallback: it is the value this
  // profile would have used had the caller not asked for anything.
  const profileBudget = Number.isFinite(profile?.maxOutputTokens)
    ? Math.round(profile.maxOutputTokens)
    : CONFIG_BOUNDS.maxOutputTokens.fallback
  const bounds = {
    minimum: CONFIG_BOUNDS.maxOutputTokens.minimum,
    maximum: Math.max(CONFIG_BOUNDS.maxOutputTokens.minimum, profileBudget),
  }
  const requestedBudget = hasNeutral ? neutral : hasOwnWire ? ownWire : undefined
  const budgetSource = hasNeutral ? NEUTRAL_OUTPUT_BUDGET_FIELD : hasOwnWire ? maxTokensField : 'not_requested'
  let appliedBudget = null

  if (requestedBudget !== undefined && maxTokensField !== null) {
    const numeric = typeof requestedBudget === 'number' ? requestedBudget : Number(requestedBudget)
    const rounded = Number.isFinite(numeric) ? Math.round(numeric) : null
    if (!Number.isFinite(numeric)) {
      // A budget that is not a number at all cannot be sent in any legal form. Fall back to what
      // this profile would have used anyway and say so, rather than passing the raw value through.
      appliedBudget = profileBudget
      clamped.push({ field: NEUTRAL_OUTPUT_BUDGET_FIELD, requested: requestedBudget, applied: appliedBudget, reason: 'not_a_number' })
    } else if (rounded < LEGAL_BUDGET_FLOOR) {
      // Not a budget: zero and negatives are not reservable amounts. Moved to the smallest budget
      // the profile envelope considers meaningful, with the reason recorded.
      appliedBudget = bounds.minimum
      clamped.push({ field: NEUTRAL_OUTPUT_BUDGET_FIELD, requested: requestedBudget, applied: appliedBudget, reason: 'below_minimum', minimum: bounds.minimum })
    } else if (rounded > bounds.maximum) {
      appliedBudget = bounds.maximum
      clamped.push({ field: NEUTRAL_OUTPUT_BUDGET_FIELD, requested: requestedBudget, applied: appliedBudget, reason: 'above_profile_cap', maximum: bounds.maximum })
    } else {
      // A legal positive integer stays exactly as the caller asked, even below the profile
      // envelope's own floor: that floor bounds a persisted profile, not a per-request budget.
      appliedBudget = rounded
      if (rounded !== numeric) {
        clamped.push({ field: NEUTRAL_OUTPUT_BUDGET_FIELD, requested: requestedBudget, applied: appliedBudget, reason: 'rounded' })
      } else if (typeof requestedBudget !== 'number') {
        clamped.push({ field: NEUTRAL_OUTPUT_BUDGET_FIELD, requested: requestedBudget, applied: appliedBudget, reason: 'coerced_to_number' })
      }
    }
    sent[maxTokensField] = appliedBudget
  } else if (requestedBudget !== undefined && maxTokensField === null) {
    rejected.push({ field: NEUTRAL_OUTPUT_BUDGET_FIELD, requested: requestedBudget, reason: 'api_shape_has_no_token_limit_field' })
  }

  return Object.freeze({
    schemaVersion: MODEL_CONFIG_SCHEMA_VERSION,
    api,
    profileId: profile?.id ?? null,
    maxTokensField,
    maxTokensFieldSource,
    sent,
    rejected,
    clamped,
    passthrough,
    budget: Object.freeze({
      field: maxTokensField,
      source: budgetSource,
      requested: requestedBudget ?? null,
      applied: appliedBudget,
      changed: clamped.length > 0,
    }),
    allowedFields: [...allowed],
    budgetPrecedence: OUTPUT_BUDGET_PRECEDENCE,
    evidenceLevel: CONFIG_EVIDENCE_LEVEL.SERIALIZATION_PREDICTION,
    evidenceNote:
      'pure-function prediction from the declared wire vocabulary; NOT a capture of a real request and never applied_verified',
  })
}
