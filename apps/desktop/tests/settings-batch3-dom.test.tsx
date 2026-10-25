// Settings batch 3 (能力 / 系统): Agent, 插件, 资料库, 数据与关于 in the unified
// layout. Collections open a right-side Sheet, destructive actions go through
// the in-app confirm dialog, the 资料库 page shows the degraded storage warning
// with its fix, 「添加技能」 keeps the expert it came from, and the static rows
// with no state behind them are gone.
import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const now = 1_700_000_000_000
const manifest = { version: '1.0.0', manifestSchemaVersion: 2, skills: [], knowledgeReferences: [], mcpServers: [], allowedTools: [] }
const agent = (o: Record<string, unknown>) => ({ description: '', runtimeType: 'pi', defaultModel: 'configured-model', icon: null, category: 'general', openingSuggestions: [], systemPrompt: '你是一名助手。', isBuiltin: false, packageVersion: '1.0.0', packageSource: 'local', packageId: null, packageHash: null, packageManifest: manifest, capabilities: null, resources: { tools: [], knowledges: [], mcps: [], skills: [] }, configurableItems: null, isDefault: false, available: true, agentKind: 'expert', invocationMode: 'inline', visibility: 'expert_center', ...o })
const agents = [
  agent({ id: 'fox-general', name: 'Fox 通用助手', agentKind: 'assistant', invocationMode: 'primary', visibility: 'chat_selector', isBuiltin: true, packageSource: 'builtin' }),
  agent({ id: 'local-writer', name: '周报撰写助手', description: '整理周报' }),
]
const servers = [
  { id: 'filesystem', name: 'Filesystem', command: 'npx.cmd', args: ['-y', 'server-filesystem'], transport: 'stdio', endpointUrl: null, definition: null, enabled: true, status: 'unknown', credentialConfigured: false, lastError: null, lastCheckedAt: null, lastLatencyMs: null, toolCount: null, consecutiveFailures: 0, createdAt: now, updatedAt: now },
]
const skills = [
  { id: 'weekly-report', name: '周报模板', description: '三段式周报', version: '1.2.0', requiredTools: [], sourcePath: 'C:\\skills\\weekly-report\\SKILL.md', enabled: true, valid: true, validationError: null, instructions: '# 周报' },
]
const calls: string[] = []

const actualDesktopClient = { ...await import('../src/features/conversations/api/desktop-client') }
afterAll(() => { mock.module('../src/features/conversations/api/desktop-client', () => actualDesktopClient) })
mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...actualDesktopClient.desktopClient,
    listAgents: async () => agents.map((item) => ({ ...item })),
    syncYuxiAgents: async () => null,
    listSkills: async () => skills.map((item) => ({ ...item })),
    listMcpServers: async () => servers.map((item) => ({ ...item })),
    deleteMcpServer: async (id: string) => { calls.push(`delete:${id}`); return true },
    listLifecycleHooks: async () => [],
    listManagedProjects: async () => [],
    listModelProviders: async () => [],
    getModelService: async () => null,
    getYuxiService: async () => null,
    getYuxiUser: async () => null,
    agentBackendPreferences: async () => { throw new Error('not in this test') },
    usageStatistics: async () => { throw new Error('not in this test') },
    observabilityStatistics: async () => { throw new Error('not in this test') },
    runtimeDiagnostics: async () => { throw new Error('not in this test') },
    knowledgePreviewCacheStatistics: async () => ({ totalFiles: 3, activeFiles: 1, totalBytes: 3 * 1024 * 1024, activeBytes: 1024 * 1024, limitBytes: 1024 * 1024 * 1024 }),
    clearKnowledgePreviewCache: async () => { calls.push('clear-cache'); return { message: '已清理预览缓存', bytes: 2 * 1024 * 1024 } },
  },
}))

