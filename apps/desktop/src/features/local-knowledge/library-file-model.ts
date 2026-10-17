export interface LibraryFile {
  id: string
  name: string
  path: string
  folder?: boolean
  size: number
  modifiedAt?: string | null
  status?: string | null
}

export function normalizeLibraryPath(path: string): string {
  return path.replace(/\\/g, '/').split('/').filter(Boolean).join('/')
}

export function libraryParentPath(path: string): string {
  return normalizeLibraryPath(path).split('/').slice(0, -1).join('/')
}

export function duplicateLibraryImportPaths(files: Array<{ name: string; relativePath?: string }>): string[] {
  const seen = new Set<string>()
  const duplicates = new Set<string>()
  for (const file of files) {
    const path = normalizeLibraryPath(file.relativePath || file.name)
    const key = path.toLocaleLowerCase()
    if (seen.has(key)) duplicates.add(path)
    seen.add(key)
  }
  return [...duplicates]
}

export function libraryFolders(files: LibraryFile[]): string[] {
  return [...libraryFolderCounts(files).keys()].sort((a, b) => a.localeCompare(b, 'zh-CN'))
}

export function libraryFolderCounts(files: LibraryFile[]): Map<string, number> {
  const folders = new Map<string, number>()
  for (const file of files) {
    const parts = normalizeLibraryPath(file.path).split('/')
    if (!file.folder) parts.pop()
    parts.forEach((_, index) => {
      const path = parts.slice(0, index + 1).join('/')
      folders.set(path, (folders.get(path) ?? 0) + (file.folder ? 0 : 1))
    })
  }
  return folders
}

export function visibleLibraryFiles(files: LibraryFile[], path: string, query: string, sort: 'name' | 'time' | 'size', descending = false): LibraryFile[] {
  const needle = query.trim().toLocaleLowerCase()
  return files.filter((file) => needle
    ? normalizeLibraryPath(file.path).toLocaleLowerCase().includes(needle)
    : libraryParentPath(file.path) === normalizeLibraryPath(path))
    .sort((a, b) => {
      if (Boolean(a.folder) !== Boolean(b.folder)) return a.folder ? -1 : 1
      const value = sort === 'time' ? (a.modifiedAt ?? '').localeCompare(b.modifiedAt ?? '')
        : sort === 'size' ? a.size - b.size : a.name.localeCompare(b.name, 'zh-CN', { numeric: true })
      return descending ? -value : value
    })
}

export function remoteLibraryFiles(documents: Array<{ id: string; name: string; parentId: string | null; isFolder: boolean; size: number; status: string | null; updatedAt: string | null }>): LibraryFile[] {
  const byId = new Map(documents.map((document) => [document.id, document]))
  const parentNames = (id: string | null, visited: Set<string>): string[] => {
    if (!id || visited.has(id)) return []
    visited.add(id)
    const parent = byId.get(id)
    return parent?.isFolder ? [...parentNames(parent.parentId, visited), parent.name] : []
  }
  return documents.map((document) => ({ id: document.id, name: document.name, path: [...parentNames(document.parentId, new Set([document.id])), document.name].join('/'), folder: document.isFolder, size: document.size, status: document.status, modifiedAt: document.updatedAt }))
}
