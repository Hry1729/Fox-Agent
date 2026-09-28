import { useSyncExternalStore } from 'react'

export type ProcessDisplayMode = 'compact' | 'standard' | 'detailed' | 'verbose'

export const PROCESS_DISPLAY_MODE_KEY = 'fox.preferences.processDisplayMode'
const CHANGE_EVENT = 'fox:process-display-mode'

export function normalizeProcessDisplayMode(value: unknown): ProcessDisplayMode {
  if (value === 'normal') return 'standard'
  if (value === 'expanded') return 'detailed'
  return value === 'compact' || value === 'standard' || value === 'detailed' || value === 'verbose'
    ? value : 'standard'
}

export function readProcessDisplayMode(): ProcessDisplayMode {
  return typeof window === 'undefined' ? 'standard'
    : normalizeProcessDisplayMode(window.localStorage.getItem(PROCESS_DISPLAY_MODE_KEY))
}

export function persistProcessDisplayMode(value: ProcessDisplayMode) {
  window.localStorage.setItem(PROCESS_DISPLAY_MODE_KEY, value)
  window.dispatchEvent(new Event(CHANGE_EVENT))
}

function subscribe(onChange: () => void) {
  window.addEventListener(CHANGE_EVENT, onChange)
  window.addEventListener('storage', onChange)
  return () => {
    window.removeEventListener(CHANGE_EVENT, onChange)
    window.removeEventListener('storage', onChange)
  }
}

export function useProcessDisplayMode(): ProcessDisplayMode {
  return useSyncExternalStore(subscribe, readProcessDisplayMode, () => 'standard')
}
