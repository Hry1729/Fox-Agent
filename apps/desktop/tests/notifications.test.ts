import { describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
const { FOX_IN_APP_NOTIFICATION_EVENT, directToastRecently, notificationIsVisibleInContext, runtimeEventRefreshesNotifications } = await import('../src/features/notifications')
const { notifySendNotStarted } = await import('../src/features/chat/send-failure-notice')

describe('notification refresh routing', () => {
  test('refreshes for run lifecycle transitions', () => {
    for (const eventType of [
      'run.started',
      'run.completed',
      'run.cancelled',
      'run.failed',
      'run.interrupted',
    ]) {
      expect(runtimeEventRefreshesNotifications(eventType)).toBe(true)
    }
  })

  test('ignores high-frequency streaming events', () => {
    for (const eventType of ['message.started', 'message.delta', 'message.completed', undefined]) {
      expect(runtimeEventRefreshesNotifications(eventType)).toBe(false)
    }
  })
})

describe('notification visibility routing', () => {
  const foregroundChat = {
    appVisible: true,
    appFocused: true,
    activeView: 'chat',
    activeConversationId: 'conversation-one',
  }

  test('silences a notification whose result is already visible in the active conversation', () => {
    expect(notificationIsVisibleInContext({ workspaceView: 'chat', entityId: 'conversation-one' }, foregroundChat)).toBe(true)
  })

  test('keeps notifications from background conversations alerting', () => {
    expect(notificationIsVisibleInContext({ workspaceView: 'chat', entityId: 'conversation-two' }, foregroundChat)).toBe(false)
  })

  test('keeps notifications alerting while the app is not focused', () => {
    expect(notificationIsVisibleInContext(
      { workspaceView: 'chat', entityId: 'conversation-one' },
      { ...foregroundChat, appFocused: false },
    )).toBe(false)
  })

  test('does not hide generic notifications that have no navigable context', () => {
    expect(notificationIsVisibleInContext({ workspaceView: null, entityId: null }, foregroundChat)).toBe(false)
  })

  test('matches the active non-chat page and its entity', () => {
    expect(notificationIsVisibleInContext(
      { workspaceView: 'knowledge-detail', entityId: 'knowledge-one' },
      { ...foregroundChat, activeView: 'knowledge-detail', activeEntityId: 'knowledge-one' },
    )).toBe(true)
  })
})

describe('13.37: a refused send shows a toast right away', () => {
  test('every refusal dispatches its own visible toast with the settings entry, and the stored copy is not toasted again', () => {
    const seen: Array<{ id: string; title: string; body: string; workspaceView?: string | null }> = []
    const listener = (event: Event) => seen.push((event as CustomEvent).detail.notification)
    window.addEventListener(FOX_IN_APP_NOTIFICATION_EVENT, listener)
    try {
      notifySendNotStarted('model.connection_unverified', 'raw', { backend: 'deepseek', developerUi: false })
      notifySendNotStarted('model.connection_unverified', 'raw', { backend: 'deepseek', developerUi: false })
      notifySendNotStarted('model.connection_required', 'raw', { backend: 'fox', developerUi: false })
    } finally {
      window.removeEventListener(FOX_IN_APP_NOTIFICATION_EVENT, listener)
    }
    // The same refusal twice is two toasts (B3-2 showed nothing the second time).
    expect(seen).toHaveLength(3)
    expect(new Set(seen.map((item) => item.id)).size).toBe(3)
    expect(seen[0].title).toBe('模型连接需要重新测试')
    expect(seen[0].workspaceView).toBe('model-service')
    expect(seen[2].title).toBe('还没有配置模型服务')
    expect(seen[2].body).toBe('请先到「设置 → 模型服务」添加并测试一个供应商，再发送。')
    expect(directToastRecently('send-not-started:model.connection_unverified')).toBe(true)
    expect(directToastRecently('send-not-started:model.connection_unverified', Date.now() + 60_000)).toBe(false)
    expect(directToastRecently('send-not-started:run.conversation_busy')).toBe(false)
  })
})
