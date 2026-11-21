import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/react'
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import { CompanionSidebar, WorkCompanionSidebar } from '../src/features/companions/sidebar'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import type { Companion } from '../src/features/companions/api'

const companion = (id: string, displayName: string, patch: Partial<Companion> = {}): Companion => ({
  id, conversationId: `chat-${id}`, templateAgentId: null, displayName, avatarRef: '🦊', duty: `${displayName}的职责`,
  personaOverride: null, workspaceDir: `/tmp/${id}`, workspaceState: 'ready', workspaceError: null,
  isDefault: false, pinnedAt: null, workShortcutOrder: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  reminderMode: 'follow_global', lastReadDeliveryOrder: null, manualUnreadFromOrder: null, manualUnreadAt: null,
  readDeliveryOrder: 0, pendingActionCount: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
  ...patch,
})
const fox = companion('default', '小狐', { isDefault: true })
const calls: Array<{ command: string; args: Record<string, unknown> }> = []
let records: Companion[] = []

beforeEach(() => {
  calls.length = 0
  mockIPC((command, args) => {
    const payload = ((args as { request?: Record<string, unknown> }).request ?? args) as Record<string, unknown>
    calls.push({ command, args: payload })
    if (command === 'agents_list') return { ok:true, data:[
      { id:'writer', name:'汇报材料专家', description:'整理周报和汇报材料', available:true, runtimeType:'pi', agentKind:'expert', resources:{knowledges:[],tools:[],mcps:[],skills:[]}, packageManifest:{} },
      { id:'coder', name:'代码审查专家', description:'检查代码', available:true, runtimeType:'pi', agentKind:'expert', resources:{knowledges:[],tools:[],mcps:[],skills:[]}, packageManifest:{} },
    ] }
    if (command === 'companion_add_from_expert') return { ok:true, data:companion('from-expert', '汇报伙伴') }
    if (command === 'companion_next_default_name') return { ok: true, data: 'Fox 2' }
    if (command === 'companion_create_blank') return payload.displayName === '已有伙伴'
      ? { ok: false, error: { code: 'companion.name_conflict', message: '已有伙伴使用这个名字', retryable: false } }
      : { ok: true, data: companion('new', String(payload.displayName), { avatarRef: String(payload.avatarRef) }) }
    if (command === 'companion_open') return { ok: true, data: records.find(item => item.id === payload.companionId) }
    if (['companion_set_work_shortcuts', 'companion_restore', 'companion_archive', 'companion_set_pinned', 'companion_set_pinned_order', 'companion_update_profile',
      'companion_mark_read', 'companion_mark_unread'].includes(command)) return { ok: true, data: null }
    throw new Error(`Unexpected command: ${command}`)
  })
})
afterEach(() => { cleanup(); clearMocks() })

function context(items: Companion[], opened: Companion[], refreshed: { count: number }): ContactsContextValue {
  return {
    current: items[0], privateChat: true, list: { companions: items, pendingDeletions: [] }, projects: [], model: 'test', permission: 'ask', error: '',
    refresh: async () => { refreshed.count += 1 }, refreshConversation: async () => {}, open: item => { opened.push(item) }, add: () => {}, showProfile: () => {},
    leave: () => {}, setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
  }
}

function renderSidebar(items: Companion[], kind: 'work' | 'companions', onOpenCompanions: (id?: string) => void = () => {}) {
  records = items
  const opened: Companion[] = []
  const refreshed = { count: 0 }
  const view = render(<ContactsContext.Provider value={context(items, opened, refreshed)}>
    {kind === 'work' ? <WorkCompanionSidebar onOpenCompanions={onOpenCompanions} /> : <CompanionSidebar />}
  </ContactsContext.Provider>)
  return { view, opened, refreshed }
}

const openMenu = async (trigger: HTMLElement) => {
  fireEvent.keyDown(trigger, { key: 'Enter' })
  return await waitFor(() => {
    const menu = document.querySelector('[role="menu"]')
    if (!menu) throw new Error('menu did not open')
    return menu as HTMLElement
  })
}
const closeMenu = async () => {
  const menu = document.querySelector('[role="menu"]')
  if (menu) fireEvent.keyDown(menu, { key: 'Escape' })
  await waitFor(() => expect(document.querySelector('[role="menu"]')).toBeNull())
}

