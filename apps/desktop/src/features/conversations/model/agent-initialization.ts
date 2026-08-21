import type { RuntimeInitialization } from './types'

export interface ResolvedConversationAgent {
  agentId: string
  initialized: boolean
}

export async function resolveConversationAgentId(
  candidates: Array<string | null | undefined>,
  initialize: () => Promise<Pick<RuntimeInitialization, 'defaultAgentId'>>,
): Promise<ResolvedConversationAgent> {
  const selected = candidates
    .map((candidate) => candidate?.trim() ?? '')
    .find(Boolean)

  if (selected) return { agentId: selected, initialized: false }

  const initialization = await initialize()
  const agentId = initialization.defaultAgentId.trim()
  if (!agentId) throw new Error('默认专家初始化失败，请重试')
  return { agentId, initialized: true }
}
