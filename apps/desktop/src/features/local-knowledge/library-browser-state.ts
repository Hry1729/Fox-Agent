import { useSyncExternalStore } from 'react'
import type { FileManagerView } from '@/components/corr/file-manager'
import { libraryFolderCounts, normalizeLibraryPath, type LibraryFile } from './library-file-model'

interface LibraryBrowserState {
  path: string
  view: FileManagerView
  navigation: number
  folders: { path: string; count: number }[]
}

const empty: LibraryBrowserState = { path: '', view: 'details', navigation: 0, folders: [] }
const states = new Map<string, LibraryBrowserState>()
const listeners = new Set<() => void>()
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener) } }
function update(id: string, change: Partial<LibraryBrowserState>) {
  states.set(id, { ...(states.get(id) ?? empty), ...change })
  listeners.forEach((listener) => listener())
}

export function useLibraryBrowser(id = '') {
  return useSyncExternalStore(subscribe, () => states.get(id) ?? empty, () => empty)
}

export function navigateLibraryFolder(id: string, path: string) {
  update(id, { path: normalizeLibraryPath(path), navigation: (states.get(id)?.navigation ?? 0) + 1 })
}

export function setLibraryView(id: string, view: FileManagerView) { update(id, { view }) }

export function publishLibraryFolders(id: string, files: LibraryFile[]) {
  const folders = [...libraryFolderCounts(files)].sort(([a], [b]) => a.localeCompare(b, 'zh-CN')).map(([path, count]) => ({ path, count }))
  update(id, { folders })
}
