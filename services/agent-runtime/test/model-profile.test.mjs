import test from 'node:test'
import assert from 'node:assert/strict'
import { modelProfilePrompt, modelProfileSnapshot, resolveModelProfile, transportProvider } from '../src/model-profile.mjs'

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
