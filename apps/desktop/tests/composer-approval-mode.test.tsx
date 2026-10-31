// Composer batch (2026-10-31):
// * the debugging details above the composer (locked-backend chip,
//   capability chips, configuration source / import fold, local preview) are
//   shown in developer mode only; what the user must act on stays;
// * ONE approval selector, in the composer: each backend's own modes from
//   the Host (full names in the menu, two characters on the button), greyed
//   options with their reason, the「下一轮生效」note inside the button;
// * a new conversation carries the previous choice's tier when its Agent is
//   switched; the Host maps it and never selects full access.
import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { act, cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { AgentBackendPanel } = await import('../src/features/chat/components/AgentBackendPanel')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { FoxApprovalControl, NativeApprovalControl } = await import('../src/features/chat/composer/composer-approval-control')
const { DEVELOPER_UI_KEY, persistDeveloperUi, readDeveloperUi } = await import('../src/features/chat/developer-ui')
const { agentBackendCapabilityChips } = await import('../src/features/conversations/model/agent-backend')
const { catalogRows, draftCarryTier, nativePendingNote, PENDING_NEXT_TURN_NOTE } = await import('../src/features/conversations/model/agent-native-permission')
import type { AgentBackendCapabilities, AgentBackendStatus } from '../src/features/conversations/model/agent-backend'
import type { FoxPermissionTier, NativePermissionCatalog, NativePermissionRequest, NativePermissionView } from '../src/features/conversations/model/agent-native-permission'

afterEach(() => {
  cleanup()
  window.localStorage.removeItem(DEVELOPER_UI_KEY)
})

const text = () => document.body.textContent ?? ''
const renderUi = (ui: React.ReactElement) => render(ui, { wrapper: TooltipProvider })
async function settle() {
  await act(async () => { for (let i = 0; i < 6; i++) await Promise.resolve() })
}

// ---------------------------------------------------------------------------
// The Host's lists, as `fox_agent_backend::native_permission` builds them.

const AUTO_REVIEW_REASON = '「帮我批准」由 Codex 调用 OpenAI 的 codex-auto-review 模型自动审查；当前供应商不是 OpenAI，会被拒绝'
type Opt = NativePermissionCatalog['options'][number]
const opt = (id: string, tier: FoxPermissionTier, label: string, shortLabel: string, extra: Partial<Opt> = {}): Opt => ({
  id, tier, label, shortLabel, description: `${label}说明`, native: id, fullAccess: tier === 'full_access', needsConfirmation: tier === 'full_access', listed: true, disabledReason: null, ...extra,
})

function codexCatalog(openai: boolean): NativePermissionCatalog {
  return {
    backend: 'codex', displayName: 'Codex', defaultMode: 'workspace-write', switchTiming: 'live', switchNote: '切换立即生效（Codex session/set_mode）',
    options: [
      opt('workspace-write', 'ask', '请求审批', '审批'),
      opt('agent', 'auto', '帮我批准', '批准', { disabledReason: openai ? null : AUTO_REVIEW_REASON }),
      opt('agent-full-access', 'full_access', '完全访问权限', '完全'),
      opt('read-only', 'read_only', '只读（旧设置）', '只读', { listed: false }),
    ],
  }
}
const dshCatalog: NativePermissionCatalog = {
  backend: 'deepseek', displayName: 'DeepSeek', defaultMode: 'workspace-write', switchTiming: 'next_turn_restart', switchNote: 'DeepSeek 不支持会话中途切换：下一轮开始时重启 dsh 生效',
  options: [
    opt('read-only', 'read_only', '仅可查看', '查看'),
    opt('workspace-write', 'ask', '工作区完全修改', '工作'),
    opt('danger-full-access', 'full_access', '完全权限', '完全'),
    opt('workspace-write-never', 'auto', '工作区可写·不询问（旧设置）', '工作', { listed: false }),
  ],
}
const foxTiers = (backend: string): NativePermissionCatalog => ({
  backend, displayName: backend, defaultMode: 'ask', switchTiming: backend === 'fox' ? 'next_tool_call' : 'host_answered', switchNote: backend === 'fox' ? '运行中切换，从下一次工具调用起生效' : '由 Fox 审批引擎经 pi-gate 作答：切换后从 pi 的下一次请求起生效',
  options: [opt('read_only', 'read_only', '只读', '只读'), opt('ask', 'ask', '询问', '询问'), opt('allow', 'auto', '自动', '自动'), opt('full_access', 'full_access', '完全访问', '完全')],
})

/** A small Host: catalogs, stored modes and `mode_for_tier` (same rules as the Rust one). */
function fakeHost({ openai = false, running = {} as Record<string, string> } = {}) {
  const catalogs: Record<string, NativePermissionCatalog> = { codex: codexCatalog(openai), deepseek: dshCatalog, pi: foxTiers('pi') }
  const stored: Record<string, string> = {}
  const calls: Array<NativePermissionRequest & { mode?: string }> = []
  const keyOf = (request: NativePermissionRequest) => request.conversationId ?? `draft:${request.backend}`
  const backendOf = (request: NativePermissionRequest) => request.conversationId ? request.conversationId.split(':')[0] : request.backend!
  const mapTier = (catalog: NativePermissionCatalog, tier: FoxPermissionTier | '') => {
    if (!tier || tier === 'full_access') return catalog.defaultMode
    return catalog.options.find((option) => option.tier === tier && option.listed !== false && !option.fullAccess && !option.disabledReason)?.id ?? catalog.defaultMode
  }
  const viewOf = (request: NativePermissionRequest): NativePermissionView => {
    const catalog = catalogs[backendOf(request)]
    const current = stored[keyOf(request)] ?? catalog.defaultMode
    const live = request.conversationId ? running[request.conversationId] ?? null : null
    return {
      catalog, current, currentTier: catalog.options.find((option) => option.id === current)?.tier ?? null,
      running: live, pending: Boolean(live && live !== current),
      fullAccess: catalog.options.some((option) => option.id === current && option.fullAccess), fullAccessGranted: false, fullAccessForbidden: false,
      draft: !request.conversationId, fullAccessLockedReason: null, projectFullAccessConfirmed: false,
    }
  }
  const client = {
    agentBackendNativePermissions: async (request: NativePermissionRequest) => { calls.push(request); return viewOf(request) },
    agentBackendNativePermissionSet: async (request: NativePermissionRequest & { mode?: string }) => {
      calls.push(request)
      const catalog = catalogs[backendOf(request)]
      if (!request.mode && request.carryTier !== undefined) stored[keyOf(request)] = mapTier(catalog, request.carryTier)
      else if (request.mode) stored[keyOf(request)] = request.mode
      return { ...viewOf(request), applied: request.conversationId ? 'applied' : undefined }
    },
  }
  return { client, calls, stored }
}

/** Open a Radix dropdown in happy-dom. */
function openMenu(trigger: HTMLElement) {
  // Radix toggles on Enter (a pointer-down would toggle it a second time).
  fireEvent.keyDown(trigger, { key: 'Enter' })
}
function trigger(): HTMLElement {
  return document.querySelector('.fox-execution-control') as HTMLElement
}

// ---------------------------------------------------------------------------

describe('developer mode', () => {
  test('off by default; the 设置 switch turns it on', () => {
    expect(readDeveloperUi()).toBe(false)
    persistDeveloperUi(true)
    expect(readDeveloperUi()).toBe(true)
    persistDeveloperUi(false)
    expect(readDeveloperUi()).toBe(false)
  })

  test('the retired sandbox chip is gone', () => {
    const dim = (name: string) => ({ summary: `${name}-summary`, detail: `${name}-detail` })
    const capabilities = { backend: 'codex', mode: 'standard', source: 'live', approval: dim('approval'), sandbox: dim('sandbox'), cancel: dim('cancel'), observability: dim('observability'), warning: null } as AgentBackendCapabilities
    expect(agentBackendCapabilityChips(capabilities).map((chip) => chip.key)).toEqual(['approval', 'cancel', 'observability'])
  })

  const status: AgentBackendStatus = {
    switchEnv: 'FOX_EXTERNAL_BACKENDS',
    anyExternalEnabled: true,
    supportedBackends: ['fox', 'pi', 'codex', 'deepseek'],
    retiredBackends: [],
    options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({ backend, enabled: true, wired: true, experimental: false, hidden: false, retired: false, reason: null })),
    unknownEntries: [],
  }
  const dim = (summary: string) => ({ summary, detail: `${summary}说明` })
  function panelClient(loginReady: boolean) {
    return {
      agentBackendCapabilities: async () => ({ backend: 'pi', mode: 'standard', source: 'live', approval: dim('读文件、写文件、命令逐次'), sandbox: dim('Fox 外层无；原生文件无、网络无'), cancel: dim('可确认'), observability: dim('工具起止'), warning: null }),
      agentBackendPermissionStatus: async () => ({ fullAccessForbidden: false, forbiddenReason: null, engineeringSwitch: false }),
      agentBackendCredentials: async () => ({ backend: 'pi', state: loginReady ? 'ready' : 'missing', privateHome: 'D:/fox/homes/p1/pi', credentialsPresent: loginReady, presentKeys: [], presentFiles: [], requiredKeys: loginReady ? [] : ['PI_API_KEY'], requiredFiles: [], templates: [], source: null }),
      agentBackendImportCandidates: async () => [],
      agentBackendImportCredentials: async () => ({ backend: 'pi', homes: 0, files: [], keys: [], skipped: [] }),
      agentBackendOpenPrivateHome: async () => '',
      agentBackendRevokeFullAccess: async () => ({}),
      agentBackendNetworkStatus: async () => ({ previews: [] }),
      agentBackendPreviewAction: async () => true,
      listenRuntimeEvents: async () => () => {},
    }
  }

  test('off: no backend chip, capability chips, configuration fold or local preview above the input', async () => {
    render(<AgentBackendPanel client={panelClient(true) as any} status={status} lockedBackend="pi" conversationId="c-pi" draftBackend="fox" developerUi={false} />)
    await settle()
    await settle()
    expect(text()).not.toContain('已锁定')
    expect(text()).not.toContain('读文件、写文件、命令逐次')
    expect(text()).not.toContain('可确认')
    expect(text()).not.toContain('工具起止')
    expect(text()).not.toContain('Fox 外层无')
    expect(text()).not.toContain('Agent 配置来源与导入')
    expect(text()).not.toContain('本地预览')
    expect(text()).not.toContain('下一次请求起生效')
    expect(screen.queryByLabelText('权限')).toBeNull()
    expect(document.querySelector('.fox-agent-backend-bar')).toBeNull()
  })

  test('off: a login the user must fix is still shown', async () => {
    render(<AgentBackendPanel client={panelClient(false) as any} status={status} lockedBackend="pi" conversationId="c-pi" draftBackend="fox" developerUi={false} />)
    await waitFor(() => expect(text()).toContain('这个后端还没有登录'))
    expect(text()).toContain('从真实 HOME 导入')
    expect(text()).not.toContain('已锁定')
  })

  test('on: the debugging details come back (still no sandbox chip, no second selector)', async () => {
    render(<AgentBackendPanel client={panelClient(true) as any} status={status} lockedBackend="pi" conversationId="c-pi" draftBackend="fox" developerUi />)
    await waitFor(() => expect(text()).toContain('后端：Pi（已锁定）'))
    await waitFor(() => expect(text()).toContain('读文件、写文件、命令逐次'))
    expect(text()).toContain('可确认')
    expect(text()).toContain('工具起止')
    expect(text()).not.toContain('Fox 外层无')
    await waitFor(() => expect(text()).toContain('Agent 配置来源与导入'))
    expect(text()).toContain('本地预览')
    expect(screen.queryByLabelText('权限')).toBeNull()
  })
})

