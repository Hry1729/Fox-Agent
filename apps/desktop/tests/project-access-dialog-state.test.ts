import { describe, expect, test } from 'bun:test'
import { normalizeProjectPermission, selectProjectRoot, validPickedProjectFolder } from '../src/features/chat/project-access-dialog-state'
import type { ProjectRecord } from '../src/features/conversations/model/types'

function project(rootPath: string, permissionMode: ProjectRecord['permissionMode']): ProjectRecord {
  return {
    id: rootPath,
    name: rootPath.split(/[\\/]/).at(-1) ?? rootPath,
    rootPath,
    permissionMode,
    status: 'active',
    createdAt: 1,
    updatedAt: 1,
    lastOpenedAt: null,
  }
}

describe('direct project selection', () => {
  test('normalizes the saved default permission before creating a project', () => {
    expect(normalizeProjectPermission('read_only')).toBe('read_only')
    expect(normalizeProjectPermission('allow')).toBe('allow')
    expect(normalizeProjectPermission('unknown')).toBe('ask')
    expect(normalizeProjectPermission(null, 'read_only')).toBe('read_only')
  })

  test('uses ask mode by default for a new folder', () => {
    expect(selectProjectRoot('D:\\projects\\new', [], 'ask')).toEqual({
      path: 'D:\\projects\\new',
      permissionMode: 'ask',
    })
  })

  test('uses the saved permission for a matching existing project', () => {
    const selectedPath = 'd:\\projects\\fox\\'
    const selected = selectProjectRoot(
      selectedPath,
      [project('D:\\Projects\\Fox', 'allow')],
      'ask',
    )

    expect(selected).toEqual({ path: selectedPath, permissionMode: 'allow' })
  })

  test('accepts only a non-empty path returned by the native folder picker', () => {
    expect(validPickedProjectFolder('  D:\\projects\\new  ')).toBe('D:\\projects\\new')
    expect(validPickedProjectFolder(null)).toBeNull()
    expect(() => validPickedProjectFolder(undefined)).toThrow('没有返回有效路径')
    expect(() => validPickedProjectFolder('   ')).toThrow('没有返回有效路径')
  })
})
