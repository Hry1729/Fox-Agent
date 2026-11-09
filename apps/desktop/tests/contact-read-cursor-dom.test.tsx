import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { act, cleanup, render } from '@testing-library/react'
import type { Companion } from '../src/features/companions/api'
import type { ContactBubbleRecord, ConversationDetail } from '../src/features/conversations/model/types'

const markRead = mock(async (_id: string, _order: number, _at: number) => {})
const actualApi = await import('../src/features/companions/api')
mock.module('../src/features/companions/api', () => ({
  ...actualApi,
  contacts: { ...actualApi.contacts, markRead },
}))
afterAll(() => { mock.module('../src/features/companions/api', () => actualApi) })

const { useReadCursor } = await import('../src/features/companions/context')

type ObserverEntry = { target: Element; isIntersecting: boolean }
let observers: FakeObserver[] = []
class FakeObserver {
  callback: (entries: ObserverEntry[]) => void
  targets = new Set<Element>()
  constructor(callback: (entries: ObserverEntry[]) => void) {
    this.callback = callback
    observers.push(this)
  }
  observe(target: Element) { this.targets.add(target) }
  unobserve(target: Element) { this.targets.delete(target) }
  disconnect() {
    this.targets.clear()
    observers = observers.filter(observer => observer !== this)
  }
}
;(globalThis as { IntersectionObserver?: unknown }).IntersectionObserver = FakeObserver

const bubble = (deliveryOrder: number): ContactBubbleRecord => ({
  id: `b${deliveryOrder}`, conversationId: 'chat', runId: null, messageId: `m${deliveryOrder}`,
  sourceOrdinal: deliveryOrder / 1000, author: 'companion', projection: 'message', segmentStart: null,
  segmentEnd: null, deliveryOrder, deliveredAt: deliveryOrder, content: 'x', fingerprint: 'f',
})

const contact = (patch: Partial<Companion> = {}): Companion => ({
  id: 'contact', conversationId: 'chat', templateAgentId: 'expert', displayName: '伙伴', avatarRef: '📚',
  duty: '', personaOverride: null, workspaceDir: '/tmp/contact', workspaceState: 'ready', workspaceError: null,
  isDefault: false, pinnedAt: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  reminderMode: 'follow_global', lastReadDeliveryOrder: null, manualUnreadFromOrder: null, manualUnreadAt: null,
  readDeliveryOrder: 0, pendingActionCount: 0, unreadCount: 0, activeRunStatus: null, archivedAt: null,
  deletingAt: null, ...patch,
})

const detail = (orders: number[], hasEarlierMessages = false) => ({
  bubbles: orders.map(bubble), hasEarlierMessages,
}) as unknown as ConversationDetail

function Harness({ contact: person, detail: snapshot, visible }: {
  contact: Companion; detail: ConversationDetail; visible: boolean
}) {
  useReadCursor(person, snapshot, visible, async () => {})
  return <div>{(snapshot.bubbles ?? []).map(item =>
    <section key={item.id} data-delivery-order={item.deliveryOrder}>{item.content}</section>)}</div>
}

/** Fire intersection entries on the newest observer, like a real scroll would. */
const intersect = (container: HTMLElement, orders: number[], isIntersecting: boolean) => act(() => {
  const observer = observers.at(-1)
  if (!observer) throw new Error('no active observer')
  const entries = orders.map(order => {
    const target = container.querySelector(`[data-delivery-order="${order}"]`)
    if (!target) throw new Error(`missing section ${order}`)
    return { target, isIntersecting }
  })
  observer.callback(entries)
})

afterEach(() => {
  cleanup()
  observers = []
  markRead.mockClear()
})

describe('K6 阅读游标视口进入', () => {
  test('会话内手动未读不被立即抵消，重开同一聊天重新进入视口后才清除', async () => {
    document.hasFocus = () => true
    Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true })
    const snapshot = detail([1000, 2000])
    const view = render(<Harness contact={contact()} detail={snapshot} visible />)
    await intersect(view.container, [1000, 2000], true)
    expect(markRead.mock.calls.length).toBe(1)
    expect(markRead.mock.calls[0][0]).toBe('contact')
    expect(markRead.mock.calls[0][1]).toBe(2000)

    // Mark unread while the bubbles stay on screen: even re-fired intersection
    // entries must not immediately clear the override within the same visit.
    const unread = contact({ readDeliveryOrder: 2000, manualUnreadFromOrder: 2000, manualUnreadAt: 5 })
    view.rerender(<Harness contact={unread} detail={snapshot} visible />)
    await intersect(view.container, [1000, 2000], true)
    expect(markRead.mock.calls.length).toBe(1)

    // Leaving and reopening the same chat is a fresh viewport entry: the still
    // visible bubbles count as seen again and re-reading clears the override.
    view.rerender(<Harness contact={unread} detail={snapshot} visible={false} />)
    view.rerender(<Harness contact={unread} detail={snapshot} visible />)
    await intersect(view.container, [1000, 2000], true)
    expect(markRead.mock.calls.length).toBe(2)
    expect(markRead.mock.calls[1][1]).toBe(2000)
  })

  test('有更早分页时首条标未读，重开并实际看到首条后才上报清除', async () => {
    document.hasFocus = () => true
    Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true })
    const snapshot = detail([5000, 6000, 7000], true)
    const person = contact({ readDeliveryOrder: 7000 })
    const view = render(<Harness contact={person} detail={snapshot} visible />)
    await intersect(view.container, [5000, 6000, 7000], true)
    expect(markRead.mock.calls.length).toBe(0)

    const unread = contact({ readDeliveryOrder: 7000, manualUnreadFromOrder: 5000, manualUnreadAt: 5 })
    view.rerender(<Harness contact={unread} detail={snapshot} visible />)
    await intersect(view.container, [5000, 6000, 7000], true)
    expect(markRead.mock.calls.length).toBe(0)

    view.rerender(<Harness contact={unread} detail={snapshot} visible={false} />)
    view.rerender(<Harness contact={unread} detail={snapshot} visible />)
    expect(markRead.mock.calls.length).toBe(0)
    await intersect(view.container, [6000, 7000], true)
    expect(markRead.mock.calls.length).toBe(0)

    await intersect(view.container, [5000], true)
    expect(markRead.mock.calls.length).toBe(1)
    expect(markRead.mock.calls[0]).toEqual(['contact', 7000, expect.any(Number)])
  })

  test('积压超过一页时看到窗口首条不清空更早未读，加载齐后才推进', async () => {
    document.hasFocus = () => true
    Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true })
    const person = contact({ readDeliveryOrder: 1000 })
    const windowed = detail([5000], true)
    const view = render(<Harness contact={person} detail={windowed} visible />)
    await intersect(view.container, [5000], true)
    expect(markRead.mock.calls.length).toBe(0)

    const complete = detail([2000, 5000], false)
    view.rerender(<Harness contact={person} detail={complete} visible />)
    await intersect(view.container, [2000, 5000], true)
    expect(markRead.mock.calls.length).toBe(1)
    expect(markRead.mock.calls[0][1]).toBe(5000)
  })
})