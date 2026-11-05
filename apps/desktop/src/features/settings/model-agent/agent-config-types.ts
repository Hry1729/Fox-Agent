import type { AgentBackendId } from '@/features/conversations/model/agent-backend'
export type AgentId = AgentBackendId
export type ExternalAgentId = Exclude<AgentId, 'fox'>
export type ApiType = 'openai-completions' | 'anthropic-messages' | 'openai-responses'
export type BindingSource = { source: 'fox_service'; providerId: string | null; endpointId: string; modelId: string } | { source: 'native' }
export type AgentBinding = BindingSource & { updatedAt: number }
export type AgentConfigState = 'ready' | 'configured_unverified' | 'native_importable' | 'installed_unconfigured' | 'not_installed' | 'read_failed'
export type VerifyDepth = 'model' | 'agent' | 'tool'
export type VerificationLayer = 'config_saved' | 'model_request' | 'agent_output' | 'tool_call'
export interface LayerResult { layer: VerificationLayer; status: 'passed' | 'failed' | 'skipped' | 'running'; latencyMs: number | null; message: string; code: string | null; detail: string | null }
export interface VerificationReport { identity: string; checkedAt: number; layers: LayerResult[] }
export interface ServiceSummary {
  providerId: string
  name: string
  isDefault: boolean
  matches: { agent: AgentId; endpointId: string; apiType: ApiType; verified: boolean; verifiedAt: number | null }[]
  missing: { agent: AgentId; apiType: ApiType }[]
}
export interface AgentConfigStatus {
  agent: AgentId
  enabled: boolean
  state: AgentConfigState
  install: { detected: boolean; launchable: boolean; via: 'managed' | 'system' | null; blocker: 'not_installed' | 'system_only' | 'runtime_incomplete' | null }
  native: { available: boolean; changed: boolean }
  binding: AgentBinding | null
  candidates: { providerId: string; endpointId: string; apiType: ApiType; modelId: string; verified: boolean }[]
  verification: VerificationReport | null
  diagnostic: { code: string; detail?: string | null } | null
}
export interface AgentConfigOverview { services: ServiceSummary[]; agents: AgentConfigStatus[] }
export type ImportScope = 'all_and_new' | 'all_existing' | 'default_home'
export interface ImportPreview {
  backend: ExternalAgentId
  items: { id: string; label: string; kind: 'file' | 'env_key'; defaultSelected: boolean; devPath: string | null }[]
  scopes: { id: ImportScope; projectCount: number }[]
  conflicts: { projectId: string; projectName: string; items: string[] }[]
  sourceChanged: boolean
}
export interface ImportApplyRequest { backend: ExternalAgentId; selected: string[]; scope: ImportScope; onConflict: 'keep' | 'overwrite_with_backup'; developer?: boolean }
export interface ImportApplied { written: number; kept: number; backups: number; status: AgentConfigStatus }
