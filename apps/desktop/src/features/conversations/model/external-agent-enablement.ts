// UI 1103: external Agents (Pi, Codex, dsh) are not built into Fox. The user
// installs and configures one on their own computer, then turns it on in
// 设置 → Agent. Only an enabled one appears in the composer's Agent picker and
// can be a new conversation's Agent; Fox is always there.
//
// Stored in localStorage `fox.preferences.enabledExternalAgents` (a JSON array
// of backend ids), off by default. With `FOX_DATA_DIR` the webview profile
// lives under the data directory, so each test data directory has its own.
import { useSyncExternalStore } from 'react'
import type { AgentBackendId } from './agent-backend'

export const ENABLED_EXTERNAL_AGENTS_KEY = 'fox.preferences.enabledExternalAgents'
const CHANGE_EVENT = 'fox:external-agents-enabled'

/** The external Agents 设置 → Agent lists, in display order. */
export const EXTERNAL_AGENT_IDS: readonly Exclude<AgentBackendId, 'fox'>[] = ['pi', 'codex', 'deepseek']

const EMPTY: readonly AgentBackendId[] = []
let cachedRaw: string | null | undefined
let cachedValue: readonly AgentBackendId[] = EMPTY

function parse(raw: string | null): readonly AgentBackendId[] {
  if (!raw) return EMPTY
  try {
    const value: unknown = JSON.parse(raw)
    if (!Array.isArray(value)) return EMPTY
    return EXTERNAL_AGENT_IDS.filter((backend) => value.includes(backend))
  } catch {
    return EMPTY
  }
}

/** The enabled external Agents (stable reference while unchanged). */
export function readEnabledExternalAgents(): readonly AgentBackendId[] {
  if (typeof window === 'undefined') return EMPTY
  let raw: string | null = null
  try { raw = window.localStorage.getItem(ENABLED_EXTERNAL_AGENTS_KEY) } catch { raw = null }
  if (raw !== cachedRaw) {
    cachedRaw = raw
    cachedValue = parse(raw)
  }
  return cachedValue
}

/** Fox is always enabled; an external Agent only after the user turned it on. */
export function isAgentEnabled(backend: AgentBackendId | string, enabled: readonly string[] = readEnabledExternalAgents()): boolean {
  return backend === 'fox' || enabled.includes(backend)
}

/** The backend itself when enabled, else the built-in Fox route. */
export function enabledOrFox(backend: AgentBackendId, enabled: readonly string[] = readEnabledExternalAgents()): AgentBackendId {
  return isAgentEnabled(backend, enabled) ? backend : 'fox'
}

export function setExternalAgentEnabled(backend: AgentBackendId, on: boolean) {
  if (backend === 'fox') return
  const current = readEnabledExternalAgents()
  const next = EXTERNAL_AGENT_IDS.filter((item) => (item === backend ? on : current.includes(item)))
  window.localStorage.setItem(ENABLED_EXTERNAL_AGENTS_KEY, JSON.stringify(next))
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

export function useEnabledExternalAgents(): readonly AgentBackendId[] {
  return useSyncExternalStore(subscribe, readEnabledExternalAgents, () => EMPTY)
}

/** What a send says when the conversation's Agent was turned off in settings. */
export const AGENT_DISABLED_NOTICE = '该 Agent 已停用，可在设置中启用'
/** Refusal code of that local refusal (never sent by the Host). */
export const AGENT_DISABLED_CODE = 'agent_backend_user_disabled'
