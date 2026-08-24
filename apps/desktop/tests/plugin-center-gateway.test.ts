import { describe, expect, test } from 'bun:test'
import { MockPluginGateway } from '../src/features/plugins/gateway'
import { createDefaultPluginGateway, createTauriPluginGateway, type PluginDesktopClient } from '../src/features/plugins/tauri-gateway'
import {
  pluginActivationUpdateToWire,
  pluginCardFromWire,
  type PluginCardView,
} from '../src/features/plugins/model'

describe('MockPluginGateway', () => {
  test('returns catalog, categories, and installed DTOs through the same shape', async () => {
    const gateway = new MockPluginGateway({ now: () => 100 })
    const result = await gateway.catalogList({ kind: 'mcp', pageSize: 10 })
    const installed = await gateway.installationsList()

    expect(result.items.every((item) => item.kind === 'mcp')).toBe(true)
    expect(result.categories).toContain('文件与办公')
    expect(result.total).toBe(3)
    expect(installed.items.some((item) => item.id === 'mcp-filesystem')).toBe(true)
  })

  test('projects install operations and makes completion visible in the catalog', async () => {
    const gateway = new MockPluginGateway({ now: () => 100 })
    const operation = await gateway.install('mcp-browser')

    expect(operation).toMatchObject({ operation: 'install', status: 'running', progress: 24 })
    expect((await gateway.catalogList({ kind: 'mcp' })).items.find((item) => item.id === 'mcp-browser')?.installStatus).toBe('installing')
    expect((await gateway.operationsList()).map((item) => item.id)).toEqual([operation.id])

    const completed = await gateway.completeOperation(operation.id)
    expect(completed).toMatchObject({ status: 'completed', outcome: 'success', progress: 100 })
    expect((await gateway.catalogList({ kind: 'mcp' })).items.find((item) => item.id === 'mcp-browser')?.installStatus).toBe('installed')
  })

  test('updates global activation and per-agent scope without conflating the DTO modes', async () => {
    const gateway = new MockPluginGateway({ now: () => 100 })
    const global = await gateway.setActivation('mcp-filesystem', { mode: 'global', enabled: false })
    expect(global.activation).toEqual({ mode: 'global', enabled: false })

    const before = await gateway.agentScopeList('skill-browser-automation')
    expect(before.filter((item) => item.enabled)).toHaveLength(2)
    const perAgent = await gateway.setActivation('skill-browser-automation', { mode: 'per_agent', agentId: 'research-expert', enabled: false })
    expect(perAgent.activation).toEqual({ mode: 'per_agent', enabledAgentCount: 1 })
    expect((await gateway.agentScopeList('skill-browser-automation')).find((item) => item.agentId === 'research-expert')?.enabled).toBe(false)
  })
})

describe('TauriPluginGateway', () => {
  test('selects the real gateway in Tauri and the mock gateway in browser tests', async () => {
    expect(createDefaultPluginGateway(false)).toBeInstanceOf(MockPluginGateway)
    await expect(createDefaultPluginGateway(true).install('plugin-1')).rejects.toMatchObject({
      code: 'plugin.operation_unavailable',
    })
  })

  test('maps activation payloads across camelCase UI and snake_case Rust enum fields', () => {
    const card: PluginCardView = {
      id: 'skill-research',
      kind: 'skill',
      origin: 'builtin',
      category: '研究',
      name: '研究 Skill',
      description: '测试',
      installStatus: 'installed',
      activation: { mode: 'per_agent', enabledAgentCount: 0 },
      permissions: [],
      compatible: true,
    }

    expect(pluginCardFromWire({ ...card, activation: { mode: 'per_agent', enabled_agent_count: 3 } }).activation)
      .toEqual({ mode: 'per_agent', enabledAgentCount: 3 })
    expect(pluginActivationUpdateToWire({ mode: 'per_agent', agentId: 'agent-1', enabled: true }))
      .toEqual({ mode: 'per_agent', agent_id: 'agent-1', enabled: true })
    expect(pluginActivationUpdateToWire({ mode: 'global', enabled: false }))
      .toEqual({ mode: 'global', enabled: false })
  })

  test('calls the Host commands with camelCase DTOs and keeps activation scopes distinct', async () => {
    const calls: Array<{ name: string; value: unknown }> = []
    const card: PluginCardView = {
      id: 'skill-research',
      kind: 'skill',
      origin: 'builtin',
      category: '研究',
      name: '研究 Skill',
      description: '测试',
      installStatus: 'installed',
      activation: { mode: 'per_agent', enabledAgentCount: 1 },
      permissions: [],
      compatible: true,
    }
    const client: PluginDesktopClient = {
      pluginCatalogList: async (query) => {
        calls.push({ name: 'plugin_catalog_list', value: query })
        return { items: [card], total: 1, categories: ['研究'] }
      },
      pluginInstallationsList: async () => {
        calls.push({ name: 'plugin_installations_list', value: undefined })
        return { items: [card], total: 1 }
      },
      pluginSetActivation: async (pluginId, update) => {
        calls.push({ name: 'plugin_set_activation', value: { pluginId, update } })
        return card
      },
    }
    const gateway = createTauriPluginGateway(client)

    await expect(gateway.catalogList({ kind: 'skill', pageSize: 20 })).resolves.toMatchObject({ total: 1 })
    await expect(gateway.installationsList()).resolves.toMatchObject({ items: [card] })
    await expect(gateway.setActivation('skill-research', { mode: 'per_agent', agentId: 'agent-1', enabled: true })).resolves.toEqual(card)

    expect(calls).toEqual([
      { name: 'plugin_catalog_list', value: { kind: 'skill', pageSize: 20 } },
      { name: 'plugin_installations_list', value: undefined },
      { name: 'plugin_set_activation', value: { pluginId: 'skill-research', update: { mode: 'per_agent', agentId: 'agent-1', enabled: true } } },
    ])
  })

  test('does not silently use mock install behavior in Tauri mode', async () => {
    const gateway = createTauriPluginGateway({
      pluginCatalogList: async () => ({ items: [], total: 0, categories: [] }),
      pluginInstallationsList: async () => ({ items: [], total: 0 }),
      pluginSetActivation: async () => { throw new Error('not used') },
    })

    await expect(gateway.install('plugin-1')).rejects.toMatchObject({ code: 'plugin.operation_unavailable' })
  })
})
