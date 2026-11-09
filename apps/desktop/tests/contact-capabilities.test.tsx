import { afterEach, describe, expect, test } from 'bun:test'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import { ContactCapabilities, connectorStatus, CAPABILITY_FOOTER, STDIO_BOUNDARY } from '../src/features/companions/capabilities'
import { ContactProfile } from '../src/features/companions/profile'
import type { Companion, CompanionCapabilities, ConnectorCapability } from '../src/features/companions/api'

const contact: Companion = {
  id: 'contact', conversationId: 'chat', templateAgentId: 'expert', displayName: '资料伙伴', avatarRef: '📚',
  duty: '', personaOverride: null, workspaceDir: 'D:/contact', workspaceState: 'ready', workspaceError: null,
  isDefault: false, pinnedAt: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
}

const connector = (patch: Partial<ConnectorCapability> = {}): ConnectorCapability => ({
  serverId: 'feishu', name: '飞书', transport: 'streamable-http', enabled: true, status: 'connected', lastError: null,
  tools: [{ name: 'read_calendar', granted: true }, { name: 'send_message', granted: false }],
  ...patch,
})
const capabilities = (patch: Partial<CompanionCapabilities> = {}): CompanionCapabilities => ({
  web: [{ tool: 'web_search', granted: true }, { tool: 'web_read', granted: false }],
  connectors: [
    connector(),
    connector({ serverId: 'local-files', name: '本机文件助手', transport: 'stdio', enabled: false, status: 'stopped', tools: [] }),
    connector({ serverId: 'github', name: 'GitHub', transport: 'sse', status: 'error', lastError: '还没有登录', tools: [] }),
  ],
  ...patch,
})

const context = (current: Companion): ContactsContextValue => ({
  current, privateChat: true, list: { companions: [current], pendingDeletions: [] }, projects: [], model: '全局模型', permission: 'ask', error: '',
  refresh: async () => {}, refreshConversation: async () => {}, open: () => {}, add: () => {}, showProfile: () => {},
  leave: () => {}, setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
})

interface Harness { gets: number; calls: Record<string, unknown>[] }
function mockCapabilities(data: CompanionCapabilities = capabilities(), harness: Harness = { gets: 0, calls: [] }) {
  mockIPC((command, args) => {
    if (command === 'companion_capabilities_get') { harness.gets += 1; return { ok: true, data } }
    if (command === 'companion_capability_set') { harness.calls.push(args as Record<string, unknown>); return { ok: true, data: null } }
    throw new Error('Unexpected command: ' + command)
  })
  return harness
}

afterEach(() => { cleanup(); clearMocks() })

