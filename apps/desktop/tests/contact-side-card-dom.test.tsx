// SPEC item 3（缺口 D / E）：联系人资料卡的真实 API 合同与被保留的功能。
//
// 缺口 D 曾是真实的坏控件：权限下拉用了不存在的值、打开文件夹传了目录而不是
// **会话 id**、导出传的是整个工作目录而不是**某个文件的相对路径**、导入的成功
// 提示和工作目录错误都丢了。缺口 E 是旧资料页被删掉后无处可去的头像/职责/人设
// 编辑、模板采用/暂不更新、项目笔记授权、置顶/归档/删除。这里用真 DOM 交互 +
// 真 IPC 边界断言它们都在、参数都对、失败不被吞掉。
import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import { ContactSideCard } from '../src/features/companions/contact-side-card'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import type { ContactChatSearch } from '../src/features/companions/chat-search'
import type { Companion } from '../src/features/companions/api'
import type { ConversationDetail, ProjectRecord } from '../src/features/conversations/model/types'

type Call = { command: string; request: Record<string, unknown> }
let calls: Call[] = []
let workspaceState = 'ready'
let workspaceError: string | null = null

const contact = (patch: Partial<Companion> = {}): Companion => ({
  id: 'cmp_one', conversationId: 'chat', templateAgentId: 'expert', displayName: '资料伙伴', avatarRef: '📚',
  duty: '整理资料', personaOverride: '说话简短', workspaceDir: 'C:/fox/contact', workspaceState, workspaceError,
  isDefault: false, pinnedAt: 1, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  reminderMode: 'all', lastReadDeliveryOrder: null, manualUnreadFromOrder: null, manualUnreadAt: null,
  readDeliveryOrder: 0, pendingActionCount: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null, ...patch,
})
const detail = {
  conversation: { id: 'chat', permissionMode: 'ask' },
  contextReset: { supported: true, reason: null, code: null },
  contextResetBlockedReason: null,
} as unknown as ConversationDetail

const permissionChoices: string[] = []
const sharedMemory = { count: 0 }
const searchOpens = { count: 0 }
const searchStub = { open: () => { searchOpens.count += 1 } } as ContactChatSearch

function contextValue(overrides: Partial<ContactsContextValue> = {}): ContactsContextValue {
  return {
    current: contact(), privateChat: true, list: { companions: [], pendingDeletions: [] },
    projects: [{ id: 'proj_a', name: '华东项目', status: 'active' } as ProjectRecord],
    model: 'test-model', permission: 'ask', error: '',
    refresh: async () => {}, refreshConversation: async () => {}, open: () => {}, add: () => {}, showProfile: () => {},
    leave: () => {}, setPermission: (mode) => { permissionChoices.push(mode) },
    openSharedMemory: () => { sharedMemory.count += 1 }, askToOrganize: () => {}, ...overrides,
  }
}

// 同一进程里别的文件可能已经换掉 invoke；自己接管并在结束后原样还回去。
const tauriCoreAsFound = { ...(await import('@tauri-apps/api/core')) }
let answer: ((command: string, args: unknown) => unknown) | null = null
mock.module('@tauri-apps/api/core', () => ({
  ...tauriCoreAsFound,
  invoke: async (command: string, args: unknown) => answer
    ? answer(command, args)
    : (tauriCoreAsFound.invoke as (c: string, a: unknown) => Promise<unknown>)(command, args),
}))

// 导入的成功提示与失败上报都走 notify；接管它才能断言反馈没被吞掉。
const actualNotifications = await import('../src/features/notifications')
const toasts: Array<{ type: string; message: string }> = []
mock.module('../src/features/notifications', () => ({
  ...actualNotifications,
  notify: Object.assign(
    (message: unknown, options?: unknown) => actualNotifications.notify(message, options as never),
    {
      ...actualNotifications.notify,
      success: (message: unknown) => { toasts.push({ type: 'success', message: String(message) }); return 'toast' },
      error: (message: unknown) => { toasts.push({ type: 'error', message: String(message) }); return 'toast' },
    },
  ),
}))
afterAll(() => {
  answer = null
  mock.module('@tauri-apps/api/core', () => tauriCoreAsFound)
  mock.module('../src/features/notifications', () => actualNotifications)
})

const ok = (data: unknown = true) => ({ ok: true, data })
/** 模板状态必须随决策改变：Host 在 adopt/dismiss 之后返回的就是新状态，卡片要
 *  重新读一次并显示它，而不是停留在旧 diff 上。 */
