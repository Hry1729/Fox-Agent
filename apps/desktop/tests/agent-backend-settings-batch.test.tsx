// Settings batch ①②④ after plan C1 (2026-10-31).
//
// A new conversation can use Fox (built-in PiNative) or one of the external
// Agents (Pi, Codex, DeepSeek / dsh): 设置 →「新会话使用的 Agent」is a select
// again and the composer has an Agent menu. `FOX_EXTERNAL_BACKENDS` is only an
// off switch. The retirement mechanism of the single Pi mainline (2026-10-27)
// is kept: a Host that declares a backend retired still gets a read-only
// conversation that shows its OWN binding, and nothing of it is loaded.
//
// ① No login probe / capability request / native permission list is made or
//   rendered for a retired backend (draft or locked).
// ② UI 1103: no login / configuration card above the composer, in normal or
//   developer mode (it used to leave an import receipt on the bar too).
// ④ The backend of a new conversation is the saved setting (default Fox).
import { afterEach, beforeEach, describe, expect, spyOn, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { act, cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { AgentBackendPanel } = await import('../src/features/chat/components/AgentBackendPanel')
const { AgentBackendBar } = await import('../src/features/chat/components/AgentBackendBar')
const { AgentBackendSettingsSection, NATIVE_PERMISSION_SETTING_BACKENDS } = await import('../src/features/settings/agent-backend-settings')
const { ComposerAgentPicker } = await import('../src/features/chat/composer/composer-agent-picker')
const {
  AGENT_BACKEND_ERROR_CODES,
  DEFAULT_AGENT_BACKEND_PREFERENCES,
  HIDDEN_AGENT_BACKENDS,
  RETIRED_AGENT_BACKEND_IDS,
  SELECTABLE_AGENT_BACKEND_IDS,
  SUPPORTED_AGENT_BACKEND_IDS,
  agentBackendSettingRows,
  agentBackendSettingValue,
  canPickAgentBackend,
  fullAccessSettingState,
  newConversationBackend,
} = await import('../src/features/conversations/model/agent-backend')
import type { AgentBackendCapabilities, AgentBackendId, AgentBackendPreferences, AgentBackendStatus } from '../src/features/conversations/model/agent-backend'
import type { NativePermissionRequest } from '../src/features/conversations/model/agent-native-permission'
import type { AgentBackendTarget, CredentialProbe } from '../src/features/conversations/model/agent-backend-onboarding'

// UI 1103: these tests pin the routing rules with every external Agent enabled in 设置 → Agent.
const ALL_AGENTS = ['pi', 'codex', 'deepseek'] as const
const { applyAgentEnablement, readEnabledExternalAgents, setExternalAgentEnabled } = await import('../src/features/conversations/model/external-agent-enablement')

afterEach(cleanup)

interface Deferred<T> { promise: Promise<T>; resolve: (value: T) => void; reject: (error: unknown) => void }
function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail })
  return { promise, resolve, reject }
}

/** A Host that declares the three external backends retired (mechanism kept). */
const status: AgentBackendStatus = {
  switchEnv: 'FOX_EXTERNAL_BACKENDS',
  anyExternalEnabled: false,
  supportedBackends: ['fox'],
  retiredBackends: ['pi', 'codex', 'deepseek'],
  options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({
    backend,
    enabled: backend === 'fox',
    wired: backend === 'fox',
    experimental: backend === 'deepseek',
    hidden: false,
    retired: backend !== 'fox',
    reason: backend === 'fox' ? null : `外部 Agent 路由（${backend}）已退休`,
  })),
  unknownEntries: [],
}

/** The status a C1 Host reports: four supported backends, none retired; `off` switched off. */
function c1Status(off: string[] = []): AgentBackendStatus {
  return {
    switchEnv: 'FOX_EXTERNAL_BACKENDS',
    anyExternalEnabled: off.length < 3,
    supportedBackends: ['fox', 'pi', 'codex', 'deepseek'],
    retiredBackends: [],
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({
      backend,
      enabled: !off.includes(backend),
      wired: true,
      experimental: backend === 'deepseek',
      hidden: false,
      retired: false,
      reason: off.includes(backend) ? '已被管理员关闭（FOX_EXTERNAL_BACKENDS）' : null,
    })),
    unknownEntries: [],
  }
}

