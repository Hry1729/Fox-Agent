// Per-agent native permission list: picker rows in each Agent's own names,
// greyed options with reasons, dsh's「下一轮重启生效」, full-access
// confirmation in a new conversation, revoke-only mid-session, and the
// 设置 defaults (sandboxed options only, saved with `defaultOnly`).
import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { AgentNativePermissionPicker } = await import('../src/features/chat/components/AgentNativePermissionPicker')
const { AgentBackendSettingsSection } = await import('../src/features/settings/agent-backend-settings')
const { AgentBackendPanel } = await import('../src/features/chat/components/AgentBackendPanel')
const { DEFAULT_AGENT_BACKEND_PREFERENCES } = await import('../src/features/conversations/model/agent-backend')
const {
  NEXT_TURN_RESTART_BADGE,
  nativeAppliedNotice,
  nativeBackendName,
  nativePermissionChange,
  nativePermissionRows,
  nativeSwitchBadge,
} = await import('../src/features/conversations/model/agent-native-permission')
import type { NativePermissionCatalog, NativePermissionRequest, NativePermissionView } from '../src/features/conversations/model/agent-native-permission'
import type { AgentBackendCapabilities, AgentBackendStatus } from '../src/features/conversations/model/agent-backend'

afterEach(cleanup)

const AUTO_REVIEW_REASON = '「帮我批准」由 Codex 调用 OpenAI 的 codex-auto-review 模型自动审查；当前供应商不是 OpenAI，会被拒绝'

const codexCatalog = (autoReviewOff = true): NativePermissionCatalog => ({
  backend: 'codex',
  options: [
    { id: 'workspace-write', label: '请求审批', description: 'd', native: 'workspace-write', fullAccess: false, disabledReason: null },
    { id: 'agent', label: '帮我批准', description: 'd', native: 'agent', fullAccess: false, disabledReason: autoReviewOff ? AUTO_REVIEW_REASON : null },
    { id: 'agent-full-access', label: '完全访问权限', description: 'd', native: 'agent-full-access', fullAccess: true, disabledReason: null },
  ],
  defaultMode: 'workspace-write',
  switchTiming: 'live',
  switchNote: '会话中切换立即生效（session/set_mode）',
})

const dshCatalog: NativePermissionCatalog = {
  backend: 'deepseek',
  options: [
    { id: 'read-only', label: '只读', description: 'd', native: 'n', fullAccess: false, disabledReason: null },
    { id: 'workspace-write', label: '工作区可写（询问）', description: 'd', native: 'n', fullAccess: false, disabledReason: null },
    { id: 'workspace-write-never', label: '工作区可写（不询问）', description: 'd', native: 'n', fullAccess: false, disabledReason: null },
    { id: 'danger-full-access', label: '完全访问', description: 'd', native: 'n', fullAccess: true, disabledReason: null },
  ],
  defaultMode: 'workspace-write-never',
  switchTiming: 'next_turn_restart',
  switchNote: 'DeepSeek 不支持会话中途切换：下一轮开始时重启 dsh 生效',
}

const piCatalog: NativePermissionCatalog = {
  backend: 'pi',
  options: [
    { id: 'read_only', label: '只读', description: 'd', native: 'n', fullAccess: false, disabledReason: null },
    { id: 'ask', label: '询问', description: 'd', native: 'n', fullAccess: false, disabledReason: null },
    { id: 'allow', label: '自动', description: 'd', native: 'n', fullAccess: false, disabledReason: null },
    { id: 'full_access', label: '完全访问', description: 'd', native: 'n', fullAccess: true, disabledReason: null },
  ],
  defaultMode: 'ask',
  switchTiming: 'host_answered',
  switchNote: '由 Fox 作答',
}

function view(catalog: NativePermissionCatalog, patch: Partial<NativePermissionView> = {}): NativePermissionView {
  return { catalog, current: catalog.defaultMode, fullAccess: false, fullAccessGranted: false, fullAccessForbidden: false, draft: false, fullAccessLockedReason: '完全访问只能在新建会话时选择（会话中只能撤销）', ...patch }
}


