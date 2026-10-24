// Settings batch: 13.15 re-test fixes ① ② and the new settings (④ ⑤),
// rendered with happy-dom against a fake Host.
//
// ① The draft login probe is keyed by backend + project: a change drops the
//   previous card / chips at once, late answers for an older target are
//   discarded, and the card is remounted per target (no stale local state).
// ② A successful import's receipt is shown once (on the bar); "知道了" or
//   30 s removes the whole text from the DOM. An import error stays in the card.
// ④ The bar has no backend picker; the backend comes from 设置. The settings
//   section greys forced-off backends and broadcasts a change.
import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { act, cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { AgentBackendPanel } = await import('../src/features/chat/components/AgentBackendPanel')
const { AgentBackendSettingsSection, NATIVE_PERMISSION_SETTING_BACKENDS } = await import('../src/features/settings/agent-backend-settings')
const { ComposerAgentPicker } = await import('../src/features/chat/composer/composer-agent-picker')
const { protocolAgents } = await import('../src/features/settings/provider-protocol-editor')
const {
  AGENT_BACKEND_PREFERENCES_EVENT,
  DEFAULT_AGENT_BACKEND_PREFERENCES,
  HIDDEN_AGENT_BACKENDS,
  SELECTABLE_AGENT_BACKEND_IDS,
  agentBackendSettingRows,
  agentBackendSettingValue,
  canPickAgentBackend,
  fullAccessSettingState,
  newConversationBackend,
} = await import('../src/features/conversations/model/agent-backend')
import type { AgentBackendCapabilities, AgentBackendId, AgentBackendPreferences, AgentBackendStatus } from '../src/features/conversations/model/agent-backend'
import type { AgentBackendTarget, CredentialProbe, ImportReport } from '../src/features/conversations/model/agent-backend-onboarding'
const { IMPORT_RECEIPT_VISIBLE_MS } = await import('../src/features/conversations/model/agent-backend-onboarding')

afterEach(cleanup)

interface Deferred<T> { promise: Promise<T>; resolve: (value: T) => void; reject: (error: unknown) => void }
function deferred<T>(): Deferred<T> {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail })
  return { promise, resolve, reject }
}

const status: AgentBackendStatus = {
  switchEnv: 'FOX_EXTERNAL_BACKENDS',
  anyExternalEnabled: true,
  options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({ backend, enabled: true, wired: true, experimental: backend === 'deepseek', reason: null })),
  unknownEntries: [],
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

function capabilities(backend: AgentBackendId, label: string): AgentBackendCapabilities {
  const dim = { summary: label, detail: label }
  return { backend, mode: 'sandboxed', source: 'planned', approval: dim, sandbox: { summary: `${label}-沙箱`, detail: '' }, cancel: { summary: `${label}-取消`, detail: '' }, observability: { summary: `${label}-观测`, detail: '' }, warning: null }
}

/** A fake Host where every probe / capability call is answered by the test. */
function fakeClient() {
  const probes: { target: AgentBackendTarget; reply: Deferred<CredentialProbe> }[] = []
  const caps: { backend: AgentBackendId; reply: Deferred<AgentBackendCapabilities> }[] = []
  const imports: { reply: Deferred<ImportReport> }[] = []
  const client = {
    agentBackendCapabilities: (backend: AgentBackendId) => { const reply = deferred<AgentBackendCapabilities>(); caps.push({ backend, reply }); return reply.promise },
    agentBackendPermissionStatus: async () => ({ fullAccessForbidden: false, forbiddenReason: null, engineeringSwitch: false }),
    agentBackendCredentials: (target: AgentBackendTarget) => { const reply = deferred<CredentialProbe>(); probes.push({ target, reply }); return reply.promise },
    agentBackendImportCandidates: async () => [{ id: 'file:.pi/agent/auth.json', kind: 'file' as const, name: 'auth.json', destination: '.pi/agent/auth.json' }],
    agentBackendImportCredentials: () => { const reply = deferred<ImportReport>(); imports.push({ reply }); return reply.promise },
    agentBackendOpenPrivateHome: async () => '/fox/homes/p1',
    agentBackendRevokeFullAccess: async () => ({}),
    listenRuntimeEvents: async () => () => {},
  }
  return { client, probes, caps, imports }
}

function Panel({ client, backend, project = 'p1', locked = null, conversationId = null }: { client: any; backend: AgentBackendId; project?: string; locked?: AgentBackendId | null; conversationId?: string | null }) {
  return <AgentBackendPanel client={client} status={status} lockedBackend={locked} conversationId={conversationId} draftBackend={backend} draftProjectId={project} draftProjectRoot={`D:/${project}`} />
}