/** C1 preferences: every backend usable unless switched off. */
function c1Preferences(over: Partial<AgentBackendPreferences> = {}, off: string[] = []): AgentBackendPreferences {
  return {
    ...DEFAULT_AGENT_BACKEND_PREFERENCES,
    retiredBackends: [],
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({
      backend,
      enabled: !off.includes(backend),
      experimental: backend === 'deepseek',
      retired: false,
      reason: off.includes(backend) ? '已被管理员关闭（FOX_EXTERNAL_BACKENDS）' : null,
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

function missing(backend: string): CredentialProbe {
  const files: Record<string, string[]> = { deepseek: ['.dsh/.env'], pi: ['.pi/agent/auth.json'], codex: ['.codex/auth.json'] }
  return {
    backend,
    state: 'missing_credentials',
    privateHome: `/fox/homes/p1/${backend}`,
    credentialsPresent: false,
    presentKeys: [],
    presentFiles: [],
    requiredKeys: backend === 'deepseek' ? ['DEEPSEEK_API_KEY'] : [],
    requiredFiles: files[backend] ?? [],
    templates: [],
  }
}

/** A fake Host where every probe / capability call is answered by the test. */
function fakeClient() {
  const probes: { target: AgentBackendTarget; reply: Deferred<CredentialProbe> }[] = []
  const caps: { backend: AgentBackendId; reply: Deferred<AgentBackendCapabilities> }[] = []
  const client = {
    agentBackendCapabilities: (backend: AgentBackendId) => { const reply = deferred<AgentBackendCapabilities>(); caps.push({ backend, reply }); return reply.promise },
    agentBackendPermissionStatus: async () => ({ fullAccessForbidden: false, forbiddenReason: null, engineeringSwitch: false }),
    agentBackendCredentials: (target: AgentBackendTarget) => { const reply = deferred<CredentialProbe>(); probes.push({ target, reply }); return reply.promise },
    agentBackendImportCandidates: async () => [{ id: 'file:.pi/agent/auth.json', kind: 'file' as const, name: 'auth.json', destination: '.pi/agent/auth.json' }],
    agentBackendImportCredentials: async () => ({ backend: 'pi', homes: 0, files: [], keys: [], skipped: [] }),
    agentBackendOpenPrivateHome: async () => '/fox/homes/p1',
    agentBackendRevokeFullAccess: async () => ({}),
    listenRuntimeEvents: async () => () => {},
  }
  return { client, probes, caps }
}

function Panel({ client, backend, project = 'p1', locked = null, conversationId = null, refusal = null, developerUi }: { client: any; backend: AgentBackendId; project?: string; locked?: AgentBackendId | null; conversationId?: string | null; refusal?: string | null; developerUi?: boolean }) {
  void project
  return <AgentBackendPanel client={client} status={status} lockedBackend={locked} conversationId={conversationId} draftBackend={backend} refusal={refusal} developerUi={developerUi} />
}

const text = () => document.body.textContent ?? ''

async function settle() {
  await act(async () => { await Promise.resolve(); await Promise.resolve() })
}

describe('① a new conversation never renders or loads a retired driver', () => {
  test('a draft naming a retired backend shows no card, no chips and asks the Host nothing', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="deepseek" />)
    await settle()
    expect(fake.probes).toHaveLength(0)
    expect(fake.caps).toHaveLength(0)
    expect(text()).toBe('')

    view.rerender(<Panel client={fake.client} backend="codex" />)
    await settle()
    expect(fake.probes).toHaveLength(0)
    expect(text()).toBe('')

    view.rerender(<Panel client={fake.client} backend="pi" />)
    await settle()
    expect(fake.probes).toHaveLength(0)
    expect(text()).toBe('')
  })

  test('an existing conversation of a retired backend keeps its binding and is read-only', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="fox" locked="codex" conversationId="c-old-codex" developerUi />)
    await settle()
    expect(text()).toContain('后端：Codex（已锁定）')
    expect(text()).toContain('退休')
    expect(text()).toContain('只能查看历史')
    expect(text()).toContain('不会启动运行时')
    // Its OWN binding, never relabelled as the built-in Pi driver.
    expect(text()).not.toContain('Pi')
    expect(document.querySelector('.fox-agent-backend-bar')?.getAttribute('data-access')).toBe('read_only_retired')
    // Nothing of the retired runtime is loaded: no login probe, no chips, no
    // native permission picker, no network panel.
    expect(fake.probes).toHaveLength(0)
    expect(fake.caps).toHaveLength(0)
    expect(screen.queryByLabelText('权限')).toBeNull()
    expect(screen.queryByText('隔离：Fox 不额外包装（Agent 原生）')).toBeNull()

    // 拒绝原因改走通知，栏上只保留只读说明，不再叠一条 alert。
    view.rerender(<Panel client={fake.client} backend="fox" locked="codex" conversationId="c-old-codex" refusal="Agent 后端「Codex」已在本版本退休" developerUi />)
    expect(screen.queryByRole('alert')).toBeNull()
    expect(text()).toContain('只能查看历史')
    expect(text()).not.toContain('Agent 后端「Codex」已在本版本退休')
  })

  test('a deepseek conversation shows DeepSeek, not the built-in driver', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="fox" locked="deepseek" conversationId="c-old-ds" developerUi />)
    await settle()
    expect(text()).toContain('后端：DeepSeek（已锁定）')
    expect(text()).toContain('此会话绑定已退休的 Agent 后端「DeepSeek」')
    expect(text()).not.toContain('内置')
    expect(fake.probes).toHaveLength(0)
    // Without developer mode: the read-only notice stays (the user needs it),
    // the locked-backend chip does not.
    view.rerender(<Panel client={fake.client} backend="fox" locked="deepseek" conversationId="c-old-ds" developerUi={false} />)
    expect(text()).toContain('此会话绑定已退休的 Agent 后端「DeepSeek」')
    expect(text()).not.toContain('后端：DeepSeek（已锁定）')
  })
})

