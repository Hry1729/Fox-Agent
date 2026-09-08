import { describe, expect, test } from 'bun:test'
import { notificationIsVisibleInContext, runtimeEventRefreshesNotifications } from '../src/features/notifications'

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