const React = await import('react')
const { act, cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const pages = await import('../src/features/settings/settings-pages')
const { TooltipProvider } = await import('../src/components/ui/tooltip')
const { ConfirmDialogProvider } = await import('../src/components/ui/confirm-dialog')
const { PLUGIN_RETURN_KEY, rememberPluginReturn } = await import('../src/features/settings/settings-shared')
const { settingsRouteRedirect, settingsNavigationView } = await import('../src/features/workspace/management-navigation')

afterEach(() => { cleanup(); calls.length = 0; window.sessionStorage.clear() })

const text = () => document.body.textContent ?? ''
const byLabel = (label: string) => document.querySelector(`[aria-label="${label}"]`) as HTMLElement | null
const buttonNamed = (root: ParentNode, name: string) => [...root.querySelectorAll('button')].find((item) => item.textContent?.trim() === name) as HTMLElement | undefined
const sheet = () => document.querySelector('[data-slot="sheet-content"]') as HTMLElement | null

async function mount(element: React.ReactElement) {
  await act(async () => {
    render(React.createElement(TooltipProvider, null, React.createElement(ConfirmDialogProvider), element))
  })
}

const base = { sidebarCollapsed: false, onSidebar: () => {} }

describe('Agent page', () => {
  test('clicking an expert opens its edit Sheet straight away; fake 高风险操作 rows are gone', async () => {
    await mount(React.createElement(pages.AgentSettingsPage, { ...base, navigate: () => {} }))
    await waitFor(() => { if (!byLabel('编辑 周报撰写助手')) throw new Error('expert row missing') })
    expect(text()).not.toContain('资料库内容注入')
    expect(text()).not.toContain('每次询问')
    await act(async () => { fireEvent.click(byLabel('编辑 周报撰写助手')!) })
    await waitFor(() => { if (!sheet()) throw new Error('sheet not open') })
    expect(sheet()!.getAttribute('data-side')).toBe('right')
    expect(sheet()!.className).toContain('fox-settings-sheet')
    expect((sheet()!.querySelector('input') as HTMLInputElement).value).toBe('周报撰写助手')
  })

  test('an expert id in the route reopens that expert (the way back from 添加技能)', async () => {
    await mount(React.createElement(pages.AgentSettingsPage, { ...base, navigate: () => {}, agentId: 'local-writer' }))
    await waitFor(() => { if (!sheet()) throw new Error('sheet not open') })
    expect(sheet()!.textContent).toContain('周报撰写助手')
  })

  test('添加技能 in the expert Sheet remembers the expert before opening 插件', async () => {
    const navigated: unknown[][] = []
    await mount(React.createElement(pages.AgentSettingsPage, { ...base, navigate: (...args: unknown[]) => { navigated.push(args) }, agentId: 'local-writer' }))
    await waitFor(() => { if (!sheet()) throw new Error('sheet not open') })
    const tab = [...sheet()!.querySelectorAll('[role="tab"]')].find((item) => item.textContent === '能力') as HTMLElement
    await act(async () => { fireEvent.mouseDown(tab); fireEvent.click(tab) })
    await waitFor(() => { if (!buttonNamed(sheet()!, '添加技能')) throw new Error('no 添加技能') })
    await act(async () => { fireEvent.click(buttonNamed(sheet()!, '添加技能')!) })
    expect(navigated.at(-1)).toEqual(['skills'])
    expect(JSON.parse(window.sessionStorage.getItem(PLUGIN_RETURN_KEY)!)).toEqual({ view: 'settings-ai', entityId: 'local-writer', label: '周报撰写助手' })
  })
})

describe('插件 page', () => {
  test('connector row opens the edit Sheet; 删除 asks the in-app confirm before deleting', async () => {
    await mount(React.createElement(pages.ExtensionsSettingsPage, { ...base, navigate: () => {} }))
    await waitFor(() => { if (!byLabel('编辑 Filesystem')) throw new Error('connector row missing') })
    expect(text()).toContain('周报模板')
    await act(async () => { fireEvent.click(byLabel('编辑 Filesystem')!) })
    await waitFor(() => { if (!sheet()) throw new Error('sheet not open') })
    await act(async () => { fireEvent.click(buttonNamed(sheet()!, '删除连接器')!) })
    const confirm = await waitFor(() => { const box = document.querySelector('[data-testid="fox-confirm-dialog"]'); if (!box) throw new Error('no confirm'); return box as HTMLElement })
    expect(confirm.textContent).toContain('删除连接器')
    expect(calls).toEqual([])
    await act(async () => { fireEvent.click(buttonNamed(confirm, '删除')!) })
    await waitFor(() => expect(calls).toEqual(['delete:filesystem']))
  })

  test('opened from an expert, the page offers the way back and clears the stored target', async () => {
    rememberPluginReturn({ view: 'agent-detail', entityId: 'local-writer', documentId: 'skills', label: '周报撰写助手' })
    const navigated: unknown[][] = []
    await mount(React.createElement(pages.ExtensionsSettingsPage, { ...base, navigate: (...args: unknown[]) => { navigated.push(args) }, section: 'skills' }))
    const back = await waitFor(() => { const found = buttonNamed(document, '返回 周报撰写助手'); if (!found) throw new Error('no back button'); return found })
    expect(window.sessionStorage.getItem(PLUGIN_RETURN_KEY)).toBeNull()
    await act(async () => { fireEvent.click(back) })
    expect(navigated.at(-1)).toEqual(['agent-detail', 'local-writer', { documentId: 'skills' }])
  })

  test('技能 / 连接器 routes land on the 插件 page', () => {
    expect(settingsRouteRedirect('skills')).toEqual({ view: 'settings-extensions', section: 'skills' })
    expect(settingsRouteRedirect('mcp')).toEqual({ view: 'settings-extensions', section: 'mcp' })
    expect(settingsNavigationView('skills')).toBe('settings-extensions')
  })
})

describe('资料库 page', () => {
  test('a degraded store shows the warning and opens the storage dialog with 恢复默认位置', async () => {
    const status = { rootPath: 'D:\\FoxData\\knowledge', databasePath: 'D:\\FoxData\\knowledge\\fox.db', writable: false, schemaVersion: 7, pendingRootPath: null, restartRequired: false, migrationId: null, degradedReason: '知识库路径不可写：D:\\FoxData\\knowledge' }
    const gateway = { getStorageStatus: async () => status, listKnowledgeBases: async () => [], resetStoragePath: async () => status, pickStorageDirectory: async () => null, getStorageMigration: async () => { throw new Error('none') }, startStorageMigration: async () => { throw new Error('none') } } as any
    await mount(React.createElement(pages.YuxiSettingsPage, { ...base, navigate: () => {}, gateway }))
    const alert = await waitFor(() => { const box = document.querySelector('[data-testid="fox-settings-storage-degraded"]'); if (!box) throw new Error('no warning'); return box as HTMLElement })
    expect(alert.getAttribute('role')).toBe('alert')
    expect(alert.textContent).toContain('知识库路径不可写')
    await act(async () => { fireEvent.click(buttonNamed(alert, '修改存储位置')!) })
    await waitFor(() => { if (!buttonNamed(document, '恢复默认位置')) throw new Error('storage dialog not open') })
  })
})

describe('数据与关于 page', () => {
  test('清理未使用缓存 asks the in-app confirm; the fake 架构文档 row is gone', async () => {
    await mount(React.createElement(pages.DataAboutSettingsPage, { ...base, navigate: () => {} }))
    const clear = await waitFor(() => { const found = buttonNamed(document, '清理未使用缓存'); if (!found || (found as HTMLButtonElement).disabled) throw new Error('not ready'); return found })
    expect(text()).not.toContain('架构文档')
    expect(text()).not.toContain('本机文档')
    await act(async () => { fireEvent.click(clear) })
    const confirm = await waitFor(() => { const box = document.querySelector('[data-testid="fox-confirm-dialog"]'); if (!box) throw new Error('no confirm'); return box as HTMLElement })
    expect(calls).toEqual([])
    await act(async () => { fireEvent.click(buttonNamed(confirm, '清理')!) })
    await waitFor(() => expect(calls).toEqual(['clear-cache']))
  })
})