describe('② UI 1103: no login or configuration card above the composer', () => {
  const LOGIN_CARD_TEXT = ['还没有登录', '填写 Anthropic 地址', '用已有登录', '从真实 HOME 导入', '私有 HOME', '导入回执', '选择 Pi 配置来源']
  for (const developerUi of [false, true]) {
    test(`a draft and an open conversation of an unconfigured external Agent show no card (developer mode ${developerUi ? 'on' : 'off'})`, async () => {
      const fake = fakeClient()
      const view = render(<AgentBackendPanel client={fake.client} status={c1Status()} lockedBackend={null} conversationId={null} draftBackend="deepseek" developerUi={developerUi} />)
      await settle()
      view.rerender(<AgentBackendPanel client={fake.client} status={c1Status()} lockedBackend="deepseek" conversationId="c-ds" draftBackend="fox" developerUi={developerUi} />)
      await settle()
      for (const phrase of LOGIN_CARD_TEXT) expect(text()).not.toContain(phrase)
      // The login probe is no longer asked for above the composer at all.
      expect(fake.probes).toHaveLength(0)
      expect(document.querySelector('.fox-agent-login-card, .fox-agent-model-card, .fox-agent-backend-receipt')).toBeNull()
      if (!developerUi) expect(text()).toBe('')
    })
  }
})