const text = () => document.body.textContent ?? ''

async function settle() {
  await act(async () => { await Promise.resolve(); await Promise.resolve() })
}

describe('① draft login probe is keyed by backend + project', () => {
  test('switching backend drops the old card at once and ignores the late answer of the old target', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="deepseek" />)
    await settle()
    expect(fake.probes.map((probe) => (probe.target as any).backend)).toEqual(['deepseek'])
    await act(async () => fake.probes[0].reply.resolve(missing('deepseek')))
    await waitFor(() => expect(text()).toContain('DEEPSEEK_API_KEY'))

    // Same draft, now pi: the DeepSeek card is gone before pi answers.
    view.rerender(<Panel client={fake.client} backend="pi" />)
    expect(text()).not.toContain('DEEPSEEK_API_KEY')
    expect(text()).not.toContain('这个后端还没有登录')
    await settle()
    const piRequest = fake.probes.at(-1)!
    expect((piRequest.target as any).backend).toBe('pi')

    // Back to DeepSeek and to pi again while answers are in flight: only the
    // answer for the CURRENT target is shown, an older one is discarded.
    view.rerender(<Panel client={fake.client} backend="deepseek" />)
    await settle()
    const lateDeepseek = fake.probes.at(-1)!
    view.rerender(<Panel client={fake.client} backend="pi" />)
    await settle()
    const currentPi = fake.probes.at(-1)!
    await act(async () => lateDeepseek.reply.resolve(missing('deepseek')))
    await act(async () => piRequest.reply.resolve(missing('deepseek')))
    expect(text()).not.toContain('DEEPSEEK_API_KEY')
    await act(async () => currentPi.reply.resolve(missing('pi')))
    await waitFor(() => expect(text()).toContain('.pi/agent/auth.json'))
    expect(text()).toContain('新会话：pi')

    // pi -> Fox: no card, no bar.
    view.rerender(<Panel client={fake.client} backend="fox" />)
    expect(text()).toBe('')
  })

  test('changing the project re-probes and never shows the other project\'s card', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="pi" project="p1" />)
    await settle()
    await act(async () => fake.probes[0].reply.resolve(missing('pi')))
    await waitFor(() => expect(text()).toContain('/fox/homes/p1/pi'))
    view.rerender(<Panel client={fake.client} backend="pi" project="p2" />)
    expect(text()).not.toContain('/fox/homes/p1/pi')
    await settle()
    expect((fake.probes.at(-1)!.target as any).projectId).toBe('p2')
  })

  test('capability chips are cleared on a target change and a late answer is ignored', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="deepseek" />)
    await settle()
    await act(async () => fake.caps[0].reply.resolve(capabilities('deepseek', 'DS审批')))
    await waitFor(() => expect(text()).toContain('DS审批'))
    view.rerender(<Panel client={fake.client} backend="pi" />)
    expect(text()).not.toContain('DS审批')
    await settle()
    const piCaps = fake.caps.at(-1)!
    expect(piCaps.backend).toBe('pi')
    // Codex is hidden for new conversations; an existing (locked) Codex
    // conversation still gets its chips.
    view.rerender(<Panel client={fake.client} backend="fox" locked="codex" conversationId="c-old-codex" />)
    await settle()
    await act(async () => piCaps.reply.resolve(capabilities('pi', 'PI审批')))
    expect(text()).not.toContain('PI审批')
    await act(async () => fake.caps.at(-1)!.reply.resolve(capabilities('codex', 'CX审批')))
    await waitFor(() => expect(text()).toContain('CX审批'))
  })

  test('after a successful import, switching to another backend in the same draft shows its card', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="pi" />)
    await settle()
    await act(async () => fake.probes[0].reply.resolve(missing('pi')))
    await waitFor(() => screen.getByText('从真实 HOME 导入…'))
    fireEvent.click(screen.getByText('从真实 HOME 导入…'))
    await waitFor(() => screen.getByRole('checkbox', { name: 'auth.json' }))
    fireEvent.click(screen.getByRole('checkbox', { name: 'auth.json' }))
    fireEvent.click(screen.getByText('复制选中的项目'))
    await settle()
    await act(async () => fake.imports[0].reply.resolve({ backend: 'pi', homes: 1, homesWritten: 1, files: ['.pi/agent/auth.json'], keys: [], skipped: [] }))
    await settle()
    // The refresh after the import: pi is ready now.
    await act(async () => fake.probes.at(-1)!.reply.resolve({ ...missing('pi'), state: 'ready', credentialsPresent: true }))
    await waitFor(() => expect(text()).not.toContain('这个后端还没有登录'))
    for (const backend of ['deepseek', 'pi'] as const) {
      view.rerender(<Panel client={fake.client} backend={backend} project="p9" />)
      await settle()
      await act(async () => fake.probes.at(-1)!.reply.resolve(missing(backend)))
      await waitFor(() => expect(text()).toContain('这个后端还没有登录'))
      expect(text()).toContain(`/fox/homes/p1/${backend}`)
    }
  })

  test('the card is remounted per target: an import error of one backend does not follow to the next', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="pi" />)
    await settle()
    await act(async () => fake.probes[0].reply.resolve(missing('pi')))
    await waitFor(() => screen.getByText('从真实 HOME 导入…'))
    fireEvent.click(screen.getByText('从真实 HOME 导入…'))
    await waitFor(() => screen.getByRole('checkbox', { name: 'auth.json' }))
    fireEvent.click(screen.getByRole('checkbox', { name: 'auth.json' }))
    fireEvent.click(screen.getByText('复制选中的项目'))
    await settle()
    await act(async () => fake.imports[0].reply.reject(new Error('导入失败：磁盘只读')))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toBe('导入失败：磁盘只读'))
    view.rerender(<Panel client={fake.client} backend="deepseek" />)
    await settle()
    await act(async () => fake.probes.at(-1)!.reply.resolve(missing('deepseek')))
    await waitFor(() => expect(text()).toContain('DEEPSEEK_API_KEY'))
    expect(text()).not.toContain('导入失败')
    expect(screen.queryByRole('checkbox', { name: 'auth.json' })).toBeNull()
  })
})

