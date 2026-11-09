import type { ContactBubbleRecord } from '@/features/conversations/model/types'

/** K6: only durable companion bubbles count toward the read position. Streaming
 * drafts and replaceable previews are display-only and never appear here. */
export function companionBubbleOrders(bubbles: ContactBubbleRecord[] | undefined): number[] {
  return (bubbles ?? [])
    .filter(bubble => bubble.author === 'companion')
    .map(bubble => bubble.deliveryOrder)
    .sort((a, b) => a - b)
}

/** The walk starts at the auto-read cursor, but a manual unread override pulls it
 * back to just before the override start so re-reading that region can clear it.
 * The persisted cursor itself never rewinds — the backend takes the max. */
export function readWalkBase(readOrder: number, overrideFrom: number | null): number {
  return overrideFrom != null ? Math.min(readOrder, overrideFrom - 1) : readOrder
}

/** Contiguous seen-walk: advance from `base` only while the next bubble has
 * actually been seen. An unseen bubble stops the walk, so the read position can
 * never jump over an unread interval the user has not looked at.
 *
 * The loaded window is only a suffix of the conversation. When earlier history
 * is still unloaded and the window's first companion bubble sits above the walk
 * base, an unread gap may hide between them — the walk must not advance at all
 * until earlier pages are loaded and the gap is accounted for. */
export function reportableReadOrder(
  orders: number[],
  base: number,
  seen: Set<number>,
  hasUnloadedHistory = false,
  overrideFrom: number | null = null,
): number {
  const knownOverrideBoundary =
    overrideFrom != null &&
    base === overrideFrom - 1 &&
    orders[0] === overrideFrom

  if (
    hasUnloadedHistory &&
    orders.length > 0 &&
    orders[0] > base &&
    !knownOverrideBoundary
  ) return base
  let cursor = base
  for (const order of orders) {
    if (order <= cursor) continue
    if (seen.has(order)) cursor = order
    else break
  }
  return cursor
}

/** Whether a computed position is worth reporting: it must advance beyond the
 * last report, and either pass the persisted cursor or cover an active override
 * start (which is how a fresh read clears a manual mark-unread). */
export function shouldReportRead(
  target: number,
  readOrder: number,
  overrideFrom: number | null,
  lastReported: number,
): boolean {
  if (target <= lastReported) return false
  return target > readOrder || (overrideFrom != null && target >= overrideFrom)
}