// Settings batch ①②④, on the single Pi mainline (product decision 2026-10-27).
//
// A new conversation has exactly one supported driver - the built-in Fox route
// (model engine Pi) - so nothing offers a choice: the bar has no draft row, the
// composer has no Agent menu, and 设置 →「新会话使用的 Agent」is a plain
// statement. An existing conversation frozen to a retired backend (pi / codex /
// deepseek) shows its OWN binding and is read-only.
//
// ① No login probe / capability request / native permission list is made or
//   rendered for a retired backend (draft or locked).
// ② The import receipt the login card used to leave on the bar still renders
//   once and is fully removed ("知道了"); the plumbing itself stays and is
//   covered by agent-backend-onboarding.test.ts.
// ④ The backend of a new conversation comes from the product, not a setting.
import { afterEach, describe, expect, test } from 'bun:test'
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
const { IMPORT_RECEIPT_VISIBLE_MS } = await import('../src/features/conversations/model/agent-backend-onboarding')

afterEach(cleanup)

interface Deferred<T> { promise: Promise<T>; resolve: (value: T) => void; reject: (error: unknown) => void }
function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail })
  return { promise, resolve, reject }
}

/** The status the current Host reports: one supported backend, three retired. */
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
    mode: 'sandboxed',
    state: 'missing_credentials',
    privateHome: `/fox/homes/p1/${backend}`,
    credentialsPresent: false,
    loopbackBlocked: false,
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

function Panel({ client, backend, project = 'p1', locked = null, conversationId = null, refusal = null }: { client: any; backend: AgentBackendId; project?: string; locked?: AgentBackendId | null; conversationId?: string | null; refusal?: string | null }) {
  return <AgentBackendPanel client={client} status={status} lockedBackend={locked} conversationId={conversationId} draftBackend={backend} draftProjectId={project} draftProjectRoot={`D:/${project}`} refusal={refusal} />
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
    const view = render(<Panel client={fake.client} backend="fox" locked="codex" conversationId="c-old-codex" />)
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

    // A refused send replaces the lock notice: one message, never two.
    view.rerender(<Panel client={fake.client} backend="fox" locked="codex" conversationId="c-old-codex" refusal="Agent 后端「Codex」已在本版本退休" />)
    expect(screen.getByRole('alert').textContent).toBe('Agent 后端「Codex」已在本版本退休')
    expect(text()).not.toContain('只能查看历史')
  })

  test('a deepseek conversation shows DeepSeek, not the built-in driver', async () => {
    const fake = fakeClient()
    render(<Panel client={fake.client} backend="fox" locked="deepseek" conversationId="c-old-ds" />)
    await settle()
    expect(text()).toContain('后端：DeepSeek（已锁定）')
    expect(text()).toContain('此会话绑定已退休的 Agent 后端「DeepSeek」')
    expect(text()).not.toContain('内置')
    expect(fake.probes).toHaveLength(0)
  })
})

describe('② the import receipt is rendered once, by the bar, and fully removed', () => {
  function ReceiptBar({ line }: { line: string }) {
    const [receipt, setReceipt] = React.useState<readonly string[] | null>([line])
    return <AgentBackendBar status={status} lockedBackend="codex" draftBackend="fox" importReceipt={receipt} onDismissImportReceipt={() => setReceipt(null)} />
  }

  test('"知道了" removes the receipt TEXT from the DOM, not only the button', async () => {
    const line = '已复制到 1 个项目的私有 HOME：auth.json'
    render(<ReceiptBar line={line} />)
    expect(text().split(line).length - 1).toBe(1)
    fireEvent.click(screen.getByLabelText('关闭导入回执'))
    expect(screen.queryByLabelText('关闭导入回执')).toBeNull()
    expect(text()).not.toContain(line)
    expect(text()).not.toContain('已复制到')
  })

  test('the panel still clears the receipt after its visible window (plumbing kept)', () => {
    const panel = readFileSync(new URL('../src/features/chat/components/AgentBackendPanel.tsx', import.meta.url), 'utf8')
    expect(panel).toContain('IMPORT_RECEIPT_VISIBLE_MS')
    expect(panel).toContain('setReceipt(null)')
    expect(IMPORT_RECEIPT_VISIBLE_MS).toBeGreaterThanOrEqual(10_000)
  })
})