describe('the composer approval selector', () => {
  test('Codex: two characters on the button, full names in the menu, greyed 帮我批准 with its reason', async () => {
    const host = fakeHost()
    renderUi(<NativeApprovalControl client={host.client} conversationId={null} backend="codex" projectId="p1" carryTier="" />)
    await waitFor(() => expect(trigger()?.textContent).toBe('审批'))
    expect(trigger().getAttribute('data-current')).toBe('workspace-write')
    const rows = catalogRows(codexCatalog(false), 'workspace-write')
    expect(rows.map((row) => [row.label, row.shortLabel])).toEqual([['请求审批', '审批'], ['帮我批准', '批准'], ['完全访问权限', '完全']])
    expect(rows[1]).toMatchObject({ disabled: true, reason: AUTO_REVIEW_REASON })
    openMenu(trigger())
    await waitFor(() => expect(screen.getByText('完全访问权限')).toBeTruthy())
    expect(screen.getByText('请求审批')).toBeTruthy()
    expect(screen.getByText(AUTO_REVIEW_REASON)).toBeTruthy()
    expect(document.querySelector('[data-mode="agent"]')?.getAttribute('data-disabled')).not.toBeNull()
    expect(screen.queryByText('只读（旧设置）')).toBeNull()
  })

  test('dsh: 仅可查看 / 工作区完全修改 / 完全权限; a choice waiting for the restart shows「下一轮生效」inside the button', async () => {
    const host = fakeHost({ running: { 'deepseek:c1': 'read-only' } })
    host.stored['deepseek:c1'] = 'workspace-write'
    renderUi(<NativeApprovalControl client={host.client} conversationId="deepseek:c1" backend="deepseek" />)
    await waitFor(() => expect(trigger()?.getAttribute('data-pending')).toBe('true'))
    expect(trigger().textContent).toBe(`工作${PENDING_NEXT_TURN_NOTE}`)
    expect(catalogRows(dshCatalog, 'workspace-write').map((row) => row.label)).toEqual(['仅可查看', '工作区完全修改', '完全权限'])
    // A mid-session conversation never re-asks the Host to carry anything.
    expect(host.calls.every((call) => call.carryTier === undefined)).toBe(true)
    expect(document.querySelectorAll('.fox-agent-backend-native-permission')).toHaveLength(0)
  })

  test('an older build\'s mode is shown while it runs, greyed so it cannot be picked again', () => {
    const rows = catalogRows(codexCatalog(false), 'read-only')
    expect(rows.map((row) => row.value)).toEqual(['workspace-write', 'agent', 'agent-full-access', 'read-only'])
    expect(rows[3]).toMatchObject({ disabled: true, label: '只读（旧设置）' })
  })

  test('pi and Fox use Fox\'s four modes', async () => {
    const host = fakeHost()
    renderUi(<NativeApprovalControl client={host.client} conversationId={null} backend="pi" carryTier="auto" />)
    await waitFor(() => expect(trigger()?.textContent).toBe('自动'))
    cleanup()
    renderUi(<FoxApprovalControl catalog={foxTiers('fox')} value="full_access" />)
    expect(trigger().textContent).toBe('完全')
    cleanup()
    renderUi(<FoxApprovalControl catalog={null} value="read_only" />)
    expect(trigger().textContent).toBe('只读')
    expect(catalogRows(foxTiers('fox'), 'ask').map((row) => row.shortLabel)).toEqual(['只读', '询问', '自动', '完全'])
  })

  test('the pending note is only for a live conversation', () => {
    const base = { catalog: dshCatalog, current: 'workspace-write', fullAccess: false, fullAccessGranted: false, fullAccessForbidden: false, fullAccessLockedReason: null }
    expect(nativePendingNote({ ...base, draft: false, pending: true })).toBe('下一轮生效')
    expect(nativePendingNote({ ...base, draft: false, pending: false })).toBeNull()
    expect(nativePendingNote({ ...base, draft: true, pending: true })).toBeNull()
  })
})