let templateStatus = { latestHash: 'hash-2', updateAvailable: true, dismissed: false }
function host() {
  calls = []
  toasts.length = 0
  permissionChoices.length = 0
  sharedMemory.count = 0
  searchOpens.count = 0
  workspaceState = 'ready'
  workspaceError = null
  templateStatus = { latestHash: 'hash-2', updateAvailable: true, dismissed: false }
  answer = (command, args) => {
    // direct() 传扁平参数，command() 传 { request }；两种形状都记录下来。
    const request = ((args as { request?: Record<string, unknown> })?.request ?? args) as Record<string, unknown>
    calls.push({ command, request })
    if (command === 'model_providers_list') return ok([])
    if (command === 'companion_set_reminder_mode') return ok(contact({ reminderMode: request.mode as Companion['reminderMode'] }))
    if (command === 'project_folder_open') return ok(true)
    if (command === 'project_files_list') return ok([
      { path: 'notes/报价.md', name: '报价.md', isDirectory: false },
      { path: 'notes', name: 'notes', isDirectory: true },
    ])
    if (command === 'companion_export_file') return ok('C:/Downloads/报价.md')
    if (command === 'companion_import_files') return ok(['a.pdf', 'b.pdf', 'c.pdf'])
    if (command === 'companion_workspace_repair') return ok('C:/fox/contact')
    if (command === 'companion_update_profile') return ok(contact(request as Partial<Companion>))
    if (command === 'companion_template_status') return ok({
      latestHash: templateStatus.latestHash,
      boundSnapshot: { name: '资料伙伴', packageVersion: '1.0', systemPrompt: '旧工作要求' },
      latestSnapshot: { name: '资料伙伴', packageVersion: '1.1', systemPrompt: '新工作要求' },
      updateAvailable: templateStatus.updateAvailable, dismissed: templateStatus.dismissed,
    })
    if (command === 'companion_template_update') { templateStatus.updateAvailable = false; return ok(null) }
    if (command === 'companion_template_dismiss') { templateStatus.dismissed = true; return ok(null) }
    if (command === 'companion_grants_list') return ok([{ companionId: 'cmp_one', projectId: 'proj_a', grantedAt: 1 }])
    if (command === 'companion_grant_project' || command === 'companion_revoke_project') return ok(null)
    if (command === 'companion_set_pinned' || command === 'companion_archive') return ok(contact({ pinnedAt: command === 'companion_set_pinned' ? null : 1, archivedAt: command === 'companion_archive' ? 1 : null }))
    if (command === 'companion_delete_preview') return ok({
      planHash: 'plan-1',
      plan: { companionId: 'cmp_one', displayName: '资料伙伴', workspacePath: 'C:/fox/contact', workspaceFiles: 2, workspaceBytes: 20, messages: 3, notes: 1, topics: 0, sharedMemories: 0, generations: 0, grantedProjects: ['proj_a'], files: [] },
    })
    throw new Error('Unexpected command: ' + command)
  }
}

const renderCard = (ctx: ContactsContextValue = contextValue(), patch: Partial<Companion> = {}) =>
  render(<ContactsContext.Provider value={ctx}>
    <ContactSideCard contact={contact(patch)} detail={detail} onClose={() => undefined} onResetContext={() => undefined} search={searchStub} />
  </ContactsContext.Provider>)

const openTab = (view: ReturnType<typeof renderCard>, label: string) => fireEvent.click(view.getByRole('tab', { name: label }))
const openSettings = (view: ReturnType<typeof renderCard>) => {
  fireEvent.click(view.getByRole('button', { name: '设置' }))
  expect(view.getByRole('heading', { level: 2 }).textContent).toBe('联系人设置')
}
const commandNames = () => calls.map((call) => call.command)
const requestOf = (command: string) => calls.find((call) => call.command === command)!.request
const toasted = (type: string, text: string) => toasts.some((item) => item.type === type && item.message.includes(text))

afterEach(cleanup)

