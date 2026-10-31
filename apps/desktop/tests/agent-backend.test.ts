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
 * The status a C1 Host reports (plan C1, 2026-10-31): the built-in route and
 * the three external Agents are supported, none is retired; `off` lists the
 * backends the administrator switched off (`FOX_EXTERNAL_BACKENDS`).
 */
function c1Status(off: Array<'pi' | 'codex' | 'deepseek'> = []): AgentBackendStatus {
  return {
    switchEnv: 'FOX_EXTERNAL_BACKENDS',
    anyExternalEnabled: off.length < 3,
    supportedBackends: ['fox', 'pi', 'codex', 'deepseek'],
    retiredBackends: [],
    unknownEntries: [],
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({
      backend,
      enabled: !(off as string[]).includes(backend),
      wired: true,
      experimental: backend === 'deepseek',
      hidden: false,
      retired: false,
      reason: (off as string[]).includes(backend) ? `已被管理员关闭（FOX_EXTERNAL_BACKENDS）` : null,
    })),
  }
}

/**
 * A Host that RETIRES the three external backends (the single Pi mainline of
 * 2026-10-27). No C1 Host does, but the retirement mechanism is kept for a
 * backend that is really taken out of service, so the mirror must still honour
 * a Host that declares one.
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

/** C1 preferences: every backend usable unless switched off. */
function c1Preferences(over: Partial<AgentBackendPreferences> = {}, off: Array<'pi' | 'codex' | 'deepseek'> = []): AgentBackendPreferences {
  return {
    ...DEFAULT_AGENT_BACKEND_PREFERENCES,
    retiredBackends: [],
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({
      backend,
      enabled: !(off as string[]).includes(backend),
      experimental: backend === 'deepseek',
      retired: false,
      reason: (off as string[]).includes(backend) ? '已被管理员关闭（FOX_EXTERNAL_BACKENDS）' : null,
    })),
    ...over,
  }
}

/** Preferences from a Host that retires the three external backends (mechanism kept). */
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

describe('plan C1: the built-in route and the three external Agents', () => {
  test('the build mirrors the Host: four supported backends, none retired, the mechanism kept', () => {
    expect(SUPPORTED_AGENT_BACKEND_IDS).toEqual(['fox', 'pi', 'codex', 'deepseek'])
    expect(RETIRED_AGENT_BACKEND_IDS).toEqual([])
    expect(SELECTABLE_AGENT_BACKEND_IDS).toEqual(['fox', 'pi', 'codex', 'deepseek'])
    expect(isAgentBackendRetired('fox')).toBe(false)
    for (const backend of ['pi', 'codex', 'deepseek'] as const) {
      // Nothing is retired by this build or by a C1 Host ...
      expect(isAgentBackendRetired(backend)).toBe(false)
      expect(isAgentBackendRetired(backend, c1Status())).toBe(false)
      // ... but a Host that declares a retirement is still honoured.
      expect(isAgentBackendRetired(backend, status())).toBe(true)
    }
    expect(agentBackendRetiredIds(c1Status())).toEqual([])
    expect(AGENT_BACKEND_ERROR_CODES.retired).toBe('agent_backend_retired')
  })

  test('summaries without the field are old Fox conversations', () => {
    expect(conversationAgentBackend({})).toBe('fox')
    expect(conversationAgentBackend(null)).toBe('fox')
    expect(conversationAgentBackend({ agentBackend: 'codex' })).toBe('codex')
  })

  test('an older Host is understood from enabled/wired; an enabled external Agent runs', () => {
    // Absent lists: the options say what cannot be used.
    expect(agentBackendRetiredIds(oldStatus())).toEqual(['pi', 'codex', 'deepseek'])
    expect(agentBackendRetiredIds(oldStatus(['pi']))).toEqual(['codex', 'deepseek'])
    expect(agentBackendOptionRetired({ backend: 'codex', enabled: false })).toBe(true)
    expect(agentBackendOptionRetired({ backend: 'codex', enabled: true })).toBe(false)
    expect(agentBackendOptionRetired({ backend: 'fox', enabled: false })).toBe(false)
    // No status at all: this build's own list, which is empty since C1.
    expect(agentBackendRetiredIds(null)).toEqual([])
    // An older Host that enables pi: pi runs and can be picked.
    expect(agentBackendAccess('pi', oldStatus(['pi']))).toBe('runnable')
    expect(canPickAgentBackend('pi', oldStatus(['pi']))).toBe(true)
    expect(canPickAgentBackend('codex', oldStatus(['pi']))).toBe(false)
  })
})