describe('④ 新会话使用的 Agent: a choice again (plan C1)', () => {
  // UI 1103: these pin the routing rules with every external Agent enabled in 设置 → Agent.
  beforeEach(() => applyAgentEnablement(ALL_AGENTS.map(agent => ({ agent, enabled: true }))))
  afterEach(() => applyAgentEnablement([]))
  test('the settings row is a select of Fox / Pi / Codex / DeepSeek; a switched-off one is greyed with its reason', async () => {
    const client = { agentBackendPreferences: async () => c1Preferences({}, ['codex']), saveAgentBackendPreferences: async () => c1Preferences({}, ['codex']), probeCodexResponses: async () => null }
    render(<AgentBackendSettingsSection client={client} />)
    const trigger = await screen.findByRole('combobox', { name: '新会话使用的 Agent' })
    await waitFor(() => expect(trigger.hasAttribute('disabled')).toBe(false))
    expect(trigger.textContent).toContain('Fox')
    expect(text()).not.toContain('新会话使用内置 Pi 驱动')
    // The switched-off backend is explained, not a retirement.
    expect(screen.getByLabelText('不可用的 Agent').textContent).toContain('Codex：已被管理员关闭（FOX_EXTERNAL_BACKENDS）')
    expect(text()).not.toContain('退休')
    // No Fox-wide full-access switch.
    expect(screen.queryByRole('switch', { name: '新外部会话使用完全访问' })).toBeNull()
    expect(text()).not.toContain('新外部会话使用完全访问')
  })

  test('a saved external choice is shown; a switched-off saved choice says Fox is used meanwhile', async () => {
    const saved = c1Preferences({ defaultBackend: 'codex', effectiveBackend: 'codex' })
    const client = { agentBackendPreferences: async () => saved, saveAgentBackendPreferences: async () => saved, probeCodexResponses: async () => null }
    const view = render(<AgentBackendSettingsSection client={client} />)
    await waitFor(() => expect(screen.getByRole('combobox', { name: '新会话使用的 Agent' }).textContent).toContain('Codex'))
    expect(text()).not.toContain('暂时使用')
    view.unmount()

    const off = c1Preferences({ defaultBackend: 'deepseek', effectiveBackend: 'fox' }, ['deepseek'])
    render(<AgentBackendSettingsSection client={{ agentBackendPreferences: async () => off, saveAgentBackendPreferences: async () => off, probeCodexResponses: async () => null }} />)
    await waitFor(() => expect(text()).toContain('已保存的选择当前不可用，新会话暂时使用 Fox。'))
    expect(text()).not.toContain('退休')
  })

  test('a Host-retired saved default no longer applies; old conversations keep their binding (mechanism kept)', async () => {
    const saved = preferences({ defaultBackend: 'codex', effectiveBackend: 'fox' })
    const client = { agentBackendPreferences: async () => saved, saveAgentBackendPreferences: async () => saved, probeCodexResponses: async () => null }
    render(<AgentBackendSettingsSection client={client} />)
    await waitFor(() => expect(text()).toContain('之前保存的「Codex」已退休'))
    expect(text()).toContain('不再用于新会话')
    expect(text()).toContain('仍按原绑定显示')
    expect(text()).toContain('只能查看历史')
    // The select shows what is effective (Fox), never the retired one.
    expect(screen.getByRole('combobox', { name: '新会话使用的 Agent' }).textContent).toContain('Fox')
  })

  test('per-Agent default permission rows are offered for the three external Agents', async () => {
    const calls: NativePermissionRequest[] = []
    const client = {
      agentBackendPreferences: async () => c1Preferences(),
      saveAgentBackendPreferences: async () => c1Preferences(),
      probeCodexResponses: async () => null,
      agentBackendNativePermissions: async (request: NativePermissionRequest) => {
        calls.push(request)
        return {
          catalog: { backend: request.backend!, options: [
            { id: 'ask', tier: 'ask' as const, label: '询问', description: 'd', native: 'n', fullAccess: false, disabledReason: null },
            { id: 'full', tier: 'full_access' as const, label: '完全访问', description: 'd', native: 'n', fullAccess: true, disabledReason: null },
          ], defaultMode: 'ask', switchTiming: 'live' as const, switchNote: 'note' },
          current: 'ask', fullAccess: false, fullAccessGranted: false, fullAccessForbidden: false, draft: true, fullAccessLockedReason: null,
        }
      },
      agentBackendNativePermissionSet: async () => { throw new Error('not picked in this test') },
    }
    render(<AgentBackendSettingsSection client={client} />)
    await waitFor(() => expect(screen.getByLabelText('Codex 默认权限')).toBeTruthy())
    expect(screen.getByLabelText('DeepSeek 默认权限')).toBeTruthy()
    expect(screen.getByLabelText('Pi 默认权限')).toBeTruthy()
    expect(calls.map((request) => request.backend).sort()).toEqual(['codex', 'deepseek', 'pi'])
    expect(NATIVE_PERMISSION_SETTING_BACKENDS.map((item) => item.backend)).toEqual(['codex', 'deepseek', 'pi'])
  })

  test('the model: every C1 backend can be chosen and saved; a retired or switched-off one cannot', () => {
    expect(SELECTABLE_AGENT_BACKEND_IDS).toEqual(['fox', 'pi', 'codex', 'deepseek'])
    expect(HIDDEN_AGENT_BACKENDS).toEqual([])
    expect(agentBackendSettingRows(c1Preferences(), ALL_AGENTS).map((row) => [row.value, row.disabled])).toEqual([['fox', false], ['pi', false], ['codex', false], ['deepseek', false]])
    for (const backend of ['pi', 'codex', 'deepseek'] as const) {
      expect(canPickAgentBackend(backend, c1Status())).toBe(true)
      expect(canPickAgentBackend(backend, c1Status([backend]))).toBe(false)
      expect(agentBackendSettingValue(c1Preferences({ defaultBackend: backend, effectiveBackend: backend }), ALL_AGENTS)).toBe(backend)
      expect(newConversationBackend(c1Preferences({ defaultBackend: backend, effectiveBackend: backend }), ALL_AGENTS)).toBe(backend)
      // Retirement mechanism kept: a Host-declared retirement can be neither chosen nor saved.
      expect(canPickAgentBackend(backend, status)).toBe(false)
      expect(agentBackendSettingValue(preferences({ defaultBackend: backend, effectiveBackend: backend }), ALL_AGENTS)).toBe('fox')
      expect(newConversationBackend(preferences({ defaultBackend: backend, effectiveBackend: backend }), ALL_AGENTS)).toBe('fox')
    }
    expect(AGENT_BACKEND_ERROR_CODES.retired).toBe('agent_backend_retired')
    expect(AGENT_BACKEND_ERROR_CODES.disabled).toBe('agent_backend_disabled')
    // The removed Fox-wide full-access switch stays unavailable under FORBIDDEN / forced off.
    expect(fullAccessSettingState(c1Preferences({ fullAccessForbidden: true })).disabled).toBe(true)
    expect(fullAccessSettingState(c1Preferences({ forcedOff: true })).disabled).toBe(true)
  })

  test('the composer offers an Agent menu with the four choices', async () => {
    const view = render(<ComposerAgentPicker value="fox" status={c1Status()} onSelect={() => {}} />)
    const trigger = view.getByRole('button', { name: '选择 Agent，当前 Fox' })
    expect(trigger.getAttribute('data-agent-backend')).toBe('fox')
    await settle()
  })

  test('the composer still opens a menu when only the built-in route is left (Host-retired drivers)', async () => {
    const view = render(<ComposerAgentPicker value="fox" status={status} onSelect={() => {}} />)
    expect(view.container.textContent).toBe('Fox')
    expect(view.getByRole('button', { name: '选择 Agent，当前 Fox' }).getAttribute('data-agent-backend')).toBe('fox')
    await settle()
  })

  test('the bar has no backend row for a Fox draft, and a Host-retired saved default cannot put its driver back', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="fox" />)
    expect(screen.queryByRole('combobox')).toBeNull()
    expect(text()).toBe('')
    const saved = preferences({ defaultBackend: 'deepseek', effectiveBackend: 'deepseek' })
    view.rerender(<Panel client={fake.client} backend={newConversationBackend(saved, ALL_AGENTS)} />)
    await settle()
    expect(text()).toBe('')
    expect(fake.probes).toHaveLength(0)
    expect(newConversationBackend(saved, ALL_AGENTS)).toBe('fox')
  })
})