describe('switching the Agent of a new conversation', () => {
  test('which tier is carried', () => {
    expect(draftCarryTier(null, 'codex', 'allow', null)).toBe('')
    expect(draftCarryTier('fox', 'codex', 'ask', null)).toBe('ask')
    expect(draftCarryTier('fox', 'codex', 'allow', null)).toBe('auto')
    expect(draftCarryTier('fox', 'codex', 'read_only', null)).toBe('read_only')
    expect(draftCarryTier('codex', 'deepseek', 'ask', { backend: 'codex', tier: 'auto' })).toBe('auto')
    expect(draftCarryTier('codex', 'deepseek', 'ask', { backend: 'pi', tier: 'auto' })).toBe('')
    expect(draftCarryTier('codex', 'codex', 'ask', { backend: 'codex', tier: 'auto' })).toBe('')
  })

  test('to Codex: 询问 becomes 请求审批; 自动 off OpenAI and 只读 fall back to 请求审批', async () => {
    for (const [tier, openai, expected] of [['ask', false, '审批'], ['auto', false, '审批'], ['read_only', false, '审批'], ['auto', true, '批准'], ['', false, '审批']] as const) {
      const host = fakeHost({ openai })
      renderUi(<NativeApprovalControl client={host.client} conversationId={null} backend="codex" carryTier={tier} />)
      await waitFor(() => expect(trigger()?.textContent).toBe(expected))
      expect(host.calls[0]).toMatchObject({ backend: 'codex', carryTier: tier })
      expect(host.calls[0].mode).toBeUndefined()
      cleanup()
    }
  })

  test('full access is never selected automatically', async () => {
    for (const backend of ['codex', 'deepseek', 'pi']) {
      const host = fakeHost({ openai: true })
      renderUi(<NativeApprovalControl client={host.client} conversationId={null} backend={backend} carryTier="full_access" />)
      await waitFor(() => expect(trigger()).toBeTruthy())
      expect(trigger().textContent).not.toBe('完全')
      expect(host.calls.some((call) => call.mode !== undefined)).toBe(false)
      cleanup()
    }
  })

  test('to dsh: 只读 becomes 仅可查看, 询问 / 自动 become 工作区完全修改', async () => {
    for (const [tier, expected] of [['read_only', '查看'], ['ask', '工作'], ['auto', '工作']] as const) {
      const host = fakeHost()
      renderUi(<NativeApprovalControl client={host.client} conversationId={null} backend="deepseek" carryTier={tier} />)
      await waitFor(() => expect(trigger()?.textContent).toBe(expected))
      cleanup()
    }
  })

  test('picking full access asks for the first-time confirmation before the Host is asked', async () => {
    const host = fakeHost()
    renderUi(<NativeApprovalControl client={host.client} conversationId={null} backend="codex" carryTier="" />)
    await waitFor(() => expect(trigger()?.textContent).toBe('审批'))
    openMenu(trigger())
    await waitFor(() => expect(screen.getByText('完全访问权限')).toBeTruthy())
    fireEvent.click(document.querySelector('[data-mode="agent-full-access"]') as HTMLElement)
    await waitFor(() => expect(screen.getByText('确认使用「完全访问权限」？')).toBeTruthy())
    expect(host.calls.some((call) => call.mode === 'agent-full-access')).toBe(false)
    fireEvent.click(screen.getByText('确认使用完全访问权限'))
    await waitFor(() => expect(host.calls.some((call) => call.mode === 'agent-full-access' && call.confirmFullAccess === true)).toBe(true))
    await waitFor(() => expect(trigger()?.textContent).toBe('完全'))
  })
})
