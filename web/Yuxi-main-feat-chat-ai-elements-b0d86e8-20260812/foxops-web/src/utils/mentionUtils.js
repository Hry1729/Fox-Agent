/**
 * Mention 工具（搬自 Yuxi utils/mention_utils.js，去掉 getDisplayFileName 依赖）
 * mention token 格式: @file:path / @knowledge:kb_id / @mcp:slug / @skill:slug / @subagent:id
 * 含空格或特殊字符的值用引号: @file:"path with spaces.txt"
 */

export const mentionTypePrefixMap = {
  file: 'file',
  knowledge: 'knowledge',
  mcp: 'mcp',
  skill: 'skill',
  subagent: 'subagent'
}

const mentionTypePattern = Object.values(mentionTypePrefixMap).join('|')
const mentionTokenRegex = new RegExp(
  `@(${mentionTypePattern}):(?:"((?:\\\\.|[^"\\\\])*)"|(\\S+))`,
  'g'
)

const quoteMentionValue = (value) =>
  String(value ?? '')
    .replace(/\\/g, '\\\\')
    .replace(/"/g, '\\"')
const unquoteMentionValue = (value) => String(value ?? '').replace(/\\(["\\])/g, '$1')

export const formatMentionToken = (type, value) => {
  const prefix = mentionTypePrefixMap[type] || type
  const rawValue = String(value ?? '')
  if (/\s|["\\]/.test(rawValue)) {
    return `@${prefix}:"${quoteMentionValue(rawValue)}"`
  }
  return `@${prefix}:${rawValue}`
}

export const parseMentionText = (text = '') => {
  const value = String(text || '')
  const segments = []
  let lastIndex = 0

  mentionTokenRegex.lastIndex = 0
  for (const match of value.matchAll(mentionTokenRegex)) {
    const raw = match[0]
    const start = match.index ?? 0
    const end = start + raw.length

    if (start > lastIndex) {
      segments.push({ kind: 'text', text: value.slice(lastIndex, start), start: lastIndex, end: start })
    }

    const type = match[1]
    const quotedValue = match[2]
    const rawValue = match[3]
    segments.push({
      kind: 'mention',
      raw,
      type,
      value: quotedValue !== undefined ? unquoteMentionValue(quotedValue) : rawValue,
      start,
      end
    })
    lastIndex = end
  }

  if (lastIndex < value.length) {
    segments.push({ kind: 'text', text: value.slice(lastIndex), start: lastIndex, end: value.length })
  }

  return segments
}

/**
 * 在 text 的 caretOffset 位置查找活跃的 @提及查询。
 * 返回 { start, end, query } 或 null。
 */
export const findActiveMentionQuery = (text = '', caretOffset = 0) => {
  const value = String(text || '')
  const offset = Math.max(0, Math.min(caretOffset, value.length))

  // 如果光标在已有 mention token 内部，不触发新查询
  const segments = parseMentionText(value)
  const mentionAtCursor = segments.find(
    (s) => s.kind === 'mention' && offset > s.start && offset <= s.end
  )
  if (mentionAtCursor) return null

  const textBeforeCursor = value.slice(0, offset)
  const atIndex = textBeforeCursor.lastIndexOf('@')
  if (atIndex === -1) return null

  // @ 前必须是空白或行首
  if (atIndex > 0 && !/\s/.test(value[atIndex - 1])) return null

  const query = textBeforeCursor.slice(atIndex + 1)
  // 查询中不能包含 @ : 或空白
  if (query.includes('@') || query.includes(':') || /\s/.test(query)) return null

  return { start: atIndex, end: offset, query }
}

export const replaceRawRange = (text = '', start = 0, end = start, replacement = '') => {
  const value = String(text || '')
  const safeStart = Math.max(0, Math.min(start, value.length))
  const safeEnd = Math.max(safeStart, Math.min(end, value.length))
  return value.slice(0, safeStart) + replacement + value.slice(safeEnd)
}

export const getMentionDisplayLabel = (type, value) => {
  if (type === 'file') {
    const parts = String(value ?? '').split('/')
    return parts[parts.length - 1] || value
  }
  return String(value ?? '').trim() || type
}