describe('伙伴页面侧栏', () => {
  test('工作页小狐第一、常用按持久顺序显示，管理浮层支持排序和添加', async () => {
    const first = companion('first', '周报伙伴', { workShortcutOrder: 2 })
    const second = companion('second', '资料伙伴', { workShortcutOrder: 1 })
    const other = companion('other', '表格伙伴')
    const { view, refreshed } = renderSidebar([fox, first, second, other], 'work')
    expect([...view.container.querySelectorAll('.fox-contact-row-name')].map(node => node.textContent)).toEqual(['小狐', '资料伙伴', '周报伙伴'])
    expect(view.queryByRole('button', { name: '新建伙伴' })).toBeNull()

    fireEvent.click(view.getByRole('button', { name: '管理常用伙伴' }))
    const popover = await waitFor(() => {
      const node = document.querySelector('.fox-work-shortcut-manager')
      if (!node) throw new Error('manager did not open')
      return node as HTMLElement
    })
    expect(within(popover).getByText('已选 2 / 3')).toBeDefined()
    expect(within(popover).getByText('默认')).toBeDefined()
    expect(within(popover).queryByRole('button', { name: '从常用中移除 小狐' })).toBeNull()
    fireEvent.keyDown(within(popover).getByRole('button', { name: '调整 资料伙伴 的顺序' }), { key: 'ArrowDown' })
    await waitFor(() => expect(calls).toContainEqual({ command: 'companion_set_work_shortcuts', args: { ids: ['first', 'second'] } }))
    fireEvent.click(within(popover).getByRole('button', { name: '添加 表格伙伴' }))
    await waitFor(() => expect(calls).toContainEqual({ command: 'companion_set_work_shortcuts', args: { ids: ['second', 'first', 'other'] } }))
    expect(refreshed.count).toBe(2)
  })

  test('工作页菜单只提供查看和常用移除，并能跳到指定伙伴页', async () => {
    const first = companion('first', '周报伙伴', { workShortcutOrder: 1 })
    const destinations: Array<string | undefined> = []
    const { view } = renderSidebar([fox, first], 'work', id => destinations.push(id))
    const defaultMenu = await openMenu(view.getByRole('button', { name: '更多 小狐 操作' }))
    expect(within(defaultMenu).queryByText('从常用中移除')).toBeNull()
    fireEvent.click(within(defaultMenu).getByText('在伙伴页面中查看'))
    await waitFor(() => expect(destinations).toEqual(['default']))
    await closeMenu()
    const menu = await openMenu(view.getByRole('button', { name: '更多 周报伙伴 操作' }))
    expect(within(menu).getByText('从常用中移除')).toBeDefined()
    expect(within(menu).queryByText('置顶')).toBeNull()
    expect(within(menu).queryByText('归档')).toBeNull()
    expect(within(menu).queryByText('删除…')).toBeNull()
  })

  test('大头像网格不重复列表，右键只开菜单，小狐受保护', async () => {
    const pinned = companion('pinned', '置顶伙伴', { pinnedAt: 10, activeRunStatus: 'running' })
    const ordinary = companion('ordinary', '普通伙伴', { unreadCount: 2 })
    const archived = companion('archived', '归档伙伴', { archivedAt: 20 })
    const { view, opened } = renderSidebar([fox, pinned, ordinary, archived], 'companions')
    expect(view.container.querySelectorAll('.fox-companion-tile')).toHaveLength(2)
    expect(view.container.querySelector('.fox-companion-tile-link')?.textContent).toContain('小狐')
    expect(view.queryByText('全部伙伴')).toBeNull()
    expect(view.queryByPlaceholderText('搜索伙伴')).toBeNull()
    expect(view.getByLabelText('2 条未读').textContent).toBe('')

    fireEvent.contextMenu(view.getByRole('button', { name: '小狐' }))
    const defaultMenu = await waitFor(() => document.querySelector('[role="menu"]') as HTMLElement)
    expect(opened).toHaveLength(0)
    expect(calls.filter(call => call.command === 'companion_open')).toHaveLength(0)
    expect(within(defaultMenu).queryByText(/置顶/)).toBeNull()
    expect(within(defaultMenu).queryByText('归档')).toBeNull()
    expect(within(defaultMenu).queryByText('删除…')).toBeNull()
    await closeMenu()

    fireEvent.contextMenu(view.getByRole('button', { name: '置顶伙伴，正在工作' }))
    const pinnedMenu = await waitFor(() => document.querySelector('[role="menu"]') as HTMLElement)
    expect(within(pinnedMenu).getByText('取消置顶')).toBeDefined()
    expect(opened).toHaveLength(0)
    await closeMenu()
    fireEvent.click(view.getByRole('button', { name: '置顶伙伴，正在工作' }))
    await waitFor(() => expect(opened.map(item => item.id)).toEqual(['pinned']))
  })

  test('头像主点击不打开菜单，菜单键可打开菜单并进入重命名', async () => {
    const pinned = companion('pinned', '置顶伙伴', { pinnedAt: 20 })
    const { view, opened } = renderSidebar([pinned, fox], 'companions')
    expect(view.container.querySelector('.fox-companion-tile-link')?.textContent).toContain('小狐')
    const tile = view.getByRole('button', { name: '置顶伙伴' })
    fireEvent.pointerDown(tile, { button: 0, pointerType: 'mouse' })
    fireEvent.click(tile)
    await waitFor(() => expect(opened).toHaveLength(1))
    expect(document.querySelector('[role="menu"]')).toBeNull()
    fireEvent.keyDown(tile, { key: 'F10', shiftKey: true })
    const menu = await waitFor(() => {
      const node = document.querySelector('[role="menu"]')
      if (!node) throw new Error('menu did not open')
      return node as HTMLElement
    })
    fireEvent.click(within(menu).getByText('重命名'))
    await waitFor(() => expect(view.getByRole('textbox', { name: '重命名 置顶伙伴' })).toBeDefined())
    fireEvent.keyDown(view.getByRole('textbox', { name: '重命名 置顶伙伴' }), { key: 'Escape' })
    expect(view.queryByRole('textbox')).toBeNull()
  })

  test('归档入口切换独立列表，支持只读打开、恢复和删除入口', async () => {
    const archived = companion('archived', '归档伙伴', { archivedAt: 20 })
    const { view, opened } = renderSidebar([fox, archived], 'companions')
    const toggle = view.getByRole('button', { name: /已归档/ })
    fireEvent.click(toggle)
    expect(view.getByRole('region', { name: '已归档伙伴' })).toBeDefined()
    expect(view.queryByRole('button', { name: '小狐' })).toBeNull()
    expect(view.getByText(/归档于/)).toBeDefined()
    fireEvent.click(view.getByRole('button', { name: '归档伙伴' }))
    expect(opened.map(item => item.conversationId)).toEqual(['chat-archived'])
    expect(calls.some(call => call.command === 'companion_open')).toBe(false)
    const menu = await openMenu(view.getByRole('button', { name: '更多 归档伙伴 操作' }))
    expect(within(menu).getAllByRole('menuitem').map(node => node.textContent)).toEqual(['删除…'])
    await closeMenu()
    fireEvent.click(view.getByRole('button', { name: '恢复 归档伙伴' }))
    await waitFor(() => expect(calls.some(call => call.command === 'companion_restore' && JSON.stringify(call.args) === JSON.stringify({ companionId: 'archived' }))).toBe(true))
    fireEvent.click(view.getByRole('button', { name: '返回伙伴列表' }))
    expect(view.getByRole('button', { name: '小狐' })).toBeDefined()
  })

  test('行右键打开独立的菜单，行内重命名保留预览并支持失焦关闭和 Escape 取消', async () => {
    const reader = companion('reader', '阅读伙伴', { latestMessagePreview: '已经整理完成', lastMessageAt: Date.now() })
    const { view, opened } = renderSidebar([fox, reader], 'companions')
    const openContext = async () => {
      fireEvent.contextMenu(view.getByRole('button', { name: '阅读伙伴' }), { clientX: 85, clientY: 240 })
      return await waitFor(() => {
        const menu = document.querySelector('[data-slot="context-menu-content"]')
        if (!menu) throw new Error('context menu did not open')
        return menu as HTMLElement
      })
    }
    fireEvent.click(within(await openContext()).getByText('重命名'))
    const input = await view.findByRole('textbox', { name: '重命名 阅读伙伴' }) as HTMLInputElement
    expect(input.selectionEnd).toBe('阅读伙伴'.length)
    expect(view.getByText('已经整理完成')).toBeDefined()
    expect(input.closest('.fox-companion-row')?.querySelector('time')).toBeDefined()
    fireEvent.change(input, { target: { value: '文档伙伴' } })
    fireEvent.blur(input)
    await waitFor(() => expect(calls).toContainEqual({ command: 'companion_update_profile', args: { companionId: 'reader', displayName: '文档伙伴' } }))
    expect(view.queryByRole('textbox')).toBeNull()
    fireEvent.click(within(await openContext()).getByText('重命名'))
    const cancel = await view.findByRole('textbox', { name: '重命名 阅读伙伴' })
    fireEvent.change(cancel, { target: { value: '不应保存' } })
    fireEvent.keyDown(cancel, { key: 'Escape' })
    expect(view.queryByRole('textbox')).toBeNull()
    expect(calls.filter(item => item.command === 'companion_update_profile')).toHaveLength(1)
    expect(opened).toHaveLength(0)
  })

  test('确认后一次提交自定义头像和名称，重名时保留选择', async () => {
    const { view, opened } = renderSidebar([fox], 'companions')
    const menu = await openMenu(view.getByRole('button', { name: '新建伙伴' }))
    fireEvent.click(within(menu).getByRole('menuitem', { name: '新建伙伴' }))
    const dialog = within(await view.findByRole('dialog', { name: '新建伙伴' }))
    const name = dialog.getByRole('textbox', { name: '伙伴名称' }) as HTMLInputElement
    await waitFor(() => expect(name.value).toBe('Fox 2'))
    expect(calls.some(item => item.command === 'companion_create_blank')).toBe(false)
    const avatar = dialog.getByRole('radio', { name: '头像 2' }) as HTMLInputElement
    const avatarRef = avatar.parentElement!.querySelector('img')!.getAttribute('src')
    fireEvent.click(avatar)
    fireEvent.change(name, { target: { value: '已有伙伴' } })
    fireEvent.click(dialog.getByRole('button', { name: '创建伙伴' }))
    expect((await dialog.findByRole('alert')).textContent).toBe('已有伙伴使用这个名字')
    expect(name.value).toBe('已有伙伴')
    expect(avatar.checked).toBe(true)
    fireEvent.change(name, { target: { value: '归纳伙伴' } })
    fireEvent.click(dialog.getByRole('button', { name: '创建伙伴' }))
    await waitFor(() => expect(opened.map(item => item.displayName)).toEqual(['归纳伙伴']))
    expect(calls.filter(item => item.command === 'companion_create_blank').at(-1)?.args).toEqual({ displayName: '归纳伙伴', avatarRef })
    expect(view.queryByRole('dialog')).toBeNull()
  })

  test('从专家创建支持搜索职责、选择并提交专家身份', async () => {
    const { view, opened } = renderSidebar([fox], 'companions')
    const menu = await openMenu(view.getByRole('button', { name: '新建伙伴' }))
    expect(menu.querySelector('small')).toBeNull()
    fireEvent.click(within(menu).getByRole('menuitem', { name: '从专家创建' }))
    const dialog = within(await view.findByRole('dialog', { name: '从专家创建' }))
    await dialog.findByRole('button', { name: '选择汇报材料专家' })
    expect((dialog.getByRole('button', { name: '确认创建' }) as HTMLButtonElement).disabled).toBe(true)
    fireEvent.change(dialog.getByRole('textbox', { name: '搜索专家' }), { target: { value: '周报' } })
    expect(dialog.queryByRole('button', { name: '选择代码审查专家' })).toBeNull()
    fireEvent.click(dialog.getByRole('button', { name: '选择汇报材料专家' }))
    fireEvent.click(dialog.getByRole('button', { name: '确认创建' }))
    await waitFor(() => expect(opened.map(item => item.id)).toEqual(['from-expert']))
    expect(calls).toContainEqual({ command:'companion_add_from_expert', args:{ expertId:'writer' } })
  })

  test('置顶网格按持久顺序显示，可用键盘调整顺序且不移动小狐', async () => {
    const first = companion('first', '周报伙伴', { pinnedAt: 1, pinnedOrder: 1 })
    const second = companion('second', '资料伙伴', { pinnedAt: 20, pinnedOrder: 2 })
    const { view } = renderSidebar([second, fox, first], 'companions')
    expect([...view.container.querySelectorAll('.fox-companion-tile-link')].map(node => node.textContent)).toEqual(['🦊小狐', '🦊周报伙伴', '🦊资料伙伴'])
    expect(view.queryByRole('button', { name: '调整 小狐 的置顶顺序' })).toBeNull()
    fireEvent.keyDown(view.getByRole('button', { name: '调整 周报伙伴 的置顶顺序' }), { key: 'ArrowRight' })
    await waitFor(() => expect(calls).toContainEqual({ command: 'companion_set_pinned_order', args: { ids: ['second', 'first'] } }))
    expect(calls.some(call => call.command === 'companion_set_work_shortcuts')).toBe(false)
  })

  test('收起后仍有新建入口和全部伙伴，保留选中状态', async () => {
    const opened: Companion[] = []
    const items = [fox, ...Array.from({ length: 8 }, (_, index) => companion(`p${index}`, `伙伴 ${index}`))]
    records = items
    const view = render(<ContactsContext.Provider value={context(items, opened, { count: 0 })}>
      <CompanionSidebar collapsed activeId="chat-p7" />
    </ContactsContext.Provider>)
    const menu = await openMenu(view.getByRole('button', { name: '新建伙伴' }))
    expect(within(menu).getByRole('menuitem', { name: '从专家创建' })).toBeDefined()
    await closeMenu()
    expect(view.container.querySelector('.fox-collapsed-companions button')?.getAttribute('aria-label')).toBe('小狐')
    const last = view.getByRole('button', { name: '伙伴 7' })
    expect(last.getAttribute('aria-current')).toBe('page')
    fireEvent.click(last)
    await waitFor(() => expect(opened.map(item => item.id)).toEqual(['p7']))
  })

  test('伙伴行显示最近时间但不泄露失败详情，行菜单标记已读或未读', async () => {
    const lastMessageAt = Date.now()
    const failed = companion('failed', '资料伙伴', {
      lastMessageAt, latestFailureSummary: '模型服务超时', unreadCount: 2, latestDeliveryOrder: 2000,
    })
    const read = companion('read', '审校伙伴', { latestBubbleId: 'read-bubble', workShortcutOrder: 1 })
    const { view } = renderSidebar([fox, failed, read], 'companions')
    expect(view.container.querySelector('time')?.getAttribute('datetime')).toBe(new Date(lastMessageAt).toISOString())
    expect(view.queryByText(/模型服务超时/)).toBeNull()
    expect(view.getByRole('status', { name: '运行异常' })).toBeDefined()
    const unreadMenu = await openMenu(view.getByRole('button', { name: '更多 资料伙伴 操作' }))
    fireEvent.click(within(unreadMenu).getByText('标记为已读'))
    await waitFor(() => expect(calls).toContainEqual({ command: 'companion_mark_read', args: {
      companionId: 'failed', deliveryOrder: 2000, observedAt: expect.any(Number),
    } }))
    await closeMenu()
    const readMenu = await openMenu(view.getByRole('button', { name: '更多 审校伙伴 操作' }))
    expect(within(readMenu).getByRole('menuitemcheckbox', { name: '显示在工作页' }).getAttribute('aria-checked')).toBe('true')
    fireEvent.click(within(readMenu).getByText('标记为未读'))
    await waitFor(() => expect(calls).toContainEqual({ command: 'companion_mark_unread', args: { companionId: 'read', bubbleId: 'read-bubble' } }))
  })
})