describe('② the import receipt is shown once and fully removed', () => {
  async function importOnce(stillMissing: boolean) {
    const fake = fakeClient()
    render(<Panel client={fake.client} backend="pi" />)
    await settle()
    await act(async () => fake.probes[0].reply.resolve(missing('pi')))
    await waitFor(() => screen.getByText('从真实 HOME 导入…'))
    fireEvent.click(screen.getByText('从真实 HOME 导入…'))
    await waitFor(() => screen.getByRole('checkbox', { name: 'auth.json' }))
    fireEvent.click(screen.getByRole('checkbox', { name: 'auth.json' }))
    fireEvent.click(screen.getByText('复制选中的项目'))
    await settle()
    await act(async () => fake.imports[0].reply.resolve({ backend: 'pi', homes: 2, homesWritten: 1, files: ['.pi/agent/auth.json'], keys: [], skipped: [] }))
    await settle()
    // The refresh may still say "missing" (e.g. copied to the other project
    // only): the card stays, and must not echo the receipt.
    await act(async () => fake.probes.at(-1)!.reply.resolve(stillMissing ? missing('pi') : { ...missing('pi'), state: 'ready', credentialsPresent: true }))
    await waitFor(() => screen.getByLabelText('关闭导入回执'))
    return fake
  }

  test('"知道了" removes the receipt TEXT from the DOM, not only the button (card still shown)', async () => {
    await importOnce(true)
    const receipt = '已复制到 1 个项目的私有 HOME：auth.json'
    expect(text().split(receipt).length - 1).toBe(1)
    expect(text()).toContain('这个后端还没有登录')
    fireEvent.click(screen.getByLabelText('关闭导入回执'))
    expect(screen.queryByLabelText('关闭导入回执')).toBeNull()
    expect(text()).not.toContain(receipt)
    expect(text()).not.toContain('已复制到')
  })

  test('the receipt disappears entirely after 30 s', async () => {
    // Capture the receipt's 30 s timer instead of waiting for it.
    const realSetTimeout = globalThis.setTimeout
    const armed: (() => void)[] = []
    globalThis.setTimeout = ((callback: () => void, ms?: number, ...rest: unknown[]) => {
      if (ms === IMPORT_RECEIPT_VISIBLE_MS) {
        armed.push(callback)
        return realSetTimeout(() => {}, 0)
      }
      return realSetTimeout(callback as any, ms, ...(rest as []))
    }) as typeof setTimeout
    try {
      await importOnce(false)
      expect(text()).toContain('已复制到 1 个项目的私有 HOME：auth.json')
      expect(armed.length).toBeGreaterThan(0)
      await act(async () => { armed.at(-1)!() })
      expect(text()).not.toContain('已复制到')
      expect(screen.queryByLabelText('导入回执')).toBeNull()
    } finally {
      globalThis.setTimeout = realSetTimeout
    }
  })
})

