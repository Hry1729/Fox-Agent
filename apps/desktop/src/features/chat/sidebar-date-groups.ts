import type { ConversationSummary } from '@/features/conversations/model/types'

export function sidebarDateGroups<T extends ConversationSummary>(items: readonly T[], now = new Date()) {
  const keyOf = (date: Date) => `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
  const today = keyOf(now)
  const yesterdayDate = new Date(now)
  yesterdayDate.setDate(now.getDate() - 1)
  const yesterday = keyOf(yesterdayDate)
  const groups = new Map<string, { key: string; label: string; items: T[] }>()
  for (const item of [...items].sort((a, b) => Number(Boolean(b.pinned)) - Number(Boolean(a.pinned)) || (b.lastMessageAt || b.createdAt) - (a.lastMessageAt || a.createdAt))) {
    const date = new Date(item.lastMessageAt || item.createdAt)
    const day = Number.isNaN(date.getTime()) ? 'unknown' : keyOf(date)
    const key = item.pinned ? 'pinned' : day
    const label = item.pinned ? '置顶' : day === today ? '今天' : day === yesterday ? '昨天' : day === 'unknown' ? '更早' : day
    const group = groups.get(key) ?? { key, label, items: [] }
    group.items.push(item)
    groups.set(key, group)
  }
  return [...groups.values()]
}