// The settings dropdowns are the shared Radix Select: open with the keyboard,
// read the listed options (value + disabled), pick by clicking an option.
async function openSettingsSelect(trigger: HTMLElement) {
  fireEvent.keyDown(trigger, { key: 'Enter' })
  return await waitFor(() => {
    const items = screen.getAllByRole('option')
    expect(items.length).toBeGreaterThan(0)
    return items.map((item) => ({ item, value: item.getAttribute('data-value') ?? '', label: item.textContent ?? '', disabled: item.getAttribute('aria-disabled') === 'true' || item.hasAttribute('data-disabled') }))
  })
}
async function closeSettingsSelect() {
  fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' })
  await waitFor(() => expect(screen.queryAllByRole('option')).toHaveLength(0))
}
async function pickSettingsSelect(trigger: HTMLElement, value: string) {
  const options = await openSettingsSelect(trigger)
  const option = options.find((entry) => entry.value === value)
  expect(option).toBeTruthy()
  fireEvent.click(option!.item)
}

describe('model', () => {
  test('Codex rows use the native names and keep the auto-review reason', () => {
    const rows = nativePermissionRows(view(codexCatalog()))
    expect(rows.map((row) => row.label)).toEqual(['请求审批', '帮我批准', '完全访问权限'])
    expect(rows[1]).toMatchObject({ disabled: true, reason: AUTO_REVIEW_REASON })
    // The Agent's native full access is selectable in place; Fox no longer
    // adds or removes an outer SRT layer around it.
    expect(rows[2].disabled).toBe(false)
    expect(rows[2].reason).toBeNull()
    // A new conversation may pick it.
    expect(nativePermissionRows(view(codexCatalog(), { draft: true }))[2].disabled).toBe(false)
    // 设置 lists sandboxed options only.
    expect(nativePermissionRows(view(dshCatalog, { draft: true }), { sandboxedOnly: true }).map((row) => row.value)).not.toContain('danger-full-access')
  })

  test('changes: confirm full access in a draft, refuse mid-session, confirm revoke', () => {
    expect(nativePermissionChange(view(codexCatalog(), { draft: true }), 'agent-full-access').kind).toBe('confirm_full_access')
    expect(nativePermissionChange(view(codexCatalog()), 'agent-full-access').kind).toBe('confirm_full_access')
    expect(nativePermissionChange(view(codexCatalog()), 'agent').kind).toBe('refused')
    const full = view(codexCatalog(false), { current: 'agent-full-access', fullAccess: true, fullAccessGranted: true })
    expect(nativePermissionChange(full, 'workspace-write').kind).toBe('confirm_revoke')
    const engineering = { ...full, fullAccessGranted: false }
    expect(nativePermissionChange(engineering, 'workspace-write').kind).toBe('confirm_revoke')
    expect(nativePermissionChange(view(dshCatalog), 'read-only').kind).toBe('apply')
    expect(nativePermissionChange(view(dshCatalog), 'workspace-write-never').kind).toBe('none')
  })

  test('dsh is marked next-turn only mid-session; applied notices', () => {
    expect(nativeSwitchBadge(view(dshCatalog))).toBe(NEXT_TURN_RESTART_BADGE)
    expect(nativeSwitchBadge(view(dshCatalog, { draft: true }))).toBeNull()
    expect(nativeSwitchBadge(view(codexCatalog()))).toBeNull()
    expect(nativeAppliedNotice(view(dshCatalog, { applied: 'next_turn' }))).toContain('下一轮')
    expect(nativeAppliedNotice(view(codexCatalog(), { applied: 'applied' }))).toContain('立即生效')
    expect(nativeAppliedNotice(view(codexCatalog(), { applied: 'not_running' }))).toBeNull()
  })

  test('13.19 fix ①: the next-turn notice names the conversation\'s own Agent, never a hard-coded dsh', () => {
    // pi revoking full access (the Host answers next_turn: sandboxed relaunch).
    expect(nativeAppliedNotice(view({ ...piCatalog, displayName: 'pi' }, { applied: 'next_turn' }))).toBe('已保存，下一轮开始时重启 pi 生效')
    expect(nativeAppliedNotice(view({ ...codexCatalog(false), displayName: 'Codex' }, { applied: 'next_turn' }))).toBe('已保存，下一轮开始时重启 Codex 生效')
    expect(nativeAppliedNotice(view({ ...dshCatalog, displayName: 'DeepSeek' }, { applied: 'next_turn' }))).toBe('已保存，下一轮开始时重启 DeepSeek 生效')
    // An older Host without displayName: the backend bar's label.
    expect(nativeBackendName(view(piCatalog))).toBe('pi')
    expect(nativeBackendName(view(dshCatalog))).toBe('DeepSeek')
    expect(nativeBackendName(view(codexCatalog()))).toBe('Codex')
    for (const catalog of [piCatalog, codexCatalog(), dshCatalog]) {
      expect(nativeAppliedNotice(view(catalog, { applied: 'next_turn' }))).not.toContain('dsh')
    }
  })
})

