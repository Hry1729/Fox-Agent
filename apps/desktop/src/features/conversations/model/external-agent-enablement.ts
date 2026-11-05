import { useEffect, useSyncExternalStore } from 'react'
import { desktopClient } from '../api/desktop-client'
import type { AgentBackendId } from './agent-backend'

export const EXTERNAL_AGENT_IDS: readonly Exclude<AgentBackendId, 'fox'>[] = ['pi', 'codex', 'deepseek']
const EMPTY: readonly AgentBackendId[] = []
let enabled: readonly AgentBackendId[] = EMPTY
const listeners = new Set<() => void>()
let refresh: Promise<void> | null = null

export function applyAgentEnablement(agents: readonly { agent: AgentBackendId; enabled: boolean }[]) {
  const next = EXTERNAL_AGENT_IDS.filter(backend => agents.some(agent => agent.agent === backend && agent.enabled))
  if (next.join() === enabled.join()) return
  enabled = next
  listeners.forEach(listener => listener())
}
export function readEnabledExternalAgents(): readonly AgentBackendId[] { return enabled }
export function isAgentEnabled(backend: AgentBackendId | string, selected: readonly string[] = enabled): boolean { return backend === 'fox' || selected.includes(backend) }
export function enabledOrFox(backend: AgentBackendId, selected: readonly string[] = enabled): AgentBackendId { return isAgentEnabled(backend, selected) ? backend : 'fox' }
export async function refreshAgentEnablement() {
  if (!refresh) refresh = desktopClient.agentConfigOverview().then(overview => applyAgentEnablement(overview.agents)).finally(() => { refresh = null })
  return refresh
}
export async function setExternalAgentEnabled(backend: AgentBackendId, on: boolean) {
  if (backend === 'fox') return
  await desktopClient.agentEnableSet(backend, on)
  applyAgentEnablement(EXTERNAL_AGENT_IDS.map(agent => ({ agent, enabled: agent === backend ? on : enabled.includes(agent) })))
}
function subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener) } }
export function useEnabledExternalAgents(): readonly AgentBackendId[] {
  useEffect(() => { void refreshAgentEnablement().catch(() => {}) }, [])
  return useSyncExternalStore(subscribe, readEnabledExternalAgents, () => EMPTY)
}
export const AGENT_DISABLED_NOTICE = '该 Agent 已停用，可在「设置 → 模型与 Agent」中启用'
export const AGENT_DISABLED_CODE = 'agent_backend_user_disabled'
