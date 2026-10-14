import { useEffect, useSyncExternalStore } from 'react'

/** SSR keeps the fallback; the client starts fetching before any gesture. */
export function deferredModule<T>(load: () => Promise<T>) {
  let pending: Promise<T> | undefined
  let resolved: T | undefined
  const listeners = new Set<() => void>()
  const preload = () => pending ??= load().then(module => {
    resolved = module
    listeners.forEach(notify => notify())
    return module
  }).catch(error => { pending = undefined; throw error })
  const subscribe = (notify: () => void) => { listeners.add(notify); return () => { listeners.delete(notify) } }
  const snapshot = () => resolved
  const serverSnapshot = () => undefined
  const start = () => { void preload().catch(() => { /* fallback remains usable; a gesture retries */ }) }
  function useModule() {
    const module = useSyncExternalStore(subscribe, snapshot, serverSnapshot)
    useEffect(start, [])
    return module
  }
  return { useModule, preload: start }
}