function fakeClient(initial: NativePermissionView, onSet?: (request: NativePermissionRequest & { mode: string }) => NativePermissionView) {
  const calls: Array<NativePermissionRequest & { mode: string }> = []
  let current = initial
  return {
    calls,
    client: {
      agentBackendNativePermissions: async (_request: NativePermissionRequest) => current,
      agentBackendNativePermissionSet: async (request: NativePermissionRequest & { mode: string }) => {
        calls.push(request)
        current = onSet ? onSet(request) : { ...current, current: request.mode }
        return current
      },
    },
  }
}

describe('picker', () => {
  test('Codex mid-session: native names, greyed「帮我批准」with its reason, live switch', async () => {
    const { client, calls } = fakeClient(view(codexCatalog()), (request) => ({ ...view(codexCatalog()), current: request.mode, applied: 'applied' }))
    render(<AgentNativePermissionPicker client={client} conversationId="c1" backend="codex" />)
    const select = await screen.findByLabelText('权限') as HTMLSelectElement
    expect(Array.from(select.options).map((option) => option.textContent)).toEqual(['请求审批', '帮我批准（不可用）', '完全访问权限'])
    expect(screen.getByText(`帮我批准：${AUTO_REVIEW_REASON}`)).toBeTruthy()
    // The native full-access row is not locked by Fox.
    expect(document.querySelector('[data-disabled-option="agent-full-access"]')).toBeNull()
    expect(Array.from(select.options).map((option) => option.disabled)).toEqual([false, true, false])
    expect(screen.queryByText(NEXT_TURN_RESTART_BADGE)).toBeNull()
    // Picking it asks for the native risk confirmation and does not reach
    // the Host until confirmed.
    fireEvent.change(select, { target: { value: 'agent-full-access' } })
    expect(calls).toHaveLength(0)
    expect(screen.getByRole('alertdialog', { name: '确认完全访问' })).toBeTruthy()
  })

  test('dsh mid-session shows「下一轮重启生效」and applies at once (no confirmation)', async () => {
    const { client, calls } = fakeClient(view(dshCatalog), (request) => ({ ...view(dshCatalog), current: request.mode, applied: 'next_turn' }))
    render(<AgentNativePermissionPicker client={client} conversationId="c2" backend="deepseek" />)
    const select = await screen.findByLabelText('权限') as HTMLSelectElement
    expect(select.value).toBe('workspace-write-never')
    expect(screen.getByText(NEXT_TURN_RESTART_BADGE)).toBeTruthy()
    fireEvent.change(select, { target: { value: 'read-only' } })
    await waitFor(() => expect(calls).toEqual([{ conversationId: 'c2', mode: 'read-only' }]))
    expect(await screen.findByText('已保存，下一轮开始时重启 DeepSeek 生效')).toBeTruthy()
  })

  test('new conversation: full access needs the risk confirmation', async () => {
    const { client, calls } = fakeClient(view(codexCatalog(false), { draft: true, fullAccessLockedReason: null }))
    render(<AgentNativePermissionPicker client={client} conversationId={null} backend="codex" />)
    const select = await screen.findByLabelText('权限') as HTMLSelectElement
    fireEvent.change(select, { target: { value: 'agent-full-access' } })
    expect(calls).toHaveLength(0)
    // A draft has no full-access lock: no inline lock note.
    expect(document.querySelector('[data-disabled-option="agent-full-access"]')).toBeNull()
    const dialog = screen.getByRole('alertdialog', { name: '确认完全访问' })
    expect(dialog.textContent).toContain('Fox 不额外包装')
    fireEvent.click(screen.getByText('取消'))
    expect(screen.queryByRole('alertdialog')).toBeNull()
    fireEvent.change(select, { target: { value: 'agent-full-access' } })
    fireEvent.click(screen.getByText('确认使用完全访问'))
    await waitFor(() => expect(calls).toEqual([{ backend: 'codex', mode: 'agent-full-access' }]))
  })

  test('mid-session full access: leaving it goes through the revoke confirmation', async () => {
    const full = view(codexCatalog(false), { current: 'agent-full-access', fullAccess: true, fullAccessGranted: true, fullAccessLockedReason: null })
    const { client, calls } = fakeClient(full, (request) => ({ ...view(codexCatalog(false)), current: request.mode }))
    render(<AgentNativePermissionPicker client={client} conversationId="c3" backend="codex" />)
    const select = await screen.findByLabelText('权限') as HTMLSelectElement
    expect(select.value).toBe('agent-full-access')
    fireEvent.change(select, { target: { value: 'workspace-write' } })
    expect(screen.getByRole('alertdialog', { name: '撤销完全访问' })).toBeTruthy()
    fireEvent.click(screen.getByText('切回受限权限'))
    await waitFor(() => expect(calls).toEqual([{ conversationId: 'c3', mode: 'workspace-write' }]))
    await waitFor(() => expect((screen.getByLabelText('权限') as HTMLSelectElement).value).toBe('workspace-write'))
  })

  test('a Host refusal is shown, not swallowed', async () => {
    const { client } = fakeClient(view(dshCatalog), () => { throw new Error('完全访问只能在新建会话时选择；会话中只能撤销，不能开启') })
    render(<AgentNativePermissionPicker client={client} conversationId="c4" backend="deepseek" />)
    fireEvent.change(await screen.findByLabelText('权限'), { target: { value: 'read-only' } })
    expect((await screen.findByRole('alert')).textContent).toContain('不能开启')
  })
})