describe('④ 新会话使用的 Agent: a statement, not a choice', () => {
  test('the settings row states the built-in driver and offers no select at all', async () => {
    const client = { agentBackendPreferences: async () => preferences(), saveAgentBackendPreferences: async () => preferences(), probeCodexResponses: async () => null }
    render(<AgentBackendSettingsSection client={client} />)
    await waitFor(() => expect(text()).toContain('新会话使用内置 Pi 驱动'))
    expect(screen.queryByRole('combobox')).toBeNull()
    expect(screen.queryByRole('combobox', { name: '新会话使用的 Agent' })).toBeNull()
    expect(screen.queryByLabelText('新会话使用的 Agent')?.tagName).not.toBe('SELECT')
    // The retirement is explained instead of a greyed option list.
    expect(text()).toContain('外部 Agent 后端已在本版本退休')
    expect(text()).not.toContain('（不可用）')
    // No Fox-wide full-access switch either.
    expect(screen.queryByRole('switch', { name: '新外部会话使用完全访问' })).toBeNull()
    expect(text()).not.toContain('新外部会话使用完全访问')
  })

  test('a saved retired default no longer applies; old conversations keep their binding', async () => {
    const saved = preferences({ defaultBackend: 'codex', effectiveBackend: 'fox' })
    const client = { agentBackendPreferences: async () => saved, saveAgentBackendPreferences: async () => saved, probeCodexResponses: async () => null }
    render(<AgentBackendSettingsSection client={client} />)
    await waitFor(() => expect(text()).toContain('之前保存的「Codex」已退休'))
    expect(text()).toContain('不再用于新会话')
    expect(text()).toContain('仍按原绑定显示')
    expect(text()).toContain('只能查看历史')
    // The statement itself still names the built-in driver, not the saved one.
    expect(text()).toContain('新会话使用内置 Pi 驱动')
    expect(screen.queryByRole('combobox')).toBeNull()
  })

  test('no per-Agent default permission rows are offered for a retired driver', async () => {
    const calls: NativePermissionRequest[] = []
    const client = {
      agentBackendPreferences: async () => preferences(),
      saveAgentBackendPreferences: async () => preferences(),
      probeCodexResponses: async () => null,
      agentBackendNativePermissions: async (request: NativePermissionRequest) => { calls.push(request); throw new Error('must not be called') },
      agentBackendNativePermissionSet: async () => { throw new Error('must not be called') },
    }
    render(<AgentBackendSettingsSection client={client} />)
    await waitFor(() => expect(text()).toContain('新会话使用内置 Pi 驱动'))
    await settle()
    expect(calls).toHaveLength(0)
    expect(NATIVE_PERMISSION_SETTING_BACKENDS).toEqual([])
    expect(screen.queryByLabelText('Codex 默认权限')).toBeNull()
    expect(screen.queryByLabelText('pi 默认权限')).toBeNull()
  })

  test('the model: a retired choice can be neither chosen nor saved', () => {
    expect(SELECTABLE_AGENT_BACKEND_IDS).toEqual(['fox'])
    expect(HIDDEN_AGENT_BACKENDS).toEqual([])
    expect(agentBackendSettingRows(preferences()).map((row) => [row.value, row.disabled])).toEqual([['fox', false]])
    for (const backend of ['pi', 'codex', 'deepseek'] as const) {
      expect(canPickAgentBackend(backend, status)).toBe(false)
      expect(agentBackendSettingValue(preferences({ defaultBackend: backend, effectiveBackend: backend }))).toBe('fox')
      expect(newConversationBackend(preferences({ defaultBackend: backend, effectiveBackend: backend }))).toBe('fox')
      expect(preferences({ defaultBackend: backend }).effectiveBackend).toBe('fox')
    }
    // The Host's own refusal of such a save is a routing refusal, recognised by
    // the UI and never shown as a generic failure.
    expect(AGENT_BACKEND_ERROR_CODES.retired).toBe('agent_backend_retired')
    // The removed Fox-wide full-access switch is permanently unavailable.
    expect(fullAccessSettingState(preferences()).disabled).toBe(true)
  })

  test('the composer has no Agent menu: one driver is not a choice', async () => {
    const view = render(<ComposerAgentPicker value="fox" status={status} onSelect={() => {}} />)
    expect(view.container.textContent).toBe('')
    expect(screen.queryByRole('button')).toBeNull()
    await settle()
    expect(document.querySelector('.fox-composer-agent-trigger')).toBeNull()
  })

  test('the bar has no picker: a new conversation shows no backend row', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="fox" />)
    expect(screen.queryByRole('combobox')).toBeNull()
    expect(text()).toBe('')
    // An old saved default (retired) cannot put an external driver back on the
    // bar: the draft is created as the built-in route.
    const saved = preferences({ defaultBackend: 'deepseek', effectiveBackend: 'deepseek' })
    view.rerender(<Panel client={fake.client} backend={newConversationBackend(saved)} />)
    await settle()
    expect(text()).toBe('')
    expect(fake.probes).toHaveLength(0)
    expect(newConversationBackend(saved)).toBe('fox')
  })
})

describe('single Pi mainline (2026-10-27): the four-driver era is over', () => {
  test('no user-facing choice offers a retired driver', () => {
    expect(RETIRED_AGENT_BACKEND_IDS).toEqual(['pi', 'codex', 'deepseek'])
    expect(SUPPORTED_AGENT_BACKEND_IDS).toEqual(['fox'])
    expect(agentBackendSettingRows(preferences()).map((row) => row.value)).toEqual(['fox'])
    expect(agentBackendSettingRows(preferences()).some((row) => row.value !== 'fox')).toBe(false)
    expect(canPickAgentBackend('codex', status)).toBe(false)
    expect(canPickAgentBackend('pi', status)).toBe(false)
    expect(NATIVE_PERMISSION_SETTING_BACKENDS).toEqual([])
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
