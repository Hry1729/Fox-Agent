import type { PluginActivation, PluginCardView, PluginKind, PluginInstallStatus } from './model'

export interface PluginCatalogFilter {
  kind: PluginKind
  search?: string
  category?: string
}

export function filterPluginCards(cards: PluginCardView[], filter: PluginCatalogFilter): PluginCardView[] {
  const search = filter.search?.trim().toLocaleLowerCase()
  return cards.filter((card) => {
    if (card.kind !== filter.kind) return false
    if (filter.category && card.category !== filter.category) return false
    if (!search) return true
    return [card.id, card.name, card.description, card.category]
      .some((value) => value.toLocaleLowerCase().includes(search))
  })
}

export function categoriesForCards(cards: PluginCardView[], kind: PluginKind): string[] {
  return [...new Set(cards.filter((card) => card.kind === kind).map((card) => card.category))]
}

export function countInstalledPlugins(cards: PluginCardView[]): number {
  return cards.filter((card) => card.installStatus !== 'not_installed').length
}

export function pluginKindLabel(kind: PluginKind): string {
  if (kind === 'mcp') return 'MCP 服务'
  if (kind === 'skill') return 'Skills'
  return '工具'
}

export function pluginOriginLabel(origin: PluginCardView['origin']): string {
  if (origin === 'builtin') return '内建'
  if (origin === 'official') return '官方'
  if (origin === 'community') return '社区'
  return '本地'
}

export function pluginInstallStatusLabel(status: PluginInstallStatus): string {
  if (status === 'not_installed') return '未安装'
  if (status === 'installing') return '安装中'
  if (status === 'installed') return '已安装'
  if (status === 'update_available') return '可更新'
  if (status === 'uninstalling') return '卸载中'
  return '安装失败'
}

export type PluginAction = 'install' | 'update' | 'uninstall' | 'none'

export function pluginInstallAction(card: PluginCardView): { action: PluginAction; label: string; disabled: boolean } {
  if (card.installStatus === 'not_installed' || card.installStatus === 'install_failed') {
    return { action: 'install', label: card.installStatus === 'install_failed' ? '重试安装' : '安装', disabled: !card.compatible }
  }
  if (card.installStatus === 'installing' || card.installStatus === 'uninstalling') {
    return { action: 'none', label: pluginInstallStatusLabel(card.installStatus), disabled: true }
  }
  if (card.installStatus === 'update_available') {
    return { action: 'update', label: '更新', disabled: !card.compatible }
  }
  return { action: 'uninstall', label: '卸载', disabled: false }
}

export function activationSummary(activation: PluginActivation): string {
  if (activation.mode === 'global') return activation.enabled ? '全局已启用' : '全局未启用'
  if (activation.mode === 'per_agent') return `${activation.enabledAgentCount} 个 Agent 已启用`
  return '由 Host 管理'
}

export function sortPluginCards(cards: PluginCardView[]): PluginCardView[] {
  return [...cards].sort((left, right) => {
    const statusRank = (status: PluginInstallStatus) => status === 'installed' ? 0 : status === 'update_available' ? 1 : 2
    return statusRank(left.installStatus) - statusRank(right.installStatus) || left.name.localeCompare(right.name, 'zh-CN')
  })
}
