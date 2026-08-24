import { describe, expect, test } from 'bun:test'
import type { PluginCardView } from '../src/features/plugins/model'
import {
  activationSummary,
  categoriesForCards,
  filterPluginCards,
  pluginInstallAction,
  pluginKindLabel,
  sortPluginCards,
} from '../src/features/plugins/selectors'

function card(overrides: Partial<PluginCardView> = {}): PluginCardView {
  return {
    id: 'plugin',
    kind: 'skill',
    origin: 'builtin',
    category: '效率工具',
    name: '插件',
    description: '一个用于测试的插件',
    installStatus: 'not_installed',
    activation: { mode: 'per_agent', enabledAgentCount: 0 },
    permissions: [],
    compatible: true,
    ...overrides,
  }
}

describe('plugin center selectors', () => {
  test('filters by kind, category, and searchable metadata', () => {
    const cards = [
      card({ id: 'browser', kind: 'skill', category: '效率工具', name: 'BrowserSkill' }),
      card({ id: 'pdf', kind: 'skill', category: '文档处理', name: 'PDF 文档提取' }),
      card({ id: 'terminal', kind: 'tool', category: '开发工具', name: '终端命令' }),
    ]

    expect(filterPluginCards(cards, { kind: 'skill' }).map((item) => item.id)).toEqual(['browser', 'pdf'])
    expect(filterPluginCards(cards, { kind: 'skill', category: '文档处理' }).map((item) => item.id)).toEqual(['pdf'])
    expect(filterPluginCards(cards, { kind: 'skill', search: 'browser' }).map((item) => item.id)).toEqual(['browser'])
    expect(categoriesForCards(cards, 'skill')).toEqual(['效率工具', '文档处理'])
  })

  test('keeps installed plugins before catalog-only plugins', () => {
    const cards = [
      card({ id: 'zeta', name: 'Zeta', installStatus: 'not_installed' }),
      card({ id: 'alpha', name: 'Alpha', installStatus: 'installed' }),
      card({ id: 'beta', name: 'Beta', installStatus: 'update_available' }),
    ]

    expect(sortPluginCards(cards).map((item) => item.id)).toEqual(['alpha', 'beta', 'zeta'])
  })

  test('exposes install, retry, update, uninstall, and busy states', () => {
    expect(pluginInstallAction(card()).action).toBe('install')
    expect(pluginInstallAction(card({ installStatus: 'install_failed' })).label).toBe('重试安装')
    expect(pluginInstallAction(card({ installStatus: 'update_available' })).action).toBe('update')
    expect(pluginInstallAction(card({ installStatus: 'installed' })).action).toBe('uninstall')
    expect(pluginInstallAction(card({ installStatus: 'installing' }))).toMatchObject({ action: 'none', disabled: true })
    expect(pluginInstallAction(card({ compatible: false })).disabled).toBe(true)
  })

  test('keeps activation semantics explicit for global, per-agent, and Host-managed tools', () => {
    expect(activationSummary({ mode: 'global', enabled: true })).toBe('全局已启用')
    expect(activationSummary({ mode: 'per_agent', enabledAgentCount: 2 })).toBe('2 个 Agent 已启用')
    expect(activationSummary({ mode: 'not_applicable' })).toBe('由 Host 管理')
    expect(pluginKindLabel('mcp')).toBe('MCP 服务')
    expect(pluginKindLabel('skill')).toBe('Skills')
    expect(pluginKindLabel('tool')).toBe('工具')
  })
})
