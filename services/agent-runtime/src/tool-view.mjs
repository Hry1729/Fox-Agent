// Model-view bounding for large re-readable reference tool results.
//
// The durable result is never truncated: Host persistence keeps the full
// content, pairing, success/failure state and execution receipt. This only
// shapes the content projected into the model context so a long help index,
// document read or data sample cannot flood the input window. The model can
// always page the same controlled tool back with a narrower selector.

// Tools whose output is re-readable reference material rather than a one-shot
// fact. Execution/write/fact tools are never bounded.
const BOUNDABLE_TOOLS = new Set([
  'office_help',
  'office_read',
  'office_validate',
  'read',
  'read_attachment',
  'attachment_pages',
  'graph_read',
  'read_only_graph_read',
  'query_knowledge_graph',
  'list_mcp_tools',
  'call_mcp_tool',
])

// Reference text kept verbatim per tool result (UTF-8 bytes/characters are
// close enough for this bound; the Host enforces exact byte limits).
const HEAD_CHARS = 6_000
const TAIL_CHARS = 2_000
const MAX_CHARS = 9_000
const RECEIPT_MARKER = 'FOX_EXECUTION_RECEIPT_V1'

function utf8Bytes(value) {
  return Buffer.byteLength(value ?? '', 'utf8')
}

function cutHead(text, maxBytes) {
  if (utf8Bytes(text) <= maxBytes) return text
  const bytes = Buffer.from(text, 'utf8')
  return bytes.subarray(0, Math.min(maxBytes, bytes.length)).toString('utf8')
}

function cutTail(text, maxBytes) {
  if (utf8Bytes(text) <= maxBytes) return text
  const bytes = Buffer.from(text, 'utf8')
  return bytes.subarray(Math.max(0, bytes.length - maxBytes)).toString('utf8')
}

/**
 * Bound one text block. Returns the replacement text, or null when it must
 * pass through unchanged (small, receipt-bearing, or non-boundable tool).
 */
export function boundToolText(tool, { isError = false, text = '' } = {}) {
  if (isError || !BOUNDABLE_TOOLS.has(tool)) return null
  if (typeof text !== 'string' || utf8Bytes(text) <= MAX_CHARS) return null
  if (text.includes(RECEIPT_MARKER)) return null
  const head = cutHead(text, HEAD_CHARS)
  const tail = cutTail(text, TAIL_CHARS)
  const total = utf8Bytes(text)
  const omitted = Math.max(0, total - utf8Bytes(head) - utf8Bytes(tail))
  return [
    head,
    '',
    `[Fox 已为本条工具结果建立有界视图：原文约 ${total} 字节，模型上下文中仅保留开头 ${utf8Bytes(head)} 与结尾 ${utf8Bytes(tail)} 字节；完整结果已由 Host 完整持久化，不会丢失。需要被省略部分的精确内容时，用同一工具按需重新查询——office_help 用 property=<属性名> 或 page=<页码>，office_read/read 用更精确的 selector 或更小范围/分页，不要凭省略内容编造字段。]`,
    `…[省略约 ${omitted} 字节，可用同一工具按需取回]…`,
    '',
    tail,
  ].join('\n')
}

/**
 * Bound a settled result's content array for the model view. Returns the
 * possibly-replaced content; durable callers must keep the original result.
 */
export function boundToolResultContent(tool, { isError = false, content = [] } = {}) {
  if (isError || !BOUNDABLE_TOOLS.has(tool) || !Array.isArray(content)) return content
  let changed = false
  const next = content.map(block => {
    if (!block || block.type !== 'text') return block
    const bounded = boundToolText(tool, { isError, text: block.text })
    if (bounded === null) return block
    changed = true
    return { type: 'text', text: bounded }
  })
  return changed ? next : content
}