describe('④ backend comes from the settings; no picker on the bar', () => {
  test('the bar shows the settings backend as a chip, with no select, and no row for Fox', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="pi" />)
    expect(screen.queryByRole('combobox')).toBeNull()
    expect(screen.queryByLabelText('新会话的 Agent 后端')?.tagName).not.toBe('SELECT')
    expect(text()).toContain('新会话：pi')
    expect(text()).toContain('权限：沙箱（来自设置）')
    view.rerender(<Panel client={fake.client} backend="fox" />)
    expect(text()).toBe('')
  })

  function preferences(over: Partial<AgentBackendPreferences> = {}): AgentBackendPreferences {
    return {
      ...DEFAULT_AGENT_BACKEND_PREFERENCES,
      options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({ backend, enabled: true, experimental: backend === 'deepseek', reason: null })),
      ...over,
    }
  }

  test('model: forced off greys every external choice; new conversations use Fox; FORBIDDEN greys full access', () => {
    const forced = preferences({
      forcedOff: true,
      defaultBackend: 'pi',
      effectiveBackend: 'fox',
      options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({ backend, enabled: backend === 'fox', experimental: false, reason: backend === 'fox' ? null : '已被管理员关闭（环境变量 FOX_EXTERNAL_BACKENDS）' })),
    })
    expect(agentBackendSettingRows(forced).map((row) => [row.value, row.disabled])).toEqual([['fox', false], ['pi', true], ['deepseek', true]])
    expect(newConversationBackend(forced)).toBe('fox')
    expect(newConversationBackend(null)).toBe('fox')
    expect(fullAccessSettingState(preferences({ fullAccessForbidden: true, forbiddenReason: '本机已禁止完全访问（FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1）', newExternalFullAccess: true }))).toEqual({ checked: false, disabled: true, reason: '本机已禁止完全访问（FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1）' })
    expect(fullAccessSettingState(preferences())).toEqual({ checked: false, disabled: false, reason: null })
  })

  test('settings section: choosing pi saves it and broadcasts the change (the open draft re-probes)', async () => {
    let saved: AgentBackendPreferences = preferences()
    const changes: unknown[] = []
    const client = {
      agentBackendPreferences: async () => saved,
      saveAgentBackendPreferences: async (change: { defaultBackend?: AgentBackendId; newExternalFullAccess?: boolean }) => {
        changes.push(change)
        saved = { ...saved, defaultBackend: change.defaultBackend ?? saved.defaultBackend, effectiveBackend: change.defaultBackend ?? saved.effectiveBackend }
        return saved
      },
      probeCodexResponses: async () => null,
    }
    const heard: AgentBackendPreferences[] = []
    const listener = (event: Event) => heard.push((event as CustomEvent<AgentBackendPreferences>).detail)
    window.addEventListener(AGENT_BACKEND_PREFERENCES_EVENT, listener)
    render(<AgentBackendSettingsSection client={client} />)
    const select = await waitFor(() => screen.getByLabelText('新会话使用的 Agent') as HTMLSelectElement)
    await waitFor(() => expect(select.disabled).toBe(false))
    expect(select.value).toBe('fox')
    fireEvent.change(select, { target: { value: 'pi' } })
    await waitFor(() => expect(heard.length).toBe(1))
    expect(changes).toEqual([{ defaultBackend: 'pi' }])
    expect(heard[0].effectiveBackend).toBe('pi')
    window.removeEventListener(AGENT_BACKEND_PREFERENCES_EVENT, listener)

    // The panel follows: a draft switched from Fox to pi probes pi at once.
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="fox" />)
    await settle()
    expect(fake.probes).toHaveLength(0)
    view.rerender(<Panel client={fake.client} backend={newConversationBackend(heard[0])} />)
    await settle()
    expect((fake.probes.at(-1)!.target as any).backend).toBe('pi')
  })

  test('settings section: forced off shows Fox only with the explanation; FORBIDDEN greys the full-access switch', async () => {
    const forced = preferences({
      forcedOff: true,
      fullAccessForbidden: true,
      forbiddenReason: '本机已禁止完全访问（FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1）',
      options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({ backend, enabled: backend === 'fox', experimental: false, reason: backend === 'fox' ? null : '已被管理员关闭（环境变量 FOX_EXTERNAL_BACKENDS）' })),
    })
    const client = { agentBackendPreferences: async () => forced, saveAgentBackendPreferences: async () => forced, probeCodexResponses: async () => null }
    render(<AgentBackendSettingsSection client={client} />)
    const select = await waitFor(() => screen.getByLabelText('新会话使用的 Agent') as HTMLSelectElement)
    await waitFor(() => expect(text()).toContain('管理员已通过环境变量 FOX_EXTERNAL_BACKENDS 关闭外部 Agent 后端'))
    const options = Array.from(select.querySelectorAll('option'))
    expect(options.filter((option) => !option.disabled).map((option) => option.value)).toEqual(['fox'])
    const toggle = screen.getByRole('switch', { name: '新外部会话使用完全访问' })
    expect(toggle.hasAttribute('disabled') || toggle.getAttribute('data-disabled') !== null).toBe(true)
    expect(text()).toContain('FOX_EXTERNAL_FULL_ACCESS_FORBIDDEN=1')
  })
})

