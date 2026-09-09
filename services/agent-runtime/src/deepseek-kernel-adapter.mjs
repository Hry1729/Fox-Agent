// Use the Harness public single-attempt LLM API, not its agent loop. No
// filesystem, terminal, tool executor, session store or retry plugin is mounted.
import { randomUUID } from 'node:crypto'
import { DeepSeekAdapter, resolveAdapterOptions } from '@deepseek-ai/dsh-llm-deepseek'
import { BlockAssembler, createUserMessage, createAssistantMessage, createToolResultMessage } from '@deepseek-ai/dsh-llm'
import { nativeModelSettings } from './native-model-settings.mjs'

export const DEEPSEEK_KERNEL_ADAPTER = 'deepseek-harness-0.1.2-rc.1/fox-kernel-worker-v1'
const fail = () => new Error('DeepSeek Harness Kernel adapter rejected unsupported content or an unfinished model response')

export function deepSeekHistory(messages, model) {
  const content = value => (typeof value === 'string' ? [{ type: 'text', text: value }] : value).map(block => {
    if (block.type === 'text') return { type: 'text', text: block.text }
    if (block.type === 'thinking') return { type: 'reasoning', text: block.thinking }
    if (block.type === 'toolCall') return { type: 'tool-call', id: block.id, name: block.name, arguments: JSON.stringify(block.arguments) }
    // Never silently discard an image or fabricate attachment provenance.
    throw fail()
  })
  return messages.map(message => {
    if (message.role === 'toolResult') return createToolResultMessage({ callId: message.toolCallId, content: content(message.content), isError: message.isError })
    if (message.role === 'assistant') return createAssistantMessage({ content: content(message.content), source: { provider: 'deepseek-official', model } })
    if (message.role === 'user') return createUserMessage({ content: content(message.content), source: { kind: 'user' } })
    throw fail()
  })
}

export async function runDeepSeekKernelModel({ config, messages, apiKey, signal, onPreview }) {
  const service = nativeModelSettings(config.modelService)
  if (service.apiType !== 'openai-completions' || config.nativeAdapter || signal?.aborted) throw fail()
  const maxTokens = service.maxOutputTokens ?? 8192
  if (!Number.isSafeInteger(maxTokens) || maxTokens < 1 || maxTokens > 131072) throw fail()
  const effort = service.reasoning === false ? 'off' : service.thinkingLevel
  if (effort !== undefined && !['off','low','high','max'].includes(effort)) throw fail()
  const connection = resolveAdapterOptions({ baseURL: service.baseUrl, maxTokens,
    defaultContextWindow: service.contextWindow ?? 128000,
    thinking: service.reasoning === false ? 'disabled' : 'enabled',
    ...(effort ? { reasoningEffort: effort } : {}), streamIdleTimeoutMs: 120000,
  })
  const anonymousId = randomUUID()
  const adapter = new DeepSeekAdapter({ options: () => connection, resolveApiKey: async () => apiKey,
    resolveUserId: () => anonymousId, prepareExtensions: async () => ({ fields: {}, accept: async () => {} }),
  })
  const history = deepSeekHistory(messages, service.modelId)
  const assembly = new BlockAssembler()
  let size = 0
  for await (const chunk of adapter.stream({ provider: 'deepseek-official', model: service.modelId,
    messages: history, system: config.systemPrompt, tools: config.proposalTools, maxTokens, signal,
    ...(effort ? { reasoningEffort: effort } : {}),
  })) {
    size += Buffer.byteLength(JSON.stringify(chunk), 'utf8')
    if (size > 2_097_152 || signal?.aborted) throw fail()
    assembly.push(chunk)
    if (onPreview && ['text-delta','block-end'].includes(chunk.type)) onPreview(assembly.blocks().filter(block => block.type === 'text').map(block => block.text).join(''))
  }
  if (signal?.aborted || !['stop','tool-calls'].includes(assembly.finish?.kind)) throw fail()
  const content = assembly.blocks().map(block => {
    if (block.type === 'text') return { type: 'text', text: block.text }
    if (block.type === 'reasoning') return { type: 'thinking', thinking: block.text }
    if (block.type === 'tool-call') {
      const argumentsValue = JSON.parse(block.arguments)
      if (!config.proposalTools.some(tool => tool.name === block.name) || !argumentsValue || typeof argumentsValue !== 'object' || Array.isArray(argumentsValue)) throw fail()
      return { type: 'toolCall', id: block.id, name: block.name, arguments: argumentsValue }
    }
    throw fail()
  })
  const u = assembly.usage
  const usage = u ? { input: u.inputTokens, output: u.outputTokens,
    ...(u.totalTokens === undefined ? {} : { totalTokens: u.totalTokens }), cacheRead: u.cacheReadTokens ?? 0, cacheWrite: u.cacheWriteTokens ?? 0 } : {}
  return { role: 'assistant', content, stopReason: assembly.finish.kind === 'tool-calls' ? 'toolUse' : 'stop',
    api: 'openai-completions', provider: 'deepseek_harness', model: service.modelId, usage, timestamp: Date.now() }
}
