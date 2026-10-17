import { describe, expect, test } from 'bun:test'
import { duplicateLibraryImportPaths, libraryFolderCounts, libraryFolders, remoteLibraryFiles, visibleLibraryFiles, type LibraryFile } from '../src/features/local-knowledge/library-file-model'

const files: LibraryFile[] = [
  { id: 'original-1', name: '预算.xlsx', path: '申报/预算.xlsx', size: 200, status: 'indexed' },
  { id: 'original-2', name: '报告.pdf', path: '申报/论文/报告.pdf', size: 300, status: 'error' },
  { id: 'original-3', name: '说明.md', path: '说明.md', size: 100, status: 'queued' },
]

describe('资料文件模型', () => {
  test('folder totals count descendants once and preserve empty folders', () => {
    const counts = libraryFolderCounts([...files, { id: 'empty', name: '空目录', path: '申报/空目录', folder: true, size: 0 }, { id: 'similar', name: 'a.md', path: '申报二/a.md', size: 1 }])
    expect(counts.get('申报')).toBe(2)
    expect(counts.get('申报/论文')).toBe(1)
    expect(counts.get('申报/空目录')).toBe(0)
    expect(counts.get('申报二')).toBe(1)
  })
  test('derive folders without changing document identities or paths', () => {
    expect(libraryFolders(files)).toEqual(['申报', '申报/论文'])
    expect(visibleLibraryFiles(files, '申报', '', 'name').map((file) => file.id)).toEqual(['original-1'])
    expect(files[0].path).toBe('申报/预算.xlsx')
  })
  test('search covers nested paths and sorting does not mutate the source', () => {
    expect(visibleLibraryFiles(files, '', '报告', 'name').map((file) => file.id)).toEqual(['original-2'])
    expect(visibleLibraryFiles(files, '', '申报', 'size', true).map((file) => file.id)).toEqual(['original-2', 'original-1'])
    expect(files[0].id).toBe('original-1')
  })
  test('detect Windows name collisions across source locations', () => {
    expect(duplicateLibraryImportPaths([{ name: 'A.PDF' }, { name: 'a.pdf' }])).toEqual(['a.pdf'])
    expect(duplicateLibraryImportPaths([{ name: '报告.pdf', relativePath: '一/报告.pdf' }, { name: '报告.pdf', relativePath: '二/报告.pdf' }])).toEqual([])
  })
  test('remote hierarchy preserves IDs and handles unavailable and cyclic parents', () => {
    const remote = remoteLibraryFiles([
      { id: 'folder-id', name: '需求', parentId: null, isFolder: true, size: 0, status: null, updatedAt: null },
      { id: 'document-id', name: '说明.md', parentId: 'folder-id', isFolder: false, size: 20, status: 'done', updatedAt: null },
      { id: 'orphan', name: '孤立.pdf', parentId: 'missing', isFolder: false, size: 30, status: null, updatedAt: null },
      { id: 'cycle', name: '循环', parentId: 'cycle', isFolder: true, size: 0, status: null, updatedAt: null },
    ])
    expect(remote.find((file) => file.id === 'document-id')?.path).toBe('需求/说明.md')
    expect(remote.find((file) => file.id === 'orphan')?.path).toBe('孤立.pdf')
    expect(remote.find((file) => file.id === 'cycle')?.path).toBe('循环')
  })
})
