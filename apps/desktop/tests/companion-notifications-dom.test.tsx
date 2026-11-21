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
    notices = [approval]
    const bridge: NotificationBridge = {
      initial: Promise.resolve([approval]),
      connect: () => () => {},
    }
    const navigations: unknown[][] = []
    const view = render(<ContactsContext.Provider value={context}>
      <NotificationCenter bridge={bridge} open showTrigger={false} activeView="chat"
        onNavigate={(...args) => navigations.push(args)} onOpenChange={() => {}} />
    </ContactsContext.Provider>)
    expect(await view.findByText(approval.title)).toBeDefined()
    fireEvent.click(view.container.querySelector('.fox-notification-item-main')!)
    fireEvent.click(await view.findByRole('button', { name: '打开协作对话' }))
    await waitFor(() => expect(navigations).toEqual([['companions', undefined, { conversationId: 'coordination-1' }]]))
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
    expect(view.getByText('文件已经整理好。')).toBeDefined()
    expect(view.container.querySelector('.fox-notification-source-icon.is-companion .fox-contact-avatar')?.textContent).toBe('📚')
    fireEvent.click(view.container.querySelector('.fox-notification-item-main')!)
    fireEvent.click(await view.findByRole('button', { name: '打开私聊' }))
    await waitFor(() => expect(navigations).toEqual([['companions', 'c1']]))
    expect(openChanges).toContain(false)

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
