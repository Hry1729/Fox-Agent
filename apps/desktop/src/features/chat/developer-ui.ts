// Developer UI (composer batch 2026-10-31): the debugging details above the
// composer (backend chip, capability chips, configuration source / import
// fold, local preview) are shown only in developer mode. Off by default.
//
// On when either:
// * 设置 → 对话 →「开发者模式」(stored in localStorage `fox.preferences.developerUi`), or
// * the build / dev server was started with `FOX_DEV_UI=1` (vite define `__FOX_DEV_UI__`).
import { useSyncExternalStore } from 'react'

export const DEVELOPER_UI_KEY = 'fox.preferences.developerUi'
const CHANGE_EVENT = 'fox:developer-ui'

/** The build-time switch (`FOX_DEV_UI=1`); false in tests and normal builds. */
export function developerUiFromBuild(): boolean {
  return typeof __FOX_DEV_UI__ !== 'undefined' && __FOX_DEV_UI__ === true
}

export function readDeveloperUi(): boolean {
  if (developerUiFromBuild()) return true
  if (typeof window === 'undefined') return false
  try { return window.localStorage.getItem(DEVELOPER_UI_KEY) === '1' } catch { return false }
}

export function persistDeveloperUi(on: boolean) {
  window.localStorage.setItem(DEVELOPER_UI_KEY, on ? '1' : '0')
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

export function useDeveloperUi(): boolean {
  return useSyncExternalStore(subscribe, readDeveloperUi, () => false)
}