describe('a new conversation: Fox or an external Agent', () => {
  test('every enabled backend can be picked; a switched-off one is greyed with its reason', () => {
    expect(canPickAgentBackend('fox', null)).toBe(true)
    for (const backend of ['fox', 'pi', 'codex', 'deepseek'] as const) {
      expect(canPickAgentBackend(backend, c1Status())).toBe(true)
    }
    // Unknown status: only the built-in route (fails closed).
    expect(canPickAgentBackend('pi', null)).toBe(false)
    // The administrator switched codex off: it stays listed, greyed.
    expect(canPickAgentBackend('codex', c1Status(['codex']))).toBe(false)
    expect(agentBackendSettingRows(c1Preferences()).map((row) => [row.value, row.disabled, row.reason])).toEqual([
      ['fox', false, null], ['pi', false, null], ['codex', false, null], ['deepseek', false, null],
    ])
    const off = agentBackendSettingRows(c1Preferences({}, ['codex']))
    expect(off.map((row) => [row.value, row.disabled])).toEqual([['fox', false], ['pi', false], ['codex', true], ['deepseek', false]])
    expect(off[2].reason).toContain('管理员关闭')
    expect(off[3].label).toBe('DeepSeek（实验）')
    // The saved choice is shown and used.
    expect(agentBackendSettingValue(c1Preferences())).toBe('fox')
    expect(agentBackendSettingValue(c1Preferences({ defaultBackend: 'codex', effectiveBackend: 'codex' }))).toBe('codex')
    // A retired backend (Host-declared) can be neither picked nor shown as the saved choice.
    for (const backend of ['pi', 'codex', 'deepseek'] as const) {
      expect(canPickAgentBackend(backend, status())).toBe(false)
    }
    expect(agentBackendSettingValue(preferences({ defaultBackend: 'codex', effectiveBackend: 'codex' }))).toBe('fox')
  })

  test('a saved choice is applied to a new conversation unless it is retired or switched off', () => {
    expect(newConversationBackend(null)).toBe('fox')
    expect(newConversationBackend(c1Preferences())).toBe('fox')
    for (const backend of ['pi', 'codex', 'deepseek'] as const) {
      expect(newConversationBackend(c1Preferences({ defaultBackend: backend, effectiveBackend: backend }))).toBe(backend)
      // The Host answers Fox as effective when the saved one is switched off.
      expect(newConversationBackend(c1Preferences({ defaultBackend: backend, effectiveBackend: 'fox' }, [backend]))).toBe('fox')
      // A Host-declared retirement is never applied.
      const saved = preferences({ defaultBackend: backend, effectiveBackend: backend })
      expect(newConversationBackend(saved)).toBe('fox')
      expect(agentBackendSettingValue(saved)).toBe('fox')
    }
  })

  test('the picker explanation names retired drivers only when a Host declares them', () => {
    const reason = agentBackendPickerDisabledReason(status())
    expect(reason).toContain('退休')
    expect(reason).toContain('内置 Pi 驱动')
    expect(reason).toContain('Codex')
    expect(reason).toContain('DeepSeek')
    const c1 = agentBackendPickerDisabledReason(c1Status(['pi', 'codex', 'deepseek']))
    expect(c1).not.toContain('退休')
    expect(c1).toContain('管理员关闭')
    expect(c1).toContain('FOX_EXTERNAL_BACKENDS')
    expect(agentBackendPickerDisabledReason(oldStatus()).length).toBeGreaterThan(0)
    expect(agentBackendPickerDisabledReason(null)).not.toContain('退休')
  })

  test('the forced-off statement and the saved-preference note', () => {
    // FOX_EXTERNAL_BACKENDS is only an off switch: every external Agent off.
    const forced = agentBackendForcedOffNotice(c1Preferences({ forcedOff: true }, ['pi', 'codex', 'deepseek']))
    expect(forced).toContain('管理员关闭')
    expect(forced).toContain('FOX_EXTERNAL_BACKENDS')
    expect(forced).toContain('内置 Pi 驱动')
    expect(agentBackendForcedOffNotice(c1Preferences())).toBeNull()
    expect(agentBackendForcedOffNotice(preferences({ forcedOff: false }))).toBeNull()
    // No retirement note in a C1 Host, whatever was saved.
    expect(agentBackendSavedRetiredNotice(c1Preferences({ defaultBackend: 'codex', effectiveBackend: 'codex' }))).toBeNull()
    expect(agentBackendSavedRetiredNotice(preferences())).toBeNull()
    // A Host that retires the saved one still gets the note.
    const saved = agentBackendSavedRetiredNotice(preferences({ defaultBackend: 'codex', effectiveBackend: 'fox' }))
    expect(saved).toContain('Codex')
    expect(saved).toContain('退休')
    expect(saved).toContain('原绑定')
    expect(saved).toContain('只能查看历史')
  })
})

