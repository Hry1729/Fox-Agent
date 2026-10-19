import { describe, expect, test } from 'bun:test'
import {
  agentBackendAccess,
  agentBackendLockNotice,
  agentBackendPickerDisabledReason,
  canPickAgentBackend,
  conversationAgentBackend,
  type AgentBackendStatus,
} from '../src/features/conversations/model/agent-backend'

function status(enabled: Array<'pi' | 'codex' | 'deepseek'>): AgentBackendStatus {
  return {
    switchEnv: 'FOX_EXTERNAL_BACKENDS',
    anyExternalEnabled: enabled.length > 0,
    unknownEntries: [],
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({
      backend,
      enabled: backend === 'fox' || enabled.includes(backend as 'pi'),
      wired: backend === 'fox',
      experimental: backend === 'deepseek',
      reason: backend === 'fox' ? null : 'reason',
    })),
  }
}

describe('agent backend presentation (Host wiring first batch)', () => {
  test('summaries without the field are old Fox conversations', () => {
    expect(conversationAgentBackend({})).toBe('fox')
    expect(conversationAgentBackend(null)).toBe('fox')
    expect(conversationAgentBackend({ agentBackend: 'codex' })).toBe('codex')
  })

  test('mirrors host session access and fails closed without status', () => {
    expect(agentBackendAccess('fox', null)).toBe('runnable')
    expect(agentBackendAccess('pi', null)).toBe('read_only_disabled')
    expect(agentBackendAccess('pi', status([]))).toBe('read_only_disabled')
    expect(agentBackendAccess('pi', status(['pi']))).toBe('read_only_not_wired')
    expect(agentBackendLockNotice('fox', 'runnable')).toBeNull()
    expect(agentBackendLockNotice('codex', 'read_only_disabled')).toContain('只能查看历史')
  })

  test('picker is disabled and explained while the switch is off', () => {
    expect(agentBackendPickerDisabledReason(status([]))).toContain('FOX_EXTERNAL_BACKENDS')
    expect(agentBackendPickerDisabledReason(status(['codex']))).toBeNull()
    expect(canPickAgentBackend('fox', null)).toBe(true)
    expect(canPickAgentBackend('codex', status([]))).toBe(false)
    expect(canPickAgentBackend('codex', status(['codex']))).toBe(true)
    expect(canPickAgentBackend('pi', status(['codex']))).toBe(false)
  })
})
