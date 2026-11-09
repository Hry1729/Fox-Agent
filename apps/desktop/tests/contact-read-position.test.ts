import { describe, expect, test } from 'bun:test'
import {
  companionBubbleOrders,
  readWalkBase,
  reportableReadOrder,
  shouldReportRead,
} from '../src/features/companions/read-position'
import type { ContactBubbleRecord } from '../src/features/conversations/model/types'

const bubble = (deliveryOrder: number, author: 'user' | 'companion'): ContactBubbleRecord => ({
  id: `b${deliveryOrder}`, conversationId: 'c', runId: null, messageId: `m${deliveryOrder}`,
  sourceOrdinal: deliveryOrder / 1000, author, projection: 'message', segmentStart: null,
  segmentEnd: null, deliveryOrder, deliveredAt: deliveryOrder, content: 'x', fingerprint: 'f',
})

describe('K6 bubble read position', () => {
  test('only companion bubbles count, in delivery order', () => {
    expect(companionBubbleOrders([bubble(3000, 'companion'), bubble(1000, 'user'), bubble(2000, 'companion')]))
      .toEqual([2000, 3000])
    expect(companionBubbleOrders(undefined)).toEqual([])
  })

  test('the seen-walk never jumps an unseen unread interval', () => {
    const orders = [1000, 2000, 3000]
    // 2000 was not seen, so seeing 3000 must not advance past it.
    expect(reportableReadOrder(orders, 0, new Set([1000, 3000]))).toBe(1000)
    expect(reportableReadOrder(orders, 0, new Set([1000, 2000, 3000]))).toBe(3000)
  })

  test('loading messages or focusing without seeing them does not advance', () => {
    const orders = [1000, 2000, 3000]
    const target = reportableReadOrder(orders, 0, new Set())
    expect(target).toBe(0)
    expect(shouldReportRead(target, 0, null, -1)).toBe(false)
  })

  test('a manual override pulls the walk back so re-reading can clear it', () => {
    const orders = [1000, 2000, 3000]
    const base = readWalkBase(3000, 2000)
    expect(base).toBe(1999)
    // Re-reading from the override start advances through it again.
    expect(reportableReadOrder(orders, base, new Set([2000, 3000]), false, 2000)).toBe(3000)
    // Without an override the walk starts at the persisted cursor.
    expect(readWalkBase(3000, null)).toBe(3000)
  })

  test('reporting advances past the cursor or covers an active override', () => {
    expect(shouldReportRead(2000, 1000, null, 1000)).toBe(true)
    // Already reported: no redundant update.
    expect(shouldReportRead(2000, 1000, null, 2000)).toBe(false)
    // Below the cursor but covering the override start still clears it.
    expect(shouldReportRead(2000, 3000, 2000, -1)).toBe(true)
    // Below the cursor with no override is not worth reporting.
    expect(shouldReportRead(2000, 3000, null, -1)).toBe(false)
  })

  test('backlog beyond one page: seeing the window head never clears earlier unreads', () => {
    // Cursor at 1000; the loaded window only holds 5000..7000 and the unread
    // gap 2000..4000 sits in unloaded history.
    const window = [5000, 6000, 7000]
    const seen = new Set([5000, 6000, 7000])
    const target = reportableReadOrder(window, 1000, seen, true)
    expect(target).toBe(1000)
    expect(shouldReportRead(target, 1000, null, -1)).toBe(false)
    // Loading the earlier pages lifts the barrier once the walk is contiguous.
    const full = [2000, 3000, 4000, ...window]
    expect(reportableReadOrder(full, 1000, new Set(full), false)).toBe(7000)
    // Still-unseen earlier bubbles keep blocking even with the pages loaded.
    expect(reportableReadOrder(full, 1000, seen, false)).toBe(1000)
    // A window reaching down to the cursor is safe even with older history.
    expect(reportableReadOrder([1000, ...window], 1000, seen, true)).toBe(7000)
    // A manual override inside the loaded window still clears by re-reading.
    const base = readWalkBase(7000, 6000)
    expect(reportableReadOrder(window, base, new Set([6000, 7000]), true, 6000)).toBe(7000)
  })

  test.each([
    {
      name: 're-reading an override at the window head clears it',
      readOrder: 7000, overrideFrom: 5000, seen: [5000, 6000, 7000],
      expectedTarget: 7000, expectedReport: true,
    },
    {
      name: 'an unseen override at the window head blocks reporting',
      readOrder: 7000, overrideFrom: 5000, seen: [6000, 7000],
      expectedTarget: 4999, expectedReport: false,
    },
    {
      name: 'a window-head override never skips an unseen middle bubble',
      readOrder: 7000, overrideFrom: 5000, seen: [5000, 7000],
      expectedTarget: 5000, expectedReport: true,
    },
    {
      name: 'an unknown gap before the override still blocks reporting',
      readOrder: 1000, overrideFrom: 5000, seen: [5000, 6000, 7000],
      expectedTarget: 1000, expectedReport: false,
    },
    {
      name: 'an override in unloaded history still blocks reporting',
      readOrder: 7000, overrideFrom: 4000, seen: [5000, 6000, 7000],
      expectedTarget: 3999, expectedReport: false,
    },
  ])('$name', ({ readOrder, overrideFrom, seen, expectedTarget, expectedReport }) => {
    const orders = [5000, 6000, 7000]
    const base = readWalkBase(readOrder, overrideFrom)
    const target = reportableReadOrder(orders, base, new Set(seen), true, overrideFrom)
    expect(target).toBe(expectedTarget)
    expect(shouldReportRead(target, readOrder, overrideFrom, -1)).toBe(expectedReport)
  })
})