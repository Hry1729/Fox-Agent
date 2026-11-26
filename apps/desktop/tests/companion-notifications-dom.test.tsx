import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { act, cleanup, fireEvent, render, waitFor, within } from '@testing-library/react'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import type { Companion } from '../src/features/companions/api'
import type { AppNotificationRecord } from '../src/features/conversations/model/types'
import type { NotificationBridge } from '../src/features/notifications/notification-center'

const actualDesktopClient = await import('../src/features/conversations/api/desktop-client')
const realDesktopClient = { ...actualDesktopClient }
const notice: AppNotificationRecord = {
  id: 'notice-1', mergeKey: 'companion:c1:message', kind: 'completed', severity: 'normal',
  title: '伙伴完成了整理', body: '文件已经整理好。', sourceType: 'companion', sourceId: 'c1',
  workspaceView: 'companions', entityId: 'c1', progress: null, status: 'completed', action: {},
  readAt: 1700000000000, createdAt: 1700000000000, updatedAt: 1700000000000,
}
let notices = [notice]
mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...realDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...realDesktopClient.desktopClient,
    listAppNotifications: async () => notices,
    getNotificationPreferences: async () => ({ systemPopup: false, sound: false, soundId: 'soft', badge: true, quietProgress: false, updatedAt: 0 }),
    setAppNotificationRead: async () => {},
  },
}))
const { NotificationCenter } = await import('../src/features/notifications/notification-center-implementation')

const contact: Companion = {
  id: 'c1', conversationId: 'chat-c1', templateAgentId: null, displayName: '资料伙伴', avatarRef: '📚', duty: '整理资料',
  personaOverride: null, workspaceDir: '/tmp/c1', workspaceState: 'ready', workspaceError: null,
  isDefault: false, pinnedAt: null, workShortcutOrder: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  reminderMode: 'follow_global', lastReadDeliveryOrder: null, manualUnreadFromOrder: null, manualUnreadAt: null,
  readDeliveryOrder: 0, pendingActionCount: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
}
const context = {
  current: contact, privateChat: true, list: { companions: [contact], pendingDeletions: [] }, projects: [], model: 'test', permission: 'ask', error: '',
  refresh: async () => {}, refreshConversation: async () => {}, open: () => {}, add: () => {}, showProfile: () => {},
  leave: () => {}, setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
} as ContactsContextValue

afterEach(() => { cleanup(); notices = [notice] })
afterAll(() => { mock.module('../src/features/conversations/api/desktop-client', () => realDesktopClient) })

describe('伙伴通知', () => {
  test('协作审批在待处理通知中，打开接收方的协作对话', async () => {
    const approval: AppNotificationRecord = {
      ...notice, id: 'coordination-approval', kind: 'approval', status: 'pending',
      title: '需要确认协作操作', action: { coordinationConversationId: 'coordination-1' },
    }
    notices = [approval, notice]
    const bridge: NotificationBridge = {
      initial: Promise.resolve([approval]),
      connect: () => () => {},
    }
    const navigations: unknown[][] = []
    const view = render(<ContactsContext.Provider value={context}>
      <NotificationCenter bridge={bridge} open showTrigger={false} activeView="chat"
        onNavigate={(...args) => navigations.push(args)} onOpenChange={() => {}} />
    </ContactsContext.Provider>)
    expect(await view.findByText(/需要确认协作操作/)).toBeDefined()
    fireEvent.click(view.getByRole('button', { name: '需处理 1' }))
    expect(view.queryByText(notice.body)).toBeNull()
    fireEvent.click(await view.findByRole('button', { name: '打开协作对话' }))
    await waitFor(() => expect(navigations).toEqual([['companions', undefined, { conversationId: 'coordination-1' }]]))
  })

  test('消息正文在主内容容器展开，关闭按钮触发返回', async () => {
    const long = { ...notice, body: '这是任务的完整说明。'.repeat(8) }
    notices = [long]
    const host = document.createElement('main')
    document.body.append(host)
    const openChanges: boolean[] = []
    try {
      const view = render(<NotificationCenter open showTrigger={false} activeView="chat" panelHost={host}
        bridge={{ initial: Promise.resolve([long]), connect: () => () => {} }}
        onNavigate={() => {}} onOpenChange={value => openChanges.push(value)} />)
      const page = within(host)
      const expand = await page.findByRole('button', { name: '展开' })
      expect(view.container.querySelector('.fox-notification-page')).toBeNull()
      fireEvent.click(expand)
      expect(page.getByRole('button', { name: '收起' }).getAttribute('aria-expanded')).toBe('true')
      fireEvent.keyDown(page.getByRole('button', { name: '关闭消息中心' }), { key: 'Escape' })
      expect(openChanges).toEqual([false])
    } finally { host.remove() }
  })

  test('通知中心与 Toast 使用伙伴头像，打开通知进入指定私聊', async () => {
    let handlers: Parameters<NotificationBridge['connect']>[0] | undefined
    const bridge: NotificationBridge = {
      initial: Promise.resolve([notice]),
      connect: next => { handlers = next; return () => { handlers = undefined } },
    }
    const navigations: unknown[][] = []
    const openChanges: boolean[] = []
    const view = render(<ContactsContext.Provider value={context}>
      <NotificationCenter
        bridge={bridge} open showTrigger={false} activeView="chat"
        onNavigate={(...args) => navigations.push(args)} onOpenChange={open => openChanges.push(open)}
      />
    </ContactsContext.Provider>)

    await waitFor(() => expect(view.getByText('资料伙伴')).toBeDefined())
    expect(view.getByText('伙伴完成了整理 · 文件已经整理好。')).toBeDefined()
    expect(view.container.querySelector('.fox-notification-avatar.is-companion .fox-contact-avatar')?.textContent).toBe('📚')
    fireEvent.click(await view.findByRole('button', { name: '打开私聊' }))
    await waitFor(() => expect(navigations).toEqual([['companions', 'c1']]))
    fireEvent.click(view.getByRole('button', { name: '关闭消息中心' }))
    expect(openChanges).toEqual([false])

    act(() => {
      handlers?.preview({ notification: { ...notice, id: 'notice-toast', readAt: null }, soundId: 'soft' })
    })
    const toastRegion = await waitFor(() => {
      const region = document.querySelector('section[aria-label^="通知更新"]') as HTMLElement | null
      if (!region) throw new Error('toast region not ready')
      if (!within(region).queryByText('伙伴完成了整理')) throw new Error('toast not ready')
      return region
    })
    expect(toastRegion.querySelector('.fox-contact-avatar')?.textContent).toBe('📚')
  })
})