describe('Codex hidden from user-facing choices (2026-10-24)', () => {
  function preferences(over: Partial<AgentBackendPreferences> = {}): AgentBackendPreferences {
    return {
      ...DEFAULT_AGENT_BACKEND_PREFERENCES,
      options: (['fox', 'pi', 'deepseek'] as const).map((backend) => ({ backend, enabled: true, experimental: backend === 'deepseek', reason: null })),
      ...over,
    }
  }

  test('one central list hides Codex everywhere a user chooses an Agent', () => {
    expect(HIDDEN_AGENT_BACKENDS).toEqual(['codex'])
    expect(SELECTABLE_AGENT_BACKEND_IDS).toEqual(['fox', 'pi', 'deepseek'])
    expect(agentBackendSettingRows(preferences()).map((row) => row.value)).toEqual(['fox', 'pi', 'deepseek'])
    expect(canPickAgentBackend('codex', status)).toBe(false)
    expect(canPickAgentBackend('pi', status)).toBe(true)
    expect(NATIVE_PERMISSION_SETTING_BACKENDS.map((item) => item.backend)).toEqual(['deepseek', 'pi'])
    expect(Object.values(protocolAgents).flat()).not.toContain('codex')
  })

  test('a saved codex preference falls back to Fox and is explained', async () => {
    const saved = preferences({ defaultBackend: 'codex', effectiveBackend: 'fox' })
    expect(newConversationBackend(saved)).toBe('fox')
    expect(newConversationBackend(preferences({ effectiveBackend: 'codex' }))).toBe('fox')
    expect(agentBackendSettingValue(saved)).toBe('fox')
    const client = { agentBackendPreferences: async () => saved, saveAgentBackendPreferences: async () => saved, probeCodexResponses: async () => null }
    render(<AgentBackendSettingsSection client={client} />)
    const select = await waitFor(() => screen.getByLabelText('新会话使用的 Agent') as HTMLSelectElement)
    await waitFor(() => expect(text()).toContain('之前保存的 Codex 暂未开放'))
    expect(select.value).toBe('fox')
    expect(Array.from(select.querySelectorAll('option')).map((option) => option.value)).toEqual(['fox', 'pi', 'deepseek'])
  })

  test('the composer picker never lists Codex', async () => {
    render(<ComposerAgentPicker value="fox" status={status} onSelect={() => {}} />)
    fireEvent.keyDown(screen.getByRole('button', { name: '选择 Agent，当前 Fox' }), { key: 'ArrowDown' })
    await waitFor(() => expect(screen.getByRole('menuitem', { name: 'pi' })).toBeTruthy())
    const items = screen.queryAllByRole('menuitem').map((item) => item.textContent ?? '')
    expect(items.some((item) => item.includes('pi'))).toBe(true)
    expect(items.some((item) => item.includes('Codex'))).toBe(false)
  })

  test('a draft never shows Codex, an existing Codex conversation still opens with its backend', async () => {
    const fake = fakeClient()
    const view = render(<Panel client={fake.client} backend="codex" />)
    await settle()
    expect(text()).not.toContain('Codex')
    view.rerender(<Panel client={fake.client} backend="fox" locked="codex" conversationId="c-old-codex" />)
    await settle()
    expect(text()).toContain('后端：Codex（已锁定）')
    expect(text()).not.toContain('只能查看历史')
  })
})