describe('plan C1 (2026-10-31): the external Agents are back, the retirement mechanism stays', () => {
  test('every user-facing choice offers the four backends; nothing is retired by this build', () => {
    expect(RETIRED_AGENT_BACKEND_IDS).toEqual([])
    expect(SUPPORTED_AGENT_BACKEND_IDS).toEqual(['fox', 'pi', 'codex', 'deepseek'])
    expect(agentBackendSettingRows(c1Preferences(), ALL_AGENTS).map((row) => row.value)).toEqual(['fox', 'pi', 'codex', 'deepseek'])
    expect(canPickAgentBackend('codex', c1Status())).toBe(true)
    expect(canPickAgentBackend('pi', c1Status())).toBe(true)
    // A Host that retires them still wins.
    expect(canPickAgentBackend('codex', status)).toBe(false)
    expect(canPickAgentBackend('pi', status)).toBe(false)
  })

  test("Fox's own permission settings are untouched and stay on the page", () => {
    const page = readFileSync(new URL('../src/features/settings/settings-agent-page.tsx', import.meta.url), 'utf8')
    // 设置 → Agent still renders the backend section and the project-permission
    // controls (Fox's own settings), which this work package must not remove.
    expect(page).toContain('<AgentBackendSettingsSection')
    expect(page).toContain('项目权限')
    expect(page).toContain('新项目权限')
    expect(page).toContain('默认模型')
  })
})

