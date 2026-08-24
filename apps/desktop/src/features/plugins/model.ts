export const pluginKinds = ['mcp', 'skill', 'tool'] as const
export type PluginKind = (typeof pluginKinds)[number]

export const pluginOrigins = ['builtin', 'official', 'community', 'local'] as const
export type PluginOrigin = (typeof pluginOrigins)[number]

export const pluginInstallStatuses = [
  'not_installed',
  'installing',
  'installed',
  'update_available',
  'uninstalling',
  'install_failed',
] as const
export type PluginInstallStatus = (typeof pluginInstallStatuses)[number]

export const mcpRuntimeStatuses = [
  'unknown',
  'ready',
  'connecting',
  'healthy',
  'degraded',
  'error',
] as const
export type McpRuntimeStatus = (typeof mcpRuntimeStatuses)[number]

export type PluginActivation =
  | { mode: 'global'; enabled: boolean }
  | { mode: 'per_agent'; enabledAgentCount: number }
  | { mode: 'not_applicable' }

export type PluginActivationUpdate =
  | { mode: 'global'; enabled: boolean }
  | { mode: 'per_agent'; agentId: string; enabled: boolean }

export type PluginActivationUpdateWire =
  | { mode: 'global'; enabled: boolean }
  | { mode: 'per_agent'; agent_id: string; enabled: boolean }

export function pluginActivationUpdateToWire(update: PluginActivationUpdate): PluginActivationUpdateWire {
  return update.mode === 'per_agent'
    ? { mode: 'per_agent', agent_id: update.agentId, enabled: update.enabled }
    : update
}

export interface PluginCardView {
  id: string
  kind: PluginKind
  origin: PluginOrigin
  category: string
  name: string
  description: string
  icon?: string
  version?: string
  installStatus: PluginInstallStatus
  activation: PluginActivation
  runtimeStatus?: McpRuntimeStatus
  permissions: string[]
  compatible: boolean
  incompatibilityReason?: string
  installedAt?: number
  updatedAt?: number
}

export type PluginActivationWire =
  | { mode: 'global'; enabled: boolean }
  | { mode: 'per_agent'; enabledAgentCount?: number; enabled_agent_count?: number }
  | { mode: 'not_applicable' }

export type PluginCardWire = Omit<PluginCardView, 'activation'> & {
  activation: PluginActivationWire
}

export function pluginCardFromWire(card: PluginCardWire): PluginCardView {
  if (card.activation.mode !== 'per_agent') return card as PluginCardView
  return {
    ...card,
    activation: {
      mode: 'per_agent',
      enabledAgentCount: card.activation.enabledAgentCount ?? card.activation.enabled_agent_count ?? 0,
    },
  }
}

export interface PluginCatalogQuery {
  kind: PluginKind
  search?: string
  category?: string
  pageSize?: number
  cursor?: string
}

export interface PluginCatalogDTO {
  items: PluginCardView[]
  total: number
  categories: string[]
  nextCursor?: string
}

export type PluginCatalogWireDTO = Omit<PluginCatalogDTO, 'items'> & {
  items: PluginCardWire[]
}

export type InstalledPluginDTO = PluginCardView

export interface PluginInstallationsDTO {
  items: InstalledPluginDTO[]
  total: number
}

export type PluginInstallationsWireDTO = Omit<PluginInstallationsDTO, 'items'> & {
  items: PluginCardWire[]
}

export const pluginOperationStages = [
  'resolving',
  'downloading',
  'verifying',
  'extracting',
  'registering',
] as const
export type PluginOperationStage = (typeof pluginOperationStages)[number]

export const pluginOperationStatuses = [
  'queued',
  'running',
  'completed',
  'failed',
  'cancelled',
] as const
export type PluginOperationStatus = (typeof pluginOperationStatuses)[number]

export interface PluginOperationDTO {
  id: string
  pluginId: string
  parentOperationId?: string
  operation: 'install' | 'uninstall' | 'update' | 'import'
  status: PluginOperationStatus
  stage?: PluginOperationStage
  progress: number
  outcome?: 'success' | 'partial'
  message?: string
  errorCode?: string
  errorMessage?: string
  createdAt: number
  updatedAt: number
}

export interface PluginAgentScopeDTO {
  agentId: string
  agentName: string
  enabled: boolean
}

export interface PluginGateway {
  catalogList(query: PluginCatalogQuery): Promise<PluginCatalogDTO>
  installationsList(): Promise<PluginInstallationsDTO>
  operationsList(): Promise<PluginOperationDTO[]>
  install(pluginId: string): Promise<PluginOperationDTO>
  uninstall(pluginId: string): Promise<PluginOperationDTO>
  update(pluginId: string): Promise<PluginOperationDTO>
  setActivation(pluginId: string, update: PluginActivationUpdate): Promise<PluginCardView>
  agentScopeList(pluginId: string): Promise<PluginAgentScopeDTO[]>
}
