import type { ConversationSummary, ProjectRecord } from '@/features/conversations/model/types'
import { canonicalProjectRoot, normalizeProjectRoot } from './project-access-dialog-state'

export interface SidebarProjectGroup<T extends ConversationSummary = ConversationSummary> {
  key: string
  root: string
  name: string
  id?: string
  pinned: boolean
  createdAt: number
  items: T[]
}

/** Conversation recency orders children; persisted project metadata orders groups. */
export function sidebarProjectGroups<T extends ConversationSummary>(conversations: readonly T[], projects: readonly ProjectRecord[]): SidebarProjectGroup<T>[] {
  const catalog = new Map(projects.map(project => [normalizeProjectRoot(project.rootPath), project]))
  const groups = new Map<string, SidebarProjectGroup<T>>()
  for (const item of conversations) {
    const root = item.projectRoot?.trim()
    if (!root) continue
    const key = normalizeProjectRoot(root)
    const existing = groups.get(key)
    if (existing) {
      existing.items.push(item)
      if (!existing.id) existing.createdAt = Math.min(existing.createdAt, item.createdAt)
      continue
    }
    const record = catalog.get(key)
    const canonical = canonicalProjectRoot(record?.rootPath ?? root)
    groups.set(key, {
      key, root: canonical, name: record?.name ?? canonical.split(/[\\/]/).at(-1) ?? root,
      id: record?.id, pinned: Boolean(record?.pinned), createdAt: record?.createdAt ?? item.createdAt,
      items: [item],
    })
  }
  return [...groups.values()].sort((a,b) => Number(b.pinned) - Number(a.pinned)
    || a.createdAt - b.createdAt || a.name.localeCompare(b.name) || a.key.localeCompare(b.key))
}