describe('联系人资料卡的 API 合同（缺口 D）', () => {
  test('权限下拉只有真实契约值 read_only / ask / allow，并且走 ctx.setPermission', () => {
    host()
    const view = renderCard()
    const select = view.getByLabelText('权限') as HTMLSelectElement
    const values = Array.from(select.options).map((option) => option.value)
    expect(values).toEqual(['read_only', 'ask', 'allow'])
    for (const dead of ['default', 'allow_edits', 'dont_ask', 'plan']) expect(values).not.toContain(dead)
    expect(select.value).toBe('ask')
    fireEvent.change(select, { target: { value: 'allow' } })
    expect(permissionChoices).toEqual(['allow'])
  })

  test('提醒行保留全部模式，保存后刷新联系人并显示反馈', async () => {
    host()
    let refreshed = 0
    const view = renderCard(contextValue({ refresh: async () => { refreshed += 1 } }))
    const select = view.getByLabelText('提醒') as HTMLSelectElement
    expect(Array.from(select.options).map(option => option.value)).toEqual(['follow_global', 'all', 'action_only', 'muted'])
    fireEvent.change(select, { target: { value: 'action_only' } })
    expect(select.disabled).toBe(true)
    await waitFor(() => expect(commandNames()).toContain('companion_set_reminder_mode'))
    expect(requestOf('companion_set_reminder_mode')).toEqual({ companionId: 'cmp_one', mode: 'action_only' })
    await waitFor(() => expect(select.disabled).toBe(false))
    expect(refreshed).toBe(1)
    expect(toasted('success', '提醒设置已保存')).toBe(true)
  })

  test('打开文件夹传会话 id；导出先列工作目录文件，再按单个文件相对路径另存', async () => {
    host()
    const view = renderCard()
    openTab(view, '工作目录')
    fireEvent.click(view.getByRole('button', { name: /打开文件夹/ }))
    await waitFor(() => expect(commandNames()).toContain('project_folder_open'))
    expect(requestOf('project_folder_open')).toEqual({ conversationId: 'chat' })

    fireEvent.click(view.getByRole('button', { name: /导出文件/ }))
    await waitFor(() => expect(commandNames()).toContain('project_files_list'))
    expect(requestOf('project_files_list')).toEqual({ conversationId: 'chat' })
    // 目录不列进导出清单，只有文件。
    await waitFor(() => expect(view.container.querySelectorAll('.fox-contact-side-export-list button')).toHaveLength(1))
    const [entry] = Array.from(view.container.querySelectorAll('.fox-contact-side-export-list button'))
    expect(entry.textContent).toContain('notes/报价.md')
    fireEvent.click(entry)
    await waitFor(() => expect(commandNames()).toContain('companion_export_file'))
    expect(requestOf('companion_export_file')).toEqual({ companionId: 'cmp_one', path: 'notes/报价.md' })
  })

  test('添加文件成功后报出复制的文件数量', async () => {
    host()
    const view = renderCard()
    openTab(view, '工作目录')
    fireEvent.click(view.getByRole('button', { name: /添加文件/ }))
    await waitFor(() => expect(commandNames()).toContain('companion_import_files'))
    await waitFor(() => expect(toasted('success', '已复制 3 个文件')).toBe(true))
  })

  test('工作目录错误可见；状态不是 ready 才给重建入口，Host 拒绝时如实上报', async () => {
    host()
    const view = renderCard(contextValue(), { workspaceState: 'missing', workspaceError: '工作目录被移动或删除' })
    openTab(view, '工作目录')
    expect(view.getByRole('alert').textContent).toContain('工作目录被移动或删除')
    fireEvent.click(view.getByRole('button', { name: /重新创建工作目录/ }))
    await waitFor(() => expect(commandNames()).toContain('companion_workspace_repair'))
    expect(requestOf('companion_workspace_repair')).toEqual({ companionId: 'cmp_one' })

    answer = (command) => { calls.push({ command, request: {} }); throw new Error('repair refused') }
    fireEvent.click(view.getByRole('button', { name: /重新创建工作目录/ }))
    await waitFor(() => expect(toasted('error', 'repair refused')).toBe(true))
  })

  test('就绪的工作目录不显示重建入口', () => {
    host()
    const view = renderCard()
    openTab(view, '工作目录')
    expect(view.queryByRole('button', { name: /重新创建工作目录/ })).toBeNull()
    expect(view.queryByRole('alert')).toBeNull()
  })
})

