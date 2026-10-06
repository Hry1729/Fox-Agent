import { describe, expect, test } from 'bun:test'
import { sidebarProjectGroups } from '../src/features/chat/sidebar-project-groups'
import type { ConversationSummary, ProjectRecord } from '../src/features/conversations/model/types'

const project = (id: string, rootPath: string, createdAt: number, pinned = false): ProjectRecord => ({
  id, name: id, rootPath, createdAt, pinned, permissionMode: 'ask', status: 'active', updatedAt: createdAt, lastOpenedAt: null,
})
const conversation = (id: string, projectRoot: string, createdAt: number): ConversationSummary => ({
  id, agentId: 'fox-general', agentName: 'Fox', title: id, projectId: null, projectRoot, createdAt,
  updatedAt: createdAt, lastMessageAt: createdAt, permissionMode: 'ask', status: 'active',
})

describe('sidebar project ordering', () => {
  const catalog = [project('older', 'D:/projects/older', 10), project('newer', 'D:/projects/newer', 20)]
  const rows = [conversation('new-active', 'D:/projects/newer', 50), conversation('old-active', 'D:/projects/older', 60)]
  test('opening or updating conversations cannot change the creation order of projects', () => {
    expect(sidebarProjectGroups(rows, catalog).map(group => group.id)).toEqual(['older', 'newer'])
    expect(sidebarProjectGroups([...rows].reverse(), catalog).map(group => group.id)).toEqual(['older', 'newer'])
  })
  test('pinning moves the project above older ones; unpinning restores its stable position', () => {
    const pinned = catalog.map(item => item.id === 'newer' ? { ...item, pinned: true } : item)
    expect(sidebarProjectGroups(rows, pinned).map(group => group.id)).toEqual(['newer', 'older'])
    expect(sidebarProjectGroups(rows, catalog).map(group => group.id)).toEqual(['older', 'newer'])
  })
  test('full normalized paths, rather than folder names, identify the pinned project', () => {
    const records = [project('first', 'D:/one/Fox', 1), project('second', 'D:/two/Fox', 2, true)]
    const groups = sidebarProjectGroups([conversation('a', 'd:\\ONE\\fox', 30), conversation('b', 'D:/two/Fox/', 40)], records)
    expect(groups.map(group => group.id)).toEqual(['second', 'first'])
    expect(groups[0].pinned).toBe(true)
  })
  test('legacy projects without catalog metadata remain stable when child recency changes', () => {
    const rows = [conversation('b', '/work/b', 20), conversation('a', '/work/a', 10)]
    expect(sidebarProjectGroups(rows, []).map(group => group.root)).toEqual(['/work/a', '/work/b'])
    expect(sidebarProjectGroups([...rows].reverse(), []).map(group => group.root)).toEqual(['/work/a', '/work/b'])
  })
})
