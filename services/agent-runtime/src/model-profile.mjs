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
  const contextWindow = boundedNumber(config.contextWindow, 128_000, 4_096, 4_000_000)
  const maxOutputTokens = boundedNumber(config.maxOutputTokens, 8_192, 256, Math.min(contextWindow, 131_072))
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
      maxOutputTokens: boundedNumber(overrides.plannerMaxOutputTokens, 2_048, 512, 8_192),
    },
    runtime: {
      // Turn-level retry: re-running a whole agent turn after a terminal turn
      // failure. Owned independently from the provider HTTP retry below so that a
      // rate-limited request is never retried by both layers (double cost / 429
      // amplification). The Fox Kernel is the authority for turn-retry policy; the
      // sidecar defaults it off and only enables it when the Host configures it.
      maxRetries: boundedNumber(overrides.maxRetries, 0, 0, 5),
      // Provider HTTP retry: bounded retries of a single request for transient
      // transport/5xx/429 failures, honouring the server's Retry-After header.
      providerMaxRetries: boundedNumber(overrides.providerMaxRetries, 2, 0, 5),
      providerMaxRetryDelayMs: boundedNumber(overrides.providerMaxRetryDelayMs, 8_000, 500, 60_000),
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