describe('旧资料页的功能仍然可达（缺口 E）', () => {
  test('顶部设置入口与返回保留当前分页，关闭仍调用原有回调', async () => {
    host()
    let closed = 0
    const view = render(<ContactsContext.Provider value={contextValue()}>
      <ContactSideCard contact={contact()} detail={detail} onClose={() => { closed += 1 }} onResetContext={() => undefined} search={searchStub} />
    </ContactsContext.Provider>)
    await waitFor(() => expect((view.getByLabelText('联系人模型') as HTMLSelectElement).disabled).toBe(false))
    expect(view.getByRole('button', { name: '设置' }).closest('header')).not.toBeNull()
    openTab(view, '工作目录')
    openSettings(view)
    await waitFor(() => expect(view.getByText('专家模板有新版')).toBeDefined())
    expect(view.queryByRole('button', { name: '设置' })).toBeNull()
    expect(view.getByRole('button', { name: '返回详情' }).closest('header')).not.toBeNull()
    fireEvent.click(view.getByRole('button', { name: '返回详情' }))
    expect(view.getByRole('tab', { name: '工作目录' }).getAttribute('aria-selected')).toBe('true')
    expect(view.getByRole('button', { name: '打开文件夹' })).toBeDefined()
    fireEvent.click(view.getByRole('button', { name: '关闭联系人资料卡' }))
    expect(closed).toBe(1)
  })

  test('无边框重置行仍遵守运行中、Host 阻止和不支持的限制', async () => {
    host()
    let resets = 0
    const card = (current: Companion, currentDetail: ConversationDetail) => <ContactsContext.Provider value={contextValue()}>
      <ContactSideCard contact={current} detail={currentDetail} onClose={() => undefined} onResetContext={() => { resets += 1 }} search={searchStub} />
    </ContactsContext.Provider>
    const view = render(card(contact(), detail))
    await waitFor(() => expect((view.getByLabelText('联系人模型') as HTMLSelectElement).disabled).toBe(false))
    fireEvent.click(view.getByRole('button', { name: '重置上下文' }))
    expect(resets).toBe(1)
    view.rerender(card(contact({ activeRunStatus: 'running' }), detail))
    expect((view.getByRole('button', { name: '重置上下文' }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(view.getByRole('button', { name: '重置上下文' }))
    expect(resets).toBe(1)
    view.rerender(card(contact(), { ...detail, contextResetBlockedReason: '请先处理待确认操作' }))
    expect((view.getByRole('button', { name: '重置上下文' }) as HTMLButtonElement).disabled).toBe(true)
    expect(view.getByText('请先处理待确认操作')).toBeDefined()
    view.rerender(card(contact(), { ...detail, contextReset: { supported: false, reason: null, code: null } }))
    expect(view.queryByRole('button', { name: '重置上下文' })).toBeNull()
    expect(view.getByRole('button', { name: '在对话中查找' })).toBeDefined()
  })

  test('设置子视图：头像 / 职责 / 自定义人设可编辑并保存', async () => {
    host()
    const view = renderCard()
    openSettings(view)
    fireEvent.change(view.getByLabelText('职责'), { target: { value: '负责归档' } })
    fireEvent.change(view.getByLabelText('自定义人设'), { target: { value: '只用一句话回答' } })
    fireEvent.change(view.getByLabelText('头像'), { target: { value: '🗂' } })
    fireEvent.submit(view.container.querySelector('form')!)
    await waitFor(() => expect(commandNames()).toContain('companion_update_profile'))
    expect(requestOf('companion_update_profile')).toMatchObject({
      companionId: 'cmp_one', displayName: '资料伙伴', avatarRef: '🗂', duty: '负责归档', personaOverride: '只用一句话回答',
    })
    await waitFor(() => expect(toasted('success', '已保存联系人资料')).toBe(true))
  })

  test('设置子视图：模板变化逐字段可见，采用与暂不更新各走一条命令并刷新状态', async () => {
    host()
    const view = renderCard()
    openSettings(view)
    await waitFor(() => expect(commandNames()).toContain('companion_template_status'))
    expect(view.getByText('专家模板有新版')).toBeDefined()
    expect(view.getByText('当前版本 1.0')).toBeDefined()
    // 字段级差异复用资料页同一块组件（当前 / 新版）。
    expect(view.container.textContent).toContain('当前：旧工作要求')
    expect(view.container.textContent).toContain('新版：新工作要求')
    fireEvent.click(view.getByRole('button', { name: '采用新版' }))
    await waitFor(() => expect(commandNames()).toContain('companion_template_update'))
    expect(requestOf('companion_template_update')).toEqual({ companionId: 'cmp_one', expectedHash: 'hash-2' })
    // 决策之后必须重新读一次状态，卡片显示 Host 决定的结果，而不是停在旧 diff 上。
    await waitFor(() => expect(commandNames().filter((name) => name === 'companion_template_status').length).toBe(2))
    await waitFor(() => expect(view.getByText('正在使用当前模板')).toBeDefined())
    expect(view.queryByText('专家模板有新版')).toBeNull()
    expect(view.queryByText('当前：旧工作要求')).toBeNull()
    view.unmount()

    // 第二张卡要先卸载第一张：RTL 的查询绑定在 document.body 上，两张同级卡片
    // 会同时命中「联系人设置」标题。
    host()
    const second = renderCard()
    openSettings(second)
    await waitFor(() => expect(commandNames()).toContain('companion_template_status'))
    fireEvent.click(second.getByRole('button', { name: '暂不更新' }))
    await waitFor(() => expect(commandNames()).toContain('companion_template_dismiss'))
    expect(requestOf('companion_template_dismiss')).toEqual({ companionId: 'cmp_one', expectedHash: 'hash-2' })
    await waitFor(() => expect(commandNames().filter((name) => name === 'companion_template_status').length).toBe(2))
    expect((second.getByRole('button', { name: '暂不更新' }) as HTMLButtonElement).disabled).toBe(true)
    second.unmount()
  })

  test('设置子视图：项目笔记授权、置顶、归档、删除都在且参数正确', async () => {
    host()
    const view = renderCard()
    openSettings(view)
    await waitFor(() => expect(commandNames()).toContain('companion_grants_list'))
    expect(view.getByText('华东项目')).toBeDefined()
    const checkbox = view.container.querySelector('input[type="checkbox"]') as HTMLInputElement
    expect(checkbox.checked).toBe(true)
    fireEvent.click(checkbox)
    await waitFor(() => expect(commandNames()).toContain('companion_revoke_project'))
    expect(requestOf('companion_revoke_project')).toEqual({ companionId: 'cmp_one', projectId: 'proj_a' })

    fireEvent.click(view.getByRole('button', { name: '取消置顶' }))
    await waitFor(() => expect(commandNames()).toContain('companion_set_pinned'))
    expect(requestOf('companion_set_pinned')).toEqual({ companionId: 'cmp_one', pinned: false })

    fireEvent.click(view.getByRole('button', { name: '归档' }))
    await waitFor(() => expect(commandNames()).toContain('companion_archive'))

    fireEvent.click(view.getByRole('button', { name: '删除…' }))
    await waitFor(() => expect(commandNames()).toContain('companion_delete_preview'))
    await waitFor(() => expect(view.getByText('永久删除')).toBeDefined())
  })

  test('默认联系人没有项目授权与删除入口', () => {
    host()
    const view = renderCard(contextValue(), { isDefault: true, pinnedAt: null })
    openSettings(view)
    expect(view.queryByText('华东项目')).toBeNull()
    expect(view.queryByText('删除…')).toBeNull()
    expect(view.getByRole('button', { name: '置顶' })).toBeDefined()
  })

  test('主卡片保留 hero、三个分页、模型、提醒、查找与共享记忆，并能返回详情', () => {
    host()
    const view = renderCard()
    expect(view.getByText('资料伙伴')).toBeDefined()
    expect(view.getByRole('tab', { name: '详情' })).toBeDefined()
    expect(view.getByRole('tab', { name: '资料库' })).toBeDefined()
    expect(view.getByRole('tab', { name: '工作目录' })).toBeDefined()
    expect(view.getByText('模型')).toBeDefined()
    expect((view.getByLabelText('提醒') as HTMLSelectElement).value).toBe('all')
    expect(view.getByRole('button', { name: '重置上下文' })).toBeDefined()
    // 「在对话中查找」驱动的是共享的那一个搜索实例（缺口 F）。
    fireEvent.click(view.getByRole('button', { name: '在对话中查找' }))
    expect(searchOpens.count).toBe(1)
    fireEvent.click(view.getByRole('button', { name: '共享记忆' }))
    expect(sharedMemory.count).toBe(1)
    fireEvent.click(view.getByRole('button', { name: '收藏' }))
    expect(view.getByRole('heading', { level: 2 }).textContent).toBe('收藏')
    fireEvent.click(view.getByRole('button', { name: '返回详情' }))
    expect(view.getByRole('heading', { level: 2 }).textContent).toBe('联系人资料')
  })

  test('失败统一走 showContactError：Host 报错不被吞掉', async () => {
    host()
    answer = (command) => { calls.push({ command, request: {} }); throw new Error('host said no') }
    const view = renderCard()
    openTab(view, '工作目录')
    fireEvent.click(view.getByRole('button', { name: /打开文件夹/ }))
    await waitFor(() => expect(toasted('error', 'host said no')).toBe(true))
  })
})
