import type { KnowledgeDatabase, KnowledgeTypeInfo } from '@/api/knowledge'

const ICON_BASE = 'https://registry.npmmirror.com/@lobehub/icons-static-svg/latest/files/icons'

export const KB_BRAND_ICON_URLS: Record<string, string> = {
  dify: `${ICON_BASE}/dify-color.svg`,
  notion: `${ICON_BASE}/notion.svg`
}

const KB_TYPE_LABELS: Record<string, string> = {
  milvus: 'Yuxi',
  dify: 'Dify',
  notion: 'Notion'
}

const KB_TYPE_ICONS: Record<string, string> = {
  milvus: 'ri:database-2-line',
  dify: 'ri:cloud-line',
  notion: 'ri:file-text-line'
}

export type KbTypeAccent = 'blue' | 'gold' | 'purple'

const KB_TYPE_ACCENTS: Record<string, KbTypeAccent> = {
  milvus: 'blue',
  dify: 'gold',
  notion: 'purple'
}

const READ_ONLY_KB_TYPES = new Set(['dify', 'notion'])

export const normalizeKbType = (kbType?: string) => String(kbType || 'milvus').toLowerCase()

export const getKbTypeLabel = (kbType?: string, types?: Record<string, KnowledgeTypeInfo>) => {
  const key = normalizeKbType(kbType)
  return types?.[key]?.label || KB_TYPE_LABELS[key] || key
}

export const getKbTypeIcon = (kbType?: string) => {
  const key = normalizeKbType(kbType)
  return KB_TYPE_ICONS[key] || 'ri:database-2-line'
}

export const getKbTypeAccent = (kbType?: string): KbTypeAccent => {
  const key = normalizeKbType(kbType)
  return KB_TYPE_ACCENTS[key] || 'blue'
}

export const getKbBrandIconUrl = (kbType?: string) => {
  const key = normalizeKbType(kbType)
  return KB_BRAND_ICON_URLS[key] || ''
}

export const isReadOnlyDatabase = (
  database: KnowledgeDatabase | string,
  types?: Record<string, KnowledgeTypeInfo>
) => {
  const kbType = typeof database === 'string' ? normalizeKbType(database) : normalizeKbType(database.kb_type)
  if (typeof database !== 'string') {
    if (database.supports_documents !== undefined) return database.supports_documents === false
  }
  if (types?.[kbType]?.supports_documents !== undefined) {
    return types[kbType].supports_documents === false
  }
  return READ_ONLY_KB_TYPES.has(kbType)
}

export const getEmbeddingModelShortName = (database: KnowledgeDatabase) => {
  const spec = String(database.embedding_model_spec || database.embed_model || '').trim()
  if (!spec) return ''
  return spec.split('/').pop() || spec
}

export const formatKbCreatedMeta = (createdAt?: string) => {
  if (!createdAt) return ''
  const date = new Date(createdAt)
  if (Number.isNaN(date.getTime())) return ''

  const now = new Date()
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate())
  const startOfCreated = new Date(date.getFullYear(), date.getMonth(), date.getDate())
  const diffDays = Math.floor((startOfToday.getTime() - startOfCreated.getTime()) / 86400000)

  if (diffDays === 0) return '今天创建'
  if (diffDays === 1) return '昨天创建'
  if (diffDays < 7) return `${diffDays} 天前`
  if (diffDays < 30) return `${Math.floor(diffDays / 7)} 周前`

  const y = date.getFullYear()
  const m = String(date.getMonth() + 1).padStart(2, '0')
  const d = String(date.getDate()).padStart(2, '0')
  return `${y}-${m}-${d}`
}

export const getKbFileCount = (database: KnowledgeDatabase) => {
  const count = database.row_count ?? database.file_count
  return Number.isFinite(Number(count)) ? Number(count) : 0
}

export const getKbCardMeta = (
  database: KnowledgeDatabase,
  types?: Record<string, KnowledgeTypeInfo>
) => {
  const parts: string[] = []
  const createdMeta = formatKbCreatedMeta(String(database.created_at || ''))
  if (createdMeta) parts.push(createdMeta)
  if (!isReadOnlyDatabase(database, types)) {
    parts.push(`${getKbFileCount(database)} 个文件`)
  }
  return parts.join(' · ')
}
