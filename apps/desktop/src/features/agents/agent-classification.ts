import type {
  AgentInvocationMode,
  AgentKind,
  AgentRecord,
  AgentVisibility,
} from '@/features/conversations/model/types'

export type ClassifiedAgentRecord = AgentRecord & {
  agentKind: AgentKind
  invocationMode: AgentInvocationMode
  visibility: AgentVisibility
}

const legacyExpertIds = new Set([
  'fox-debugger',
  'fox-frontend',
  'fox-reviewer',
  'fox-security',
  'fox-architect',
])

const classificationDefaults: Record<AgentKind, {
  invocationMode: AgentInvocationMode
  visibility: AgentVisibility
}> = {
  assistant: { invocationMode: 'primary', visibility: 'chat_selector' },
  expert: { invocationMode: 'inline', visibility: 'expert_center' },
  worker: { invocationMode: 'child', visibility: 'hidden' },
}

function inferredAgentKind(agent: AgentRecord): AgentKind {
  if (agent.invocationMode === 'child' || agent.visibility === 'hidden') return 'worker'
  if (agent.invocationMode === 'inline' || agent.visibility === 'expert_center') return 'expert'
  if (legacyExpertIds.has(agent.id)) return 'expert'
  return 'assistant'
}

export function normalizeAgentClassification(agent: AgentRecord): ClassifiedAgentRecord {
  const agentKind = agent.agentKind ?? inferredAgentKind(agent)
  const defaults = classificationDefaults[agentKind]
  return {
    ...agent,
    agentKind,
    invocationMode: agent.invocationMode ?? defaults.invocationMode,
    visibility: agent.visibility ?? defaults.visibility,
  }
}

export function chatSelectableAgents(agents: AgentRecord[]): ClassifiedAgentRecord[] {
  return agents
    .map(normalizeAgentClassification)
    .filter((agent) => agent.available
      && agent.agentKind === 'assistant'
      && agent.invocationMode === 'primary'
      && agent.visibility === 'chat_selector')
}

export function resolveActiveChatAgent(agents: AgentRecord[], selectedAgentId?: string | null): AgentRecord | null {
  // An unavailable/temporarily missing selection must not be displayed as a
  // different assistant while dispatch still targets the selected conversation.
  if (selectedAgentId) return agents.find((agent) => agent.id === selectedAgentId) ?? null
  const selectable = chatSelectableAgents(agents)
  return selectable.find((agent) => agent.isDefault) ?? selectable[0] ?? null
}

export function expertCatalogAgents(agents: AgentRecord[]): ClassifiedAgentRecord[] {
  return agents
    .map(normalizeAgentClassification)
    .filter((agent) => agent.agentKind === 'expert'
      && agent.invocationMode === 'inline'
      && agent.visibility === 'expert_center')
}
