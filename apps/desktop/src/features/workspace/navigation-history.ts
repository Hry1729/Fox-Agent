export interface NavigationEntry<T> { key: string; route: T }
export interface NavigationHistory<T> { entries: NavigationEntry<T>[]; index: number }

export function recordNavigation<T>(history: NavigationHistory<T>, entry: NavigationEntry<T>): NavigationHistory<T> {
  if (history.entries[history.index]?.key === entry.key) {
    const entries = [...history.entries]
    entries[history.index] = entry
    return { ...history, entries }
  }
  const entries = [...history.entries.slice(0, history.index + 1), entry].slice(-100)
  return { entries, index: entries.length - 1 }
}

export function navigationTarget<T>(history: NavigationHistory<T>, delta: -1 | 1) {
  return history.entries[history.index + delta]
}
