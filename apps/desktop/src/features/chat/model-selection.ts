import type { AgentRecord, YuxiModelRecord } from '@/features/conversations/model/types'

export function yuxiModelOverrideAllowed(agent?: AgentRecord | null) {
  if (!agent || agent.runtimeType !== 'yuxi') return false
  const items = agent.configurableItems
  if (!items || typeof items !== 'object' || Array.isArray(items)) return false
  const model = (items as Record<string, unknown>).model
  return Boolean(model && typeof model === 'object' && (model as Record<string, unknown>).kind === 'llm')
}

export function conversationModelOverride(
  agent: AgentRecord | null | undefined,
  model: string | undefined,
  remoteModels: Pick<YuxiModelRecord, 'spec'>[],
): string | undefined {
  const selected = model?.trim()
  if (!agent || !selected || selected === '未配置模型') return undefined
  if (agent.runtimeType !== 'yuxi') return selected
  if (!yuxiModelOverrideAllowed(agent)) return undefined
  // Local provider IDs and remote default labels are not Yuxi model specs.
  // Omitting the override lets the remote agent resolve its configured default.
  return remoteModels.some(({ spec }) => spec === selected) ? selected : undefined
}