describe('联系人可用能力', () => {
  test('状态映射：未启用、还没连接、已连接', () => {
    expect(connectorStatus(connector({ enabled: false })).label).toBe('未启用')
    expect(connectorStatus(connector({ lastError: '连接超时' })).label).toBe('还没连接')
    expect(connectorStatus(connector()).label).toBe('已连接')
    expect(connectorStatus(connector({ transport: 'STDIO', enabled: false })).label).toBe('未启用')
  })

  test('打开面板读取能力，网页两个开关显示中文标签与初始状态', async () => {
    const harness = mockCapabilities()
    const view = render(<ContactCapabilities contact={contact} />)
    expect(view.getByText('正在读取可用能力…')).toBeDefined()
    await waitFor(() => expect(harness.gets).toBe(1))
    expect(view.getByText('可用能力')).toBeDefined()
    const search = view.getByRole('switch', { name: '网页搜索' })
    const read = view.getByRole('switch', { name: '读取网页' })
    expect(search.getAttribute('aria-checked')).toBe('true')
    expect(read.getAttribute('aria-checked')).toBe('false')
    expect(view.getByText('用已配置的搜索服务查公开网页')).toBeDefined()
    expect(view.getByText('打开链接，读取正文')).toBeDefined()
  })

  test('切换网页开关写入 @web 授权，改完重新读取', async () => {
    const harness = mockCapabilities()
    const view = render(<ContactCapabilities contact={contact} />)
    await waitFor(() => expect(harness.gets).toBe(1))
    fireEvent.click(view.getByRole('switch', { name: '网页搜索' }))
    await waitFor(() => expect(harness.calls).toEqual([{ request: { companionId: 'contact', serverId: '@web', tool: 'web_search', granted: false } }]))
    await waitFor(() => expect((view.getByRole('switch', { name: '读取网页' }) as HTMLButtonElement).disabled).toBe(false))
    fireEvent.click(view.getByRole('switch', { name: '读取网页' }))
    await waitFor(() => expect(harness.calls).toHaveLength(2))
    expect(harness.calls[1]).toEqual({ request: { companionId: 'contact', serverId: '@web', tool: 'web_read', granted: true } })
    await waitFor(() => expect(harness.gets).toBe(3))
  })

  test('启用连接器列出工具并可勾选；停用连接器不列工具', async () => {
    const harness = mockCapabilities()
    const view = render(<ContactCapabilities contact={contact} />)
    await waitFor(() => expect(harness.gets).toBe(1))
    expect(view.getByText('已连接')).toBeDefined()
    expect(view.getByText('· 用 1 个，共 2 个')).toBeDefined()
    const calendar = view.getByLabelText('允许使用 飞书 的工具 read_calendar') as HTMLInputElement
    const send = view.getByLabelText('允许使用 飞书 的工具 send_message') as HTMLInputElement
    expect(calendar.checked).toBe(true)
    expect(send.checked).toBe(false)
    fireEvent.click(send)
    await waitFor(() => expect(harness.calls).toEqual([{ request: { companionId: 'contact', serverId: 'feishu', tool: 'send_message', granted: true } }]))

    expect(view.getByText('未启用')).toBeDefined()
    expect(view.getByText('启用后可选择工具')).toBeDefined()
    const checkboxes = view.queryAllByRole('checkbox')
    expect(checkboxes).toHaveLength(2)
    expect(checkboxes.some(box => (box.getAttribute('aria-label') ?? '').includes('本机文件助手'))).toBe(false)

    expect(view.getByText('还没连接')).toBeDefined()
    expect(view.getByText('还没有登录')).toBeDefined()
    expect(view.getByText('没有发现可用工具。')).toBeDefined()
  })

  test('stdio 连接器显示本机运行边界说明', async () => {
    mockCapabilities()
    const view = render(<ContactCapabilities contact={contact} />)
    await waitFor(() => expect(view.getByText(STDIO_BOUNDARY)).toBeDefined())
    expect(view.queryAllByText(STDIO_BOUNDARY)).toHaveLength(1)
  })

  test('页脚说明生效时机', async () => {
    mockCapabilities()
    const view = render(<ContactCapabilities contact={contact} />)
    await waitFor(() => expect(view.getByText(CAPABILITY_FOOTER)).toBeDefined())
  })

  test('没有连接器时网页区仍可用', async () => {
    mockCapabilities(capabilities({ connectors: [] }))
    const view = render(<ContactCapabilities contact={contact} />)
    await waitFor(() => expect(view.getByText('还没有配置连接器。')).toBeDefined())
    expect(view.getByRole('switch', { name: '网页搜索' })).toBeDefined()
    expect(view.getByRole('switch', { name: '读取网页' })).toBeDefined()
  })

  test('读取失败显示原因并可重试', async () => {
    let fail = true
    const harness = { gets: 0, calls: [] }
    mockIPC(command => {
      if (command === 'companion_capabilities_get') {
        harness.gets += 1
        return fail ? { ok: false, error: { code: 'storage.error', message: '读取失败', retryable: true } } : { ok: true, data: capabilities() }
      }
      throw new Error('Unexpected command: ' + command)
    })
    const view = render(<ContactCapabilities contact={contact} />)
    await waitFor(() => expect(view.getByRole('alert').textContent).toContain('读取失败'))
    fail = false
    fireEvent.click(view.getByText('重试'))
    await waitFor(() => expect(view.getByRole('switch', { name: '网页搜索' })).toBeDefined())
    expect(harness.gets).toBe(2)
  })

  test('保存失败不改动当前状态', async () => {
    const harness: Harness = { gets: 0, calls: [] }
    mockIPC(command => {
      if (command === 'companion_capabilities_get') { harness.gets += 1; return { ok: true, data: capabilities() } }
      if (command === 'companion_capability_set') { harness.calls.push({}); return { ok: false, error: { code: 'companion.capability_invalid', message: '无法保存', retryable: false } } }
      throw new Error('Unexpected command: ' + command)
    })
    const view = render(<ContactCapabilities contact={contact} />)
    await waitFor(() => expect(harness.gets).toBe(1))
    fireEvent.click(view.getByRole('switch', { name: '网页搜索' }))
    await waitFor(() => expect(harness.calls).toHaveLength(1))
    expect(harness.gets).toBe(1)
    expect(view.getByRole('switch', { name: '网页搜索' }).getAttribute('aria-checked')).toBe('true')
  })

  test('资料页入口打开可用能力，并可返回资料', async () => {
    const harness: Harness = { gets: 0, calls: [] }
    mockIPC(command => {
      if (command === 'companion_template_status') return { ok: true, data: { latestHash: null, boundSnapshot: null, latestSnapshot: null, updateAvailable: false, dismissed: false } }
      if (command === 'model_providers_list') return { ok: true, data: [] }
      if (command === 'companion_capabilities_get') { harness.gets += 1; return { ok: true, data: capabilities() } }
      if (command === 'companion_capability_set') { harness.calls.push({}); return { ok: true, data: null } }
      throw new Error('Unexpected command: ' + command)
    })
    const view = render(<ContactsContext.Provider value={context(contact)}>
      <ContactProfile contact={contact} />
    </ContactsContext.Provider>)
    fireEvent.click(await waitFor(() => view.getByText('查看和设置')))
    await waitFor(() => expect(harness.gets).toBe(1))
    await waitFor(() => expect(view.getByRole('switch', { name: '网页搜索' })).toBeDefined())
    expect(view.getByText('返回资料')).toBeDefined()
    fireEvent.click(view.getByText('返回资料'))
    await waitFor(() => expect(view.getByText('查看和设置')).toBeDefined())
    expect(view.queryByRole('switch', { name: '网页搜索' })).toBeNull()
  })
})