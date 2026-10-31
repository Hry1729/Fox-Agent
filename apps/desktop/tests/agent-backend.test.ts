import { describe, expect, test } from 'bun:test'
import {
  AGENT_BACKEND_ERROR_CODES,
  AGENT_BACKEND_REFUSED,
  RETIRED_AGENT_BACKEND_IDS,
  SELECTABLE_AGENT_BACKEND_IDS,
  SUPPORTED_AGENT_BACKEND_IDS,
  agentBackendAccess,
  agentBackendCapabilityChips,
  agentBackendInputLockedReason,
  agentBackendLockNotice,
  agentBackendOptionRetired,
  agentBackendPickerDisabledReason,
  agentBackendRetiredIds,
  agentBackendSavedRetiredNotice,
  agentBackendSettingRows,
  agentBackendSettingValue,
  canPickAgentBackend,
  conversationAgentBackend,
  isAgentBackendRefusalCode,
  isAgentBackendRetired,
  newConversationBackend,
  DEFAULT_AGENT_BACKEND_PREFERENCES,
  agentBackendForcedOffNotice,
  withoutRefusedSend,
  type AgentBackendCapabilities,
  type AgentBackendPreferences,
  type AgentBackendStatus,
} from '../src/features/conversations/model/agent-backend'

/**
 * The status the current Host reports (single Pi mainline): exactly one
 * supported backend and the three retired ones, which stay listed so the UI can
 * explain them.
 */
function status(): AgentBackendStatus {
  return {
    switchEnv: 'FOX_EXTERNAL_BACKENDS',
    anyExternalEnabled: false,
    supportedBackends: ['fox'],
    retiredBackends: ['pi', 'codex', 'deepseek'],
    unknownEntries: [],
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({
      backend,
      enabled: backend === 'fox',
      wired: backend === 'fox',
      experimental: backend === 'deepseek',
      hidden: false,
      retired: backend !== 'fox',
      reason: backend === 'fox' ? null : `本版本只支持内置 Pi 驱动，此前的外部 Agent 路由（${backend}）已退休`,
    })),
  }
}

/**
 * An OLDER Host: no `supportedBackends` / `retiredBackends` and no `retired` on
 * an option, only `enabled` / `wired`. Its fallback must not resurrect a
 * retired driver: the product decision wins over a stale status.
 */
function oldStatus(enabled: Array<'pi' | 'codex' | 'deepseek'> = []): AgentBackendStatus {
  return {
    switchEnv: 'FOX_EXTERNAL_BACKENDS',
    anyExternalEnabled: enabled.length > 0,
    unknownEntries: [],
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({
      backend,
      enabled: backend === 'fox' || enabled.includes(backend),
      wired: backend === 'fox' || enabled.includes(backend),
      experimental: backend === 'deepseek',
      reason: backend === 'fox' ? null : 'reason',
    })),
  }
}

function preferences(over: Partial<AgentBackendPreferences> = {}): AgentBackendPreferences {
  return {
    ...DEFAULT_AGENT_BACKEND_PREFERENCES,
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({
      backend,
      enabled: backend === 'fox',
      experimental: backend === 'deepseek',
      retired: backend !== 'fox',
      reason: backend === 'fox' ? null : `外部 Agent 路由（${backend}）已退休`,
    })),
    ...over,
  }
}

describe('single Pi mainline: one supported driver (product decision 2026-10-27)', () => {
  test('the build mirrors the Host: one supported backend, three retired', () => {
    expect(SUPPORTED_AGENT_BACKEND_IDS).toEqual(['fox'])
    expect(RETIRED_AGENT_BACKEND_IDS).toEqual(['pi', 'codex', 'deepseek'])
    expect(SELECTABLE_AGENT_BACKEND_IDS).toEqual(['fox'])
    expect(isAgentBackendRetired('fox')).toBe(false)
    for (const backend of RETIRED_AGENT_BACKEND_IDS) {
      expect(isAgentBackendRetired(backend)).toBe(true)
      expect(isAgentBackendRetired(backend, status())).toBe(true)
    }
    expect(AGENT_BACKEND_ERROR_CODES.retired).toBe('agent_backend_retired')
  })

  test('summaries without the field are old Fox conversations', () => {
    expect(conversationAgentBackend({})).toBe('fox')
    expect(conversationAgentBackend(null)).toBe('fox')
    expect(conversationAgentBackend({ agentBackend: 'codex' })).toBe('codex')
  })

  test('an older Host is understood from enabled/wired without reviving a retired driver', () => {
    // Absent lists: the options say what cannot be used.
    expect(agentBackendRetiredIds(oldStatus())).toEqual(['pi', 'codex', 'deepseek'])
    expect(agentBackendRetiredIds(oldStatus(['pi']))).toEqual(['codex', 'deepseek'])
    expect(agentBackendOptionRetired({ backend: 'codex', enabled: false })).toBe(true)
    expect(agentBackendOptionRetired({ backend: 'codex', enabled: true })).toBe(false)
    expect(agentBackendOptionRetired({ backend: 'fox', enabled: false })).toBe(false)
    // No status at all: this build's own list.
    expect(agentBackendRetiredIds(null)).toEqual(['pi', 'codex', 'deepseek'])
    // Even an older Host that still enables pi cannot make it runnable or pickable.
    expect(agentBackendAccess('pi', oldStatus(['pi']))).toBe('read_only_retired')
    expect(canPickAgentBackend('pi', oldStatus(['pi']))).toBe(false)
  })
})

