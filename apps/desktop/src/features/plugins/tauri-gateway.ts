import { createMockPluginGateway } from './gateway'
import type { desktopClient } from '@/features/conversations/api/desktop-client'
import type {
  PluginActivationUpdate,
  PluginCardView,
  PluginCatalogDTO,
  PluginCatalogQuery,
  PluginGateway,
  PluginInstallationsDTO,
  PluginOperationDTO,
  PluginAgentScopeDTO,
} from './model'

export type PluginDesktopClient = Pick<
  typeof desktopClient,
  'pluginCatalogList' | 'pluginInstallationsList' | 'pluginSetActivation'
>

function runningInTauri(): boolean {
  return typeof window !== 'undefined'
    && Boolean((window as Window & { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__)
}

async function resolveClient(client?: PluginDesktopClient): Promise<PluginDesktopClient> {
  if (client) return client
  const module = await import('@/features/conversations/api/desktop-client')
  return module.desktopClient
}

export class PluginGatewayError extends Error {
  readonly code: string
  readonly retryable: boolean

  constructor(code: string, message: string, retryable = false) {
    super(message)
    this.name = 'PluginGatewayError'
    this.code = code
    this.retryable = retryable
  }
}

function unavailable(operation: string): never {
  throw new PluginGatewayError(
    'plugin.operation_unavailable',
    `${operation} 需要等待 Host 操作命令接入。`,
  )
}

export function createTauriPluginGateway(client?: PluginDesktopClient): PluginGateway {
  return {
    catalogList: async (query: PluginCatalogQuery): Promise<PluginCatalogDTO> => (await resolveClient(client)).pluginCatalogList(query),
    installationsList: async (): Promise<PluginInstallationsDTO> => (await resolveClient(client)).pluginInstallationsList(),
    operationsList: async (): Promise<PluginOperationDTO[]> => [],
    install: async () => unavailable('安装插件'),
    uninstall: async () => unavailable('卸载插件'),
    update: async () => unavailable('更新插件'),
    setActivation: async (pluginId: string, update: PluginActivationUpdate): Promise<PluginCardView> =>
      (await resolveClient(client)).pluginSetActivation(pluginId, update),
    agentScopeList: async (): Promise<PluginAgentScopeDTO[]> => [],
  }
}

export function createDefaultPluginGateway(tauriAvailable = runningInTauri()): PluginGateway {
  return tauriAvailable ? createTauriPluginGateway() : createMockPluginGateway()
}

export const defaultPluginGateway = createDefaultPluginGateway()