describe('Agent configuration is managed through Host bindings', () => {
  afterEach(() => applyAgentEnablement([]))

  test('Agent defaults link to model settings without running a connection or credential probe', async () => {
    let opened = 0
    const client = {
      agentBackendPreferences: async () => c1Preferences(),
      saveAgentBackendPreferences: async () => c1Preferences(),
      probeCodexResponses: async () => { throw new Error('must not probe on page open') },
    }
    render(<AgentBackendSettingsSection client={client} onOpenModelSettings={() => { opened += 1 }} />)
    await waitFor(() => expect(screen.getByRole('combobox', { name: '新会话使用的 Agent' })).toBeTruthy())
    expect(screen.queryByRole('switch', { name: '启用 Codex' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: '前往配置' }))
    expect(opened).toBe(1)
  })

  test('enablement is saved by Host before it becomes visible and ignores legacy browser storage', async () => {
    const { desktopClient } = await import('../src/features/conversations/api/desktop-client')
    const pending = deferred<void>()
    const save = spyOn(desktopClient, 'agentEnableSet').mockReturnValue(pending.promise)
    const legacyKey = 'fox.preferences.enabledExternalAgents'
    window.localStorage.setItem(legacyKey, JSON.stringify(['pi']))
    try {
      applyAgentEnablement([])
      const changing = setExternalAgentEnabled('codex', true)
      expect(readEnabledExternalAgents()).toEqual([])
      expect(save).toHaveBeenCalledWith('codex', true)
      pending.resolve()
      await changing
      expect(readEnabledExternalAgents()).toEqual(['codex'])
      expect(JSON.parse(window.localStorage.getItem(legacyKey)!)).toEqual(['pi'])
    } finally { save.mockRestore(); window.localStorage.removeItem(legacyKey) }
  })

  test('the per-Agent default permission rows appear only for enabled Agents', async () => {
    const calls: string[] = []
    const client = {
      agentBackendPreferences: async () => c1Preferences(),
      saveAgentBackendPreferences: async () => c1Preferences(),
      probeCodexResponses: async () => null,
      agentBackendNativePermissions: async (request: NativePermissionRequest) => {
        calls.push(request.backend!)
        return {
          catalog: { backend: request.backend!, options: [{ id: 'ask', tier: 'ask' as const, label: '询问', description: 'd', native: 'n', fullAccess: false, disabledReason: null }], defaultMode: 'ask', switchTiming: 'live' as const, switchNote: 'note' },
          current: 'ask', fullAccess: false, fullAccessGranted: false, fullAccessForbidden: false, draft: true, fullAccessLockedReason: null,
        }
      },
      agentBackendNativePermissionSet: async () => { throw new Error('not picked in this test') },
    }
    applyAgentEnablement([{ agent: 'codex', enabled: true }])
    render(<AgentBackendSettingsSection client={client} />)
    await waitFor(() => expect(screen.getByLabelText('Codex 默认权限')).toBeTruthy())
    expect(screen.queryByLabelText('DeepSeek 默认权限')).toBeNull()
    expect(screen.queryByLabelText('Pi 默认权限')).toBeNull()
  })
})
