import { createMockPluginCatalog, createMockPluginGateway } from './gateway'
import { connectorCatalog, connectorServerId } from './connector-presets'
import { searchProviders } from '@/features/settings/search-providers'
import { chatSelectableAgents } from '@/features/agents/agent-classification'
import { matchesPluginResource } from './plugin-browser-data'
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
> & Partial<Pick<typeof desktopClient, 'listAgents' | 'listSkills'>>

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
    agentScopeList: async (pluginId): Promise<PluginAgentScopeDTO[]> => {
      const { listAgents, listSkills } = await resolveClient(client)
      if (!listAgents || !listSkills) return unavailable('查看技能适用范围')
      const assistants = chatSelectableAgents(await listAgents()).filter(agent => agent.runtimeType === 'pi')
      return Promise.all(assistants.map(async agent => {
        const skills = await listSkills(agent.id)
        return { agentId: agent.id, agentName: agent.name, enabled: skills.some(skill => matchesPluginResource(pluginId, 'skill', skill.id) && skill.enabled) }
      }))
    },
  }
}

export function createDefaultPluginGateway(tauriAvailable = runningInTauri()): PluginGateway {
  if (tauriAvailable) return createTauriPluginGateway()
  const catalog: PluginCardView[] = connectorCatalog.map(item => ({
    id: 'mcp:' + connectorServerId(item), kind: 'mcp', origin: 'builtin', name: item.name,
    category: item.category, description: item.description, icon: 'cable', installStatus: 'not_installed',
    activation: { mode: 'global', enabled: false }, permissions: [], compatible: true,
  }))
  const plugins: PluginCardView[] = searchProviders.map(provider => ({
    id: 'search:' + provider.id, kind: 'tool', origin: 'builtin', name: provider.name,
    category: '搜索服务', description: provider.description, icon: 'search', installStatus: 'installed',
    activation: { mode: 'global', enabled: provider.id === 'auto' }, permissions: [], compatible: true,
  }))
  return createMockPluginGateway({ catalog: [...createMockPluginCatalog(), ...plugins, ...catalog] })
}

export const defaultPluginGateway = createDefaultPluginGateway()
