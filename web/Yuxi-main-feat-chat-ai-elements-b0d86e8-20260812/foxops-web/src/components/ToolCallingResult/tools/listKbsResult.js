const WRAPPER_KEYS = [
  'data',
  'result',
  'results',
  'items',
  'content',
  'knowledge_bases',
  'knowledgeBases',
  'kbs',
  'databases'
]

const normalizeKb = (item) => {
  if (!item || typeof item !== 'object' || Array.isArray(item)) return null

  const name = item.name || item.kb_name || item.database_name || item.title
  if (typeof name !== 'string' || !name.trim()) return null

  return {
    ...item,
    kb_id: item.kb_id || item.id || '',
    name: name.trim(),
    description: item.description || item.desc || item.summary || ''
  }
}

export const normalizeKbListResult = (content) => {
  const queue = [content]
  const visited = new Set()

  while (queue.length > 0) {
    const current = queue.shift()
    if (current == null) continue

    if (typeof current === 'string') {
      const value = current.trim()
      if (!value) continue
      try {
        queue.push(JSON.parse(value))
      } catch {
        // 错误提示或普通文本不是知识库列表。
      }
      continue
    }

    if (typeof current !== 'object' || visited.has(current)) continue
    visited.add(current)

    if (Array.isArray(current)) {
      const rows = current.map(normalizeKb).filter(Boolean)
      if (rows.length > 0) return rows

      // 兼容 LangChain/OpenAI 的 text content block 包装。
      current.forEach((item) => {
        if (item && typeof item === 'object') {
          if (typeof item.text === 'string') queue.push(item.text)
          if (item.content !== undefined) queue.push(item.content)
        }
      })
      continue
    }

    const row = normalizeKb(current)
    if (row) return [row]

    WRAPPER_KEYS.forEach((key) => {
      if (current[key] !== undefined) queue.push(current[key])
    })
  }

  return []
}
