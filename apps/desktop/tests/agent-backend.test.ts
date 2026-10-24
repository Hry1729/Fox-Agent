import { describe, expect, test } from 'bun:test'
import {
  AGENT_BACKEND_REFUSED,
  agentBackendAccess,
  agentBackendCapabilityChips,
  agentBackendInputLockedReason,
  isAgentBackendRefusalCode,
  withoutRefusedSend,
  agentBackendLockNotice,
  agentBackendPickerDisabledReason,
  canPickAgentBackend,
  conversationAgentBackend,
  type AgentBackendCapabilities,
  type AgentBackendStatus,
} from '../src/features/conversations/model/agent-backend'

function status(enabled: Array<'pi' | 'codex' | 'deepseek'>): AgentBackendStatus {
  return {
    switchEnv: 'FOX_EXTERNAL_BACKENDS',
    anyExternalEnabled: enabled.length > 0,
    unknownEntries: [],
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({
      backend,
      enabled: backend === 'fox' || enabled.includes(backend),
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
    expect(agentBackendPickerDisabledReason(status(['pi']))).toBeNull()
    expect(agentBackendPickerDisabledReason(status([]))).not.toContain('Codex')
    expect(canPickAgentBackend('fox', null)).toBe(true)
    expect(canPickAgentBackend('pi', status([]))).toBe(false)
    expect(canPickAgentBackend('pi', status(['pi']))).toBe(true)
    expect(canPickAgentBackend('deepseek', status(['pi']))).toBe(false)
    // Codex is hidden: never pickable for a new conversation, even when enabled.
    expect(canPickAgentBackend('codex', status(['codex']))).toBe(false)
    // ...but an existing Codex conversation keeps its access from the switch.
    expect(agentBackendAccess('codex', { ...status(['codex']), options: status(['codex']).options.map((option) => ({ ...option, wired: true })) })).toBe('runnable')
    expect(isAgentBackendRefusalCode('agent_backend_hidden')).toBe(true)
  })
})

describe('refused sends leave no optimistic trace (fix 1 after Windows acceptance)', () => {
  test('only agent_backend_* codes are routing refusals', () => {
    expect(isAgentBackendRefusalCode('agent_backend_not_yet_wired')).toBe(true)
    expect(isAgentBackendRefusalCode('agent_backend_disabled')).toBe(true)
    expect(isAgentBackendRefusalCode('agent_backend_misrouted')).toBe(true)
    expect(isAgentBackendRefusalCode('agent_backend_invalid')).toBe(true)
    expect(isAgentBackendRefusalCode('storage.operation_failed')).toBe(false)
    expect(isAgentBackendRefusalCode('runtime.resolve_failed')).toBe(false)
    expect(isAgentBackendRefusalCode(undefined)).toBe(false)
    expect(AGENT_BACKEND_REFUSED).toBe('agent_backend_refused')
  })

  test('the pending message and pending run are removed, the previous run restored', () => {
    const previous = { id: 'run-1', status: 'completed' }
    const detail = {
      conversation: { id: 'c' },
      messages: [{ id: 'm-1' }, { id: 'pending-message-1' }],
      lastRun: { id: 'pending-run-1', status: 'queued' },
    }
    const cleaned = withoutRefusedSend(detail, 'pending-message-1', previous)
    expect(cleaned.messages.map((message) => message.id)).toEqual(['m-1'])
    expect(cleaned.lastRun).toBe(previous)
    expect(cleaned.conversation).toBe(detail.conversation)
    // A new conversation had no run before the send.
    expect(withoutRefusedSend({ messages: [{ id: 'p' }], lastRun: { id: 'pending-run-2' } }, 'p', null).lastRun).toBeNull()
    // A real (persisted) run is never replaced, and nothing to remove keeps identity.
    const settled = { messages: [{ id: 'm-1' }], lastRun: { id: 'run-9' } }
    expect(withoutRefusedSend(settled, null, null)).toBe(settled)
  })

  test('input is locked only for an external conversation that is read-only because its backend is off', () => {
    expect(agentBackendInputLockedReason('fox', null)).toBeNull()
    expect(agentBackendInputLockedReason('fox', status([]))).toBeNull()
    expect(agentBackendInputLockedReason('pi', status([]))).toContain('只能查看历史')
    expect(agentBackendInputLockedReason('codex', null)).toContain('Codex')
    // Switch on but runtime not wired: the send goes to the Host, which answers
    // agent_backend_not_yet_wired, and the UI shows that one refusal.
    expect(agentBackendInputLockedReason('pi', status(['pi']))).toBeNull()
  })
})

describe('capability chips (external backend wiring)', () => {
  function view(source: 'live' | 'planned'): AgentBackendCapabilities {
    const dim = (name: string) => ({ summary: `${name}-summary`, detail: `${name}-detail` })
    return {
      backend: 'pi',
      mode: 'sandboxed',
      source,
      approval: dim('approval'),
      sandbox: dim('sandbox'),
      cancel: dim('cancel'),
      observability: dim('observability'),
      warning: null,
    }
  }

  test('no view means no chips', () => {
    expect(agentBackendCapabilityChips(null)).toEqual([])
    expect(agentBackendCapabilityChips(undefined)).toEqual([])
  })

  test('four chips in a fixed order; live detail is unmarked', () => {
    const chips = agentBackendCapabilityChips(view('live'))
    expect(chips.map((chip) => chip.key)).toEqual(['approval', 'sandbox', 'cancel', 'observability'])
    expect(chips[0]).toEqual({ key: 'approval', label: 'approval-summary', title: 'approval-detail' })
  })

  test('a planned view marks every title as a pre-launch estimate', () => {
    const chips = agentBackendCapabilityChips(view('planned'))
    expect(chips.every((chip) => chip.title.endsWith('（启动前预估）'))).toBe(true)
    expect(chips[3].label).toBe('observability-summary')
  })
})
