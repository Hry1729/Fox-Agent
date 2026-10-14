import { useCallback, useSyncExternalStore } from 'react'
import {
  DEFAULT_SHORTCUT_PREFERENCES, SHORTCUT_CHANGED_EVENT, SHORTCUT_STORAGE_KEY,
  readShortcutPreferences, restoreDefaultShortcuts, setShortcutBinding,
  type ShortcutActionId, type ShortcutPreferences,
} from './shortcut-model'

let snapshot: ShortcutPreferences = { ...DEFAULT_SHORTCUT_PREFERENCES }
function getSnapshot() {
  const next = readShortcutPreferences()
  if (Object.keys(next).some((key) => next[key as ShortcutActionId] !== snapshot[key as ShortcutActionId])) snapshot = next
  return snapshot
}
function subscribe(listener: () => void) {
  if (typeof window === 'undefined') return () => {}
  const onStorage = (event: StorageEvent) => {
    if (event.key === SHORTCUT_STORAGE_KEY || event.key === null) listener()
  }
  window.addEventListener(SHORTCUT_CHANGED_EVENT, listener)
  window.addEventListener('storage', onStorage)
  return () => {
    window.removeEventListener(SHORTCUT_CHANGED_EVENT, listener)
    window.removeEventListener('storage', onStorage)
  }
}
const getServerSnapshot = () => DEFAULT_SHORTCUT_PREFERENCES as ShortcutPreferences

/** Updates in the current WebView and across windows, without remounting global listeners. */
export function useShortcutPreferences() {
  const preferences = useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
  const setBinding = useCallback((actionId: ShortcutActionId, shortcut: string | null, replaceConflicts = false) => (
    setShortcutBinding(actionId, shortcut, { replaceConflicts })
  ), [])
  return { preferences, setBinding, restoreDefaults: restoreDefaultShortcuts }
}
