import { useCallback, useEffect, useState } from 'react'
import { desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import type { PluginCardView, PluginGateway, PluginKind } from './model'
import { pluginInstallStatusLabel } from './selectors'

export const pluginSections = ['mcp', 'plugin', 'skill'] as const
export type PluginSection = (typeof pluginSections)[number]
export const pluginSectionLabels: Record<PluginSection, string> = { mcp: '连接器', plugin: '插件', skill: '技能' }

export function isPluginSection(value: string | null | undefined): value is PluginSection {
  return pluginSections.some(section => section === value)
}

export function pluginSection(card: PluginCardView): PluginSection | 'tool' {
  return card.id.startsWith('search:') ? 'plugin' : card.kind
}

export function matchesPluginResource(cardId: string, kind: 'mcp' | 'skill', resourceId: string) {
  return cardId === resourceId || cardId === `${kind}:${resourceId}` || cardId === `${kind}-${resourceId}`
}

export function pluginStatus(card: PluginCardView): 'available' | 'disabled' | 'unconfigured' | 'error' {
  if (card.runtimeStatus === 'error' || card.lastError || !card.compatible || card.installStatus === 'install_failed') return 'error'
  if (card.installStatus === 'not_installed') return 'unconfigured'
  if (card.activation.mode === 'global' && !card.activation.enabled) return 'disabled'
  if (card.activation.mode === 'per_agent' && !card.activation.enabledAgentCount) return 'disabled'
  return 'available'
}

export function pluginStatusLabel(card: PluginCardView) {
  if (card.installStatus !== 'installed' && card.installStatus !== 'not_installed') return pluginInstallStatusLabel(card.installStatus)
  const status = pluginStatus(card)
  if (card.id.startsWith('mcp:fox-catalog-') && status === 'available') return card.runtimeStatus === 'healthy' ? '已发现工具' : '待测试'
  if (status === 'error') return '异常'
  if (status === 'unconfigured') return '未配置'
  if (pluginSection(card) === 'plugin') return status === 'disabled' ? '未选用' : '正在使用'
  return status === 'disabled' ? '未启用' : card.activation.mode === 'not_applicable' ? '内置' : '已启用'
}

export function matchesPluginQuery(card: PluginCardView, query: string) {
  return [card.name, card.description, card.category, card.id].some(value => value.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
}

async function loadCatalog(gateway: PluginGateway, kind: PluginKind) {
  const cards: PluginCardView[] = []
  let cursor: string | undefined
  do {
    const result = await gateway.catalogList({ kind, cursor, pageSize: 100 })
    cards.push(...result.items)
    cursor = result.nextCursor
  } while (cursor)
  return cards
}

export function usePluginEntries(gateway: PluginGateway, catalogSection?: PluginSection) {
  const [items, setItems] = useState<PluginCardView[]>([])
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [revision, setRevision] = useState(0)
  const refresh = useCallback(() => setRevision(value => value + 1), [])
  const catalogKind = catalogSection === 'plugin' ? 'tool' : catalogSection
  useEffect(() => {
    let active = true
    setLoading(true)
    setError(null)
    const request = catalogKind ? loadCatalog(gateway, catalogKind) : gateway.installationsList().then(result => result.items)
    void request.then(cards => { if (active) setItems(cards) }).catch(cause => {
      if (active) { setItems([]); setError(desktopErrorDetails(cause).message) }
    }).finally(() => { if (active) setLoading(false) })
    return () => { active = false }
  }, [gateway, catalogKind, revision])
  return { items, error, loading, refresh }
}
