import type { SaveModelProviderInput } from '../use-model-providers'
import type { ModelProviderRecord } from '@/features/conversations/model/types'
export type ServiceDraft = SaveModelProviderInput & { apiKey: string; clearApiKey: boolean }
export type ServicePreset = {
  id: string
  name: string
  icon: string
  baseUrl: string
  apiType: ServiceDraft['apiType']
  modelPlaceholder: string
  contextWindow: number
  maxOutputTokens: number
  supportsImageInput: boolean
}

// Keep these defaults aligned with Yuxi's builtin provider catalog.
export const servicePresets: ServicePreset[] = [
  { id: 'openai', name: 'OpenAI', icon: 'openai.svg', baseUrl: 'https://api.openai.com/v1', apiType: 'openai-completions', modelPlaceholder: 'gpt-4.1', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'deepseek', name: 'DeepSeek 官方', icon: 'deepseek.svg', baseUrl: 'https://api.deepseek.com', apiType: 'openai-completions', modelPlaceholder: 'deepseek-flash', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false },
  { id: 'alibaba', name: 'DashScope', icon: 'alibaba.svg', baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1', apiType: 'openai-completions', modelPlaceholder: 'qwen3-max', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: true },
  { id: 'alibaba-coding-plan-cn', name: 'Aliyun Coding Plan', icon: 'alibaba-cloud.svg', baseUrl: 'https://coding.dashscope.aliyuncs.com/v1', apiType: 'openai-completions', modelPlaceholder: 'qwen3-coder-plus', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'alibaba-coding-plan', name: 'Aliyun Coding Plan (International)', icon: 'alibaba-cloud.svg', baseUrl: 'https://coding-intl.dashscope.aliyuncs.com/v1', apiType: 'openai-completions', modelPlaceholder: 'qwen3-coder-plus', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'zhipuai', name: 'Zhipu (BigModel)', icon: 'zhipu.svg', baseUrl: 'https://open.bigmodel.cn/api/paas/v4', apiType: 'openai-completions', modelPlaceholder: 'glm-4.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'zhipuai-coding-plan', name: 'Zhipu Coding Plan (BigModel)', icon: 'zhipu.svg', baseUrl: 'https://open.bigmodel.cn/api/coding/paas/v4', apiType: 'openai-completions', modelPlaceholder: 'glm-4.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'zai', name: 'Zhipu (Z.AI)', icon: 'zai.svg', baseUrl: 'https://api.z.ai/api/paas/v4', apiType: 'openai-completions', modelPlaceholder: 'glm-4.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'zai-coding-plan', name: 'Zhipu Coding Plan (Z.AI)', icon: 'zai.svg', baseUrl: 'https://api.z.ai/api/coding/paas/v4', apiType: 'openai-completions', modelPlaceholder: 'glm-4.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'xiaomi-token-plan-cn', name: 'XiaomiMiMo Token Plan', icon: 'xiaomi.svg', baseUrl: 'https://token-plan-cn.xiaomimimo.com/v1', apiType: 'openai-completions', modelPlaceholder: 'mimo-v2-flash', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'xiaomi', name: 'XiaomiMiMo', icon: 'xiaomi.svg', baseUrl: 'https://api.xiaomimimo.com/v1', apiType: 'openai-completions', modelPlaceholder: 'mimo-v2-flash', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'kimi-for-coding', name: 'Kimi Code', icon: 'moonshot.svg', baseUrl: 'https://api.kimi.com/coding/v1', apiType: 'openai-completions', modelPlaceholder: 'kimi-for-coding', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'moonshotai-cn', name: 'Moonshot', icon: 'moonshot.svg', baseUrl: 'https://api.moonshot.cn/v1', apiType: 'openai-completions', modelPlaceholder: 'kimi-k2.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'moonshotai', name: 'Moonshot (International)', icon: 'moonshot.svg', baseUrl: 'https://api.moonshot.ai/v1', apiType: 'openai-completions', modelPlaceholder: 'kimi-k2.5', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'minimax-cn', name: 'MiniMax', icon: 'minimax.svg', baseUrl: 'https://api.minimaxi.com/v1', apiType: 'openai-completions', modelPlaceholder: 'MiniMax-M2.5', contextWindow: 204800, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'minimax', name: 'MiniMax (International)', icon: 'minimax.svg', baseUrl: 'https://api.minimax.io/v1', apiType: 'openai-completions', modelPlaceholder: 'MiniMax-M2.5', contextWindow: 204800, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'openrouter', name: 'OpenRouter', icon: 'openrouter.svg', baseUrl: 'https://openrouter.ai/api/v1', apiType: 'openai-completions', modelPlaceholder: 'openai/gpt-4.1', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: true },
  { id: 'modelscope', name: 'ModelScope', icon: 'modelscope.svg', baseUrl: 'https://api-inference.modelscope.cn/v1', apiType: 'openai-completions', modelPlaceholder: 'Qwen/Qwen3-235B-A22B', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false },
  { id: 'opencode', name: 'OpenCode', icon: 'opencode.svg', baseUrl: 'https://opencode.ai/zen/v1', apiType: 'openai-completions', modelPlaceholder: 'opencode/big-pickle', contextWindow: 128000, maxOutputTokens: 16384, supportsImageInput: false },
  { id: 'siliconflow-cn', name: 'SiliconFlow', icon: 'siliconflow.svg', baseUrl: 'https://api.siliconflow.cn/v1', apiType: 'openai-completions', modelPlaceholder: 'deepseek-ai/DeepSeek-V4-Flash', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false },
  { id: 'siliconflow', name: 'SiliconFlow (International)', icon: 'siliconflow.svg', baseUrl: 'https://api.siliconflow.com/v1', apiType: 'openai-completions', modelPlaceholder: 'deepseek-ai/DeepSeek-V4-Flash', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false },
]

export const providerIconOptions = Array.from(new Map(servicePresets.map((preset) => [preset.icon, preset])).values())

export function providerIcon(name: string, baseUrl: string, customIcon?: string | null) {
  if (customIcon) return `/providers/${customIcon}`
  const preset = servicePresets.find((item) => item.baseUrl === baseUrl)
    ?? servicePresets.find((item) => name.toLowerCase().includes(item.id.split('-')[0]))
  return preset ? `/providers/${preset.icon}` : null
}

export function presetDraft(preset: ServicePreset, isDefault: boolean): ServiceDraft {
  return {
    name: preset.name,
    icon: preset.icon,
    baseUrl: preset.baseUrl,
    apiType: preset.apiType,
    protocolProfiles: preset.id === 'deepseek' ? [
      { id: 'primary', presetId: 'deepseek-official', apiType: preset.apiType, baseUrl: preset.baseUrl, modelId: '', separateCredential: false },
      { id: crypto.randomUUID(), presetId: 'deepseek-official', apiType: 'anthropic-messages', baseUrl: 'https://api.deepseek.com/anthropic', modelId: '', separateCredential: false },
      { id: crypto.randomUUID(), presetId: 'deepseek-official', apiType: 'openai-responses', baseUrl: 'https://api.deepseek.com', modelId: '', separateCredential: false },
    ] : preset.id === 'openai' ? [
      { id: 'primary', presetId: 'openai', apiType: preset.apiType, baseUrl: preset.baseUrl, modelId: '', separateCredential: false },
      { id: crypto.randomUUID(), presetId: 'openai', apiType: 'openai-responses', baseUrl: 'https://api.openai.com/v1', modelId: '', separateCredential: false },
    ] : [],
    isDefault,
    apiKey: '',
    clearApiKey: false,
    models: [{
      modelId: '',
      displayName: '',
      contextWindow: preset.contextWindow,
      maxOutputTokens: preset.maxOutputTokens,
      supportsImageInput: preset.supportsImageInput,
      isDefault: true,
    }],
  }
}

export function serviceDraft(provider?: ModelProviderRecord): ServiceDraft {
  return provider ? {
    id: provider.id, name: provider.name, icon: provider.icon, baseUrl: provider.baseUrl, apiType: provider.apiType,
    isDefault: provider.isDefault, apiKey: '', clearApiKey: false,
    protocolProfiles: (provider.protocolProfiles ?? []).map(profile => ({ ...profile })), protocolKeys: {},
    models: provider.models.map((item) => ({ ...item })),
  } : {
    name: '', icon: null, baseUrl: '', apiType: 'openai-completions', isDefault: true, apiKey: '', clearApiKey: false,
    models: [{ modelId: '', displayName: '', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false, isDefault: true }],
  }
}