describe('an existing conversation of an external Agent (C1)', () => {
  test('runnable when enabled; read-only with the switched-off wording when off; fails closed without status', () => {
    for (const backend of ['pi', 'codex', 'deepseek'] as const) {
      expect(agentBackendAccess(backend, c1Status())).toBe('runnable')
      expect(agentBackendInputLockedReason(backend, c1Status())).toBeNull()
      expect(agentBackendAccess(backend, c1Status([backend]))).toBe('read_only_disabled')
      expect(agentBackendAccess(backend, null)).toBe('read_only_disabled')
      const reason = agentBackendInputLockedReason(backend, c1Status([backend]))!
      expect(reason).toContain('未启用')
      expect(reason).not.toContain('退休')
      const notice = agentBackendLockNotice(backend, 'read_only_disabled', 'FOX_EXTERNAL_BACKENDS')!
      expect(notice).toContain('FOX_EXTERNAL_BACKENDS')
      expect(notice).toContain('只能查看历史')
    }
  })
})

describe('an existing conversation of a Host-retired driver (mechanism kept)', () => {
  test('reports read_only_retired and keeps its own name in the notice', () => {
    expect(agentBackendAccess('fox', null)).toBe('runnable')
    expect(agentBackendAccess('fox', status())).toBe('runnable')
    for (const backend of ['pi', 'codex', 'deepseek'] as const) {
      expect(agentBackendAccess(backend, status())).toBe('read_only_retired')
      // Without a status the mirror still fails closed (as switched off).
      expect(agentBackendAccess(backend, null)).toBe('read_only_disabled')
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
    for (const backend of ['pi', 'codex', 'deepseek'] as const) {
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

  test('three chips in a fixed order (the retired sandbox chip is gone); live detail is unmarked', () => {
    const chips = agentBackendCapabilityChips(view('live'))
    expect(chips.map((chip) => chip.key)).toEqual(['approval', 'cancel', 'observability'])
    expect(chips[0]).toEqual({ key: 'approval', label: 'approval-summary', title: 'approval-detail' })
  })

  test('a planned view marks every title as a pre-launch estimate', () => {
    const chips = agentBackendCapabilityChips(view('planned'))
    expect(chips.every((chip) => chip.title.endsWith('（启动前预估）'))).toBe(true)
    expect(chips[2].label).toBe('observability-summary')
  })
})
