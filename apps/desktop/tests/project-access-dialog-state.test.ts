import { describe, expect, test } from 'bun:test'
import { canonicalProjectRoot, matchingProject, normalizeProjectRoot, normalizeProjectPermission, selectProjectRoot, validPickedProjectFolder } from '../src/features/chat/project-access-dialog-state'
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

  test('uses the saved permission for a matching existing project and writes one canonical path', () => {
    const selected = selectProjectRoot(
      'd:\\projects\\fox\\',
      [project('D:\\Projects\\Fox', 'allow')],
      'ask',
    )

    // One directory must not be recorded twice just because the picker returned a
    // different separator style or a trailing separator.
    expect(selected).toEqual({ path: 'd:\\projects\\fox', permissionMode: 'allow' })
  })

  test('identifies a project by its full path, never by its folder name', () => {
    const projects = [project('D:\\work\\one\\shared', 'allow'), project('D:\\work\\two\\shared', 'read_only')]
    // Same leaf name, different project: the full path decides.
    expect(matchingProject('D:/work/two/shared/', projects)?.permissionMode).toBe('read_only')
    expect(matchingProject('D:\\work\\three\\shared', projects)).toBeUndefined()
    expect(normalizeProjectRoot('D:\\WORK\\Two/Shared\\')).toBe('d:\\work\\two\\shared')
  })

  test('a drive root keeps its separator instead of collapsing to a drive letter', () => {
    expect(canonicalProjectRoot('D:\\')).toBe('D:\\')
    expect(canonicalProjectRoot('D:/')).toBe('D:\\')
    expect(canonicalProjectRoot('  D:\\work\\project\\  ')).toBe('D:\\work\\project')
    expect(canonicalProjectRoot('\\\\server\\share\\')).toBe('\\\\server\\share')
    expect(normalizeProjectRoot('D:\\')).toBe('d:\\')
  })

  test('accepts only a non-empty path returned by the native folder picker', () => {
    expect(validPickedProjectFolder('  D:\\projects\\new  ')).toBe('D:\\projects\\new')
    expect(validPickedProjectFolder(null)).toBeNull()
    expect(() => validPickedProjectFolder(undefined)).toThrow('没有返回有效路径')
    expect(() => validPickedProjectFolder('   ')).toThrow('没有返回有效路径')
  })
})
