import { useLayoutEffect, useRef, useState } from 'react'
import { navigationTarget, recordNavigation, type NavigationEntry, type NavigationHistory } from './navigation-history'

export function useNavigationHistory<T>(entry: NavigationEntry<T> | null, restore: (route: T) => Promise<void>, onError: (error: unknown) => void) {
  const history = useRef<NavigationHistory<T>>({ entries: [], index: -1 })
  const restoring = useRef(false)
  const [, refresh] = useState(0)

  useLayoutEffect(() => {
    if (!entry || restoring.current) return
    const changed = history.current.entries[history.current.index]?.key !== entry.key
    history.current = recordNavigation(history.current, entry)
    if (changed) refresh(value => value + 1)
  }, [entry])

  const go = async (delta: -1 | 1) => {
    if (restoring.current) return
    const target = navigationTarget(history.current, delta)
    if (!target) return
    const previous = history.current.entries[history.current.index]
    restoring.current = true
    refresh(value => value + 1)
    try {
      await restore(target.route)
      history.current = { ...history.current, index: history.current.index + delta }
    } catch (error) {
      // A failed conversation load can change selection before rejecting.
      try { if (previous) await restore(previous.route) } catch { /* Surface the original failure below. */ }
      onError(error)
    } finally {
      restoring.current = false
      refresh(value => value + 1)
    }
  }

  return {
    canGoBack: !restoring.current && history.current.index > 0,
    canGoForward: !restoring.current && history.current.index < history.current.entries.length - 1,
    back: () => { void go(-1) },
    forward: () => { void go(1) },
  }
}