describe('a new conversation: the built-in Pi path only', () => {
  test('only the built-in route can be picked', () => {
    expect(canPickAgentBackend('fox', null)).toBe(true)
    expect(canPickAgentBackend('fox', status())).toBe(true)
    for (const backend of RETIRED_AGENT_BACKEND_IDS) {
      expect(canPickAgentBackend(backend, status())).toBe(false)
    }
    expect(agentBackendSettingRows(preferences()).map((row) => [row.value, row.disabled, row.reason])).toEqual([['fox', false, null]])
    expect(agentBackendSettingValue(preferences())).toBe('fox')
    expect(agentBackendSettingValue(preferences({ defaultBackend: 'codex', effectiveBackend: 'codex' }))).toBe('fox')
  })

  test('a saved retired preference is never applied to a new conversation', () => {
    expect(newConversationBackend(null)).toBe('fox')
    expect(newConversationBackend(preferences())).toBe('fox')
    for (const backend of RETIRED_AGENT_BACKEND_IDS) {
      const saved = preferences({ defaultBackend: backend, effectiveBackend: backend })
      expect(newConversationBackend(saved)).toBe('fox')
      expect(agentBackendSettingValue(saved)).toBe('fox')
    }
  })

  test('the picker explanation is always the retirement one, and names the retired drivers', () => {
    const reason = agentBackendPickerDisabledReason(status())
    expect(reason).toContain('退休')
    expect(reason).toContain('内置 Pi 驱动')
    expect(reason).toContain('Codex')
    expect(reason).toContain('DeepSeek')
    expect(agentBackendPickerDisabledReason(oldStatus()).length).toBeGreaterThan(0)
    expect(agentBackendPickerDisabledReason(null).length).toBeGreaterThan(0)
  })

  test('the settings statement and the saved-preference note', () => {
    expect(agentBackendForcedOffNotice(preferences())).toContain('退休')
    expect(agentBackendForcedOffNotice(preferences())).toContain('内置 Pi 驱动')
    expect(agentBackendForcedOffNotice(preferences({ forcedOff: false }))).toBeNull()
    expect(agentBackendSavedRetiredNotice(preferences())).toBeNull()
    const saved = agentBackendSavedRetiredNotice(preferences({ defaultBackend: 'codex', effectiveBackend: 'fox' }))
    expect(saved).toContain('Codex')
    expect(saved).toContain('退休')
    expect(saved).toContain('原绑定')
    expect(saved).toContain('只能查看历史')
  })
})

describe('an existing conversation of a retired driver', () => {
  test('reports read_only_retired and keeps its own name in the notice', () => {
    expect(agentBackendAccess('fox', null)).toBe('runnable')
    expect(agentBackendAccess('fox', status())).toBe('runnable')
    for (const backend of RETIRED_AGENT_BACKEND_IDS) {
      expect(agentBackendAccess(backend, status())).toBe('read_only_retired')
      // Without a status the mirror still fails closed the same way.
      expect(agentBackendAccess(backend, null)).toBe('read_only_retired')
      const notice = agentBackendLockNotice(backend, 'read_only_retired', 'FOX_EXTERNAL_BACKENDS')
      expect(notice).toContain('退休')
      expect(notice).toContain('只能查看历史')
      expect(notice).toContain('不会启动运行时')
    }
    expect(agentBackendLockNotice('fox', 'runnable')).toBeNull()
    // The original binding is named: a Codex / DeepSeek conversation is never
    // relabelled as the built-in Pi driver.
    const codex = agentBackendLockNotice('codex', 'read_only_retired')!
    expect(codex).toContain('Codex')
    expect(codex).not.toContain('Pi')
    const deepseek = agentBackendLockNotice('deepseek', 'read_only_retired')!
    expect(deepseek).toContain('DeepSeek')
    expect(deepseek).not.toContain('内置')
  })

  test('the composer lock reason uses the retirement wording', () => {
    expect(agentBackendInputLockedReason('fox', null)).toBeNull()
    expect(agentBackendInputLockedReason('fox', status())).toBeNull()
    for (const backend of RETIRED_AGENT_BACKEND_IDS) {
      const reason = agentBackendInputLockedReason(backend, status())!
      expect(reason).toContain('退休')
      expect(reason).toContain('只能查看历史')
    }
    expect(agentBackendInputLockedReason('codex', null)).toContain('Codex')
    expect(agentBackendInputLockedReason('pi', null)).not.toContain('内置')
  })
})

describe('refused sends leave no optimistic trace (fix 1 after Windows acceptance)', () => {
  test('only agent_backend_* codes are routing refusals', () => {
    expect(isAgentBackendRefusalCode('agent_backend_retired')).toBe(true)
    expect(isAgentBackendRefusalCode('agent_backend_not_yet_wired')).toBe(true)
    expect(isAgentBackendRefusalCode('agent_backend_disabled')).toBe(true)
    expect(isAgentBackendRefusalCode('agent_backend_misrouted')).toBe(true)
    expect(isAgentBackendRefusalCode('agent_backend_invalid')).toBe(true)
    // A retirement refusal is not a generic failure and must never be shown as one.
    expect(isAgentBackendRefusalCode('storage.operation_failed')).toBe(false)
    expect(isAgentBackendRefusalCode('runtime.resolve_failed')).toBe(false)
    expect(isAgentBackendRefusalCode('kernel_engine_retired')).toBe(false)
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
})

describe('capability chips (external backend wiring, kept for existing records)', () => {
  function view(source: 'live' | 'planned'): AgentBackendCapabilities {
    const dim = (name: string) => ({ summary: `${name}-summary`, detail: `${name}-detail` })
    return {
      backend: 'pi',
      mode: 'standard',
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