describe('native permission switches refresh the capability chips', () => {
  const status: AgentBackendStatus = {
    switchEnv: 'FOX_EXTERNAL_BACKENDS',
    anyExternalEnabled: true,
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({ backend, enabled: true, wired: true, experimental: false, reason: null })),
    unknownEntries: [],
  }
  const caps = (full: boolean): AgentBackendCapabilities => ({
    backend: 'pi',
    mode: 'full_access',
    source: 'live',
    approval: { summary: '审批：Fox 拦截', detail: '' },
    sandbox: { summary: full ? '沙箱：Fox 外层无；原生文件无、网络无' : '沙箱：Fox 外层无；原生文件强制、网络强制', detail: '' },
    cancel: { summary: '取消：支持', detail: '' },
    observability: { summary: '观测：完整', detail: '' },
    warning: null,
  })

  test('pi conversation: switching back to a restricted native mode refreshes the chips', async () => {
    let full = true
    let capabilityCalls = 0
    const client = {
      agentBackendCapabilities: async () => { capabilityCalls += 1; return caps(full) },
      agentBackendPermissionStatus: async () => ({ fullAccessForbidden: false, forbiddenReason: null, engineeringSwitch: false, granted: false, mode: 'full_access' as const, locked: true }),
      agentBackendCredentials: async () => ({ backend: 'pi', mode: 'sandboxed', state: 'ready', privateHome: '/h', credentialsPresent: true, loopbackBlocked: false, presentKeys: [], presentFiles: [], requiredKeys: [], requiredFiles: [], templates: [] }),
      agentBackendImportCandidates: async () => [],
      agentBackendImportCredentials: async () => ({ copied: [], skipped: [], failed: [], projects: 1 }),
      agentBackendOpenPrivateHome: async () => '/h',
      listenRuntimeEvents: async () => () => {},
      agentBackendNativePermissions: async () => view({ ...piCatalog, displayName: 'pi' }, { current: 'full_access', fullAccess: true }),
      agentBackendNativePermissionSet: async (request: NativePermissionRequest & { mode: string }) => {
        full = false
        return { ...view({ ...piCatalog, displayName: 'pi' }, { current: request.mode }), applied: 'next_turn' }
      },
    }
    render(<AgentBackendPanel client={client as any} status={status} lockedBackend="pi" conversationId="c-pi" draftBackend="fox" />)
    expect(await screen.findByText('沙箱：Fox 外层无；原生文件无、网络无')).toBeTruthy()
    const before = capabilityCalls
    fireEvent.change(await screen.findByLabelText('权限'), { target: { value: 'ask' } })
    fireEvent.click(screen.getByText('切回受限权限'))
    expect(await screen.findByText('已保存，下一轮开始时重启 pi 生效')).toBeTruthy()
    await waitFor(() => expect(screen.getByText('沙箱：Fox 外层无；原生文件强制、网络强制')).toBeTruthy())
    expect(capabilityCalls).toBeGreaterThan(before)
  })
})

