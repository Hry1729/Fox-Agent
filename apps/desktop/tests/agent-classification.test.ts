import { describe, expect, test } from 'bun:test'
import {
  chatSelectableAgents,
  expertCatalogAgents,
  normalizeAgentClassification,
  resolveActiveChatAgent,
} from '../src/features/agents/agent-classification'
import type { AgentRecord } from '../src/features/conversations/model/types'

function agent(id: string, overrides: Partial<AgentRecord> = {}): AgentRecord {
  return {
    id,
    name: id,
    description: '',
    runtimeType: 'pi',
    defaultModel: '',
    icon: null,
    category: 'general',
    openingSuggestions: [],
    systemPrompt: '',
    isBuiltin: true,
    packageVersion: '1.0.0',
    packageManifest: {},
    capabilities: {},
    resources: { tools: [], knowledges: [], mcps: [], skills: [] },
    configurableItems: {},
    isDefault: false,
    available: true,
    ...overrides,
  }
}

describe('agent classification', () => {
  test('preserves a selected offline remote agent instead of presenting the local default', () => {
    const local = agent('fox-general', { isDefault: true })
    const remote = agent('yuxi:default-chatbot', { runtimeType: 'yuxi', available: false })
    expect(resolveActiveChatAgent([local, remote], remote.id)).toBe(remote)
    expect(resolveActiveChatAgent([local], remote.id)).toBeNull()
    expect(resolveActiveChatAgent([local, remote], null)?.id).toBe(local.id)
  })
  test('normalizes missing legacy fields without overwriting explicit classification', () => {
    expect(normalizeAgentClassification(agent('fox-debugger'))).toMatchObject({
      agentKind: 'expert',
      invocationMode: 'inline',
      visibility: 'expert_center',
    })
    expect(normalizeAgentClassification(agent('remote-worker', {
      agentKind: 'worker',
      invocationMode: 'child',
      visibility: 'hidden',
    }))).toMatchObject({
      agentKind: 'worker',
      invocationMode: 'child',
      visibility: 'hidden',
    })
  })

  test('keeps only available primary assistants in the chat selector', () => {
    const records = [
      agent('assistant'),
      agent('offline-assistant', { available: false }),
      agent('expert', { agentKind: 'expert', invocationMode: 'inline', visibility: 'expert_center' }),
      agent('worker', { agentKind: 'worker', invocationMode: 'child', visibility: 'hidden' }),
    ]

    expect(chatSelectableAgents(records).map((item) => item.id)).toEqual(['assistant'])
  })

  test('keeps unavailable experts visible in the expert catalog and hides workers', () => {
    const records = [
      agent('assistant'),
      agent('offline-expert', {
        agentKind: 'expert',
        invocationMode: 'inline',
        visibility: 'expert_center',
        available: false,
      }),
      agent('worker', { agentKind: 'worker', invocationMode: 'child', visibility: 'hidden' }),
    ]

    expect(expertCatalogAgents(records).map((item) => item.id)).toEqual(['offline-expert'])
  })
})