describe('settings', () => {
  test('per-Agent defaults list sandboxed options only and save with defaultOnly', async () => {
    const calls: Array<NativePermissionRequest & { mode: string }> = []
    const catalogs: Record<string, NativePermissionCatalog> = {
      codex: codexCatalog(),
      deepseek: dshCatalog,
      pi: { backend: 'pi', options: [
        { id: 'read_only', label: '只读', description: 'd', native: 'n', fullAccess: false, disabledReason: null },
        { id: 'ask', label: '询问', description: 'd', native: 'n', fullAccess: false, disabledReason: null },
        { id: 'allow', label: '自动', description: 'd', native: 'n', fullAccess: false, disabledReason: null },
        { id: 'full_access', label: '完全访问', description: 'd', native: 'n', fullAccess: true, disabledReason: null },
      ], defaultMode: 'ask', switchTiming: 'host_answered', switchNote: '由 Fox 作答' },
    }
    const client = {
      agentBackendPreferences: async () => DEFAULT_AGENT_BACKEND_PREFERENCES,
      saveAgentBackendPreferences: async () => DEFAULT_AGENT_BACKEND_PREFERENCES,
      probeCodexResponses: async () => null,
      agentBackendNativePermissions: async (request: NativePermissionRequest) => view(catalogs[request.backend!], { draft: true, sandboxedMode: request.backend === 'deepseek' ? 'read-only' : undefined, current: request.backend === 'deepseek' ? 'danger-full-access' : catalogs[request.backend!].defaultMode }),
      agentBackendNativePermissionSet: async (request: NativePermissionRequest & { mode: string }) => {
        calls.push(request)
        return view(catalogs[request.backend!], { draft: true, current: request.mode })
      },
    }
    render(<AgentBackendSettingsSection client={client} />)
    const dsh = await screen.findByRole('combobox', { name: 'DeepSeek 默认权限' })
    // Full access from the switch: the remembered sandboxed choice is shown.
    expect(dsh.textContent).toBe('只读')
    const dshOptions = await openSettingsSelect(dsh)
    expect(dshOptions.find((option) => option.item.getAttribute('aria-selected') === 'true')?.value).toBe('read-only')
    expect(dshOptions.map((option) => option.value)).not.toContain('danger-full-access')
    await closeSettingsSelect()
    const pi = await screen.findByRole('combobox', { name: 'pi 默认权限' })
    await pickSettingsSelect(pi, 'allow')
    await waitFor(() => expect(calls).toEqual([{ backend: 'pi', mode: 'allow', defaultOnly: true }]))
    // Codex is selectable again: its default-permission row is present.
    expect(screen.getByLabelText('Codex 默认权限')).toBeTruthy()
  })
})
