import { toolFailurePresentation } from '../run-status-facts'
import { attachmentSourceNames, stepExtraSource } from '../step-source-summary'
import { Attachment } from '@/components/ai-elements/attachments'
import { type RunEventRecord } from '@/features/conversations/model/types'
import { normalizeKnowledgeSourceLocator } from '@/features/knowledge/knowledge-source-locator'
import { type KnowledgeSourceLocator } from '@/features/workspace/types'
import { FileEdit, FileText, FolderOpen, FolderSearch, Globe2, Search, Sparkles, Terminal, WandSparkles } from 'lucide-react'
import { isCommandTool } from '../composer/fox-tool-classification'

export type RuntimeToolStep = {
  id: string
  seq: number
  lastSeq: number
  name: string
  input: unknown
  output?: unknown
  isError: boolean
  completed: boolean
  awaitingUser: boolean
}

export type RuntimeSource = {
  id: string
  title: string
  knowledgeBaseId: string
  documentId: string
  excerpt: string
  locator: KnowledgeSourceLocator
}

export function compactValue(value: unknown, fallback = '') {
  if (typeof value === 'string') return value.trim()
  if (typeof value === 'number' || typeof value === 'boolean') return String(value)
  return fallback
}

function toolInput(tool: RuntimeToolStep) {
  return tool.input && typeof tool.input === 'object' && !Array.isArray(tool.input)
    ? tool.input as Record<string, unknown>
    : {}
}

/** The path-ish input a tool row exposes, e.g. `src/a.ts`. Only rows that
 *  actually carry a path may turn their subject into a clickable file name. */
function toolSubjectPath(tool: RuntimeToolStep) {
  const input = toolInput(tool)
  return compactValue(input.path || input.file || input.filePath || input.directory || input.dir || input.cwd)
}

type ToolCategory =
  | 'ask' | 'attachment' | 'compute' | 'office' | 'connector' | 'directory'
  | 'find' | 'search' | 'command' | 'web' | 'skill' | 'edit' | 'file' | 'data' | 'generic'

/** One family per tool, shared by the verb and the glyph: a row must never read
 *  as a file operation while showing the generic wand, or the other way round. */
function toolCategory(tool: RuntimeToolStep): ToolCategory {
  const name = tool.name.toLowerCase()
  if (name === 'ask_user_question') return 'ask'
  if (name === 'read_attachment') return 'attachment'
  if (name === 'attachment_compute') return 'compute'
  if (name === 'call_mcp_tool' || name.startsWith('office_')) return 'office'
  if (name === 'list_mcp_tools') return 'connector'
  if (/^(ls|ls_dir|list_dir|list_directory|list_files|dir)$/.test(name)) return 'directory'
  if (/^(find|glob)$/.test(name)) return 'find'
  if (/^(grep|search_content|search_files)$/.test(name)) return 'search'
  if (isCommandTool(name)) return 'command'
  if (/^(web_|http_|fetch|browser)/.test(name)) return 'web'
  if (/^skill_load/.test(name)) return 'skill'
  if (/^(edit|write|multiedit|apply_patch|str_replace|create|patch)/.test(name)) return 'edit'
  if (/^(read|view|open)_?(file)?$/.test(name)) return 'file'
  if (/read|query|snapshot|status|info/.test(name)) return 'data'
  return 'generic'
}

/** Attachment and artifact references an input may carry, in declaration order. */
function attachmentIds(input: Record<string, unknown>) {
  const single = compactValue(input.attachmentId || input.attachment_id || input.artifactId || input.artifact_id)
  const list = input.attachmentIds ?? input.attachment_ids ?? input.artifactIds ?? input.artifact_ids ?? input.attachments ?? input.artifacts
  const ids: string[] = single ? [single] : []
  if (Array.isArray(list)) for (const entry of list) {
    if (typeof entry === 'string' && entry.trim()) ids.push(entry.trim())
    else if (entry && typeof entry === 'object') {
      const record = entry as Record<string, unknown>
      const id = compactValue(record.id || record.attachmentId || record.artifactId)
      if (id) ids.push(id)
    }
  }
  return [...new Set(ids)]
}

/** The name an attachment tool acts on. The model usually passes only an id, so
 *  the settled result's `displayName` and the conversation's attachment list are
 *  consulted. An id neither of them can name contributes nothing: a shortened
 *  internal identifier is not a file name and must never reach the row. */
function attachmentSubject(tool: RuntimeToolStep, attachmentNames?: ReadonlyMap<string, string>) {
  const input = toolInput(tool)
  const named = compactValue(input.name || input.title || input.attachmentName || input.fileName || input.displayName)
  if (named) return named
  const output = tool.output
  if (output && typeof output === 'object' && !Array.isArray(output)) {
    const record = output as Record<string, unknown>
    const details = record.details && typeof record.details === 'object' && !Array.isArray(record.details)
      ? record.details as Record<string, unknown>
      : record
    const fromResult = compactValue(details.displayName || details.name || details.title)
    if (fromResult) return fromResult
  }
  const ids = attachmentIds(input)
  if (!ids.length) return ''
  const resolved = ids.map((id) => attachmentNames?.get(id)).filter((value): value is string => Boolean(value))
  if (resolved.length > 1) return `${resolved.length} 个附件`
  if (resolved.length === 1) return resolved[0]
  // Counted, never named: how many attachments a call touched is a fact, the id
  // itself is an internal handle the reader has no use for.
  return ids.length > 1 ? `${ids.length} 个附件` : ''
}

/** `call_mcp_tool` hides the real Office tool inside its payload, so the row
 *  names what actually ran instead of the transport wrapper. */
function officeToolSubject(tool: RuntimeToolStep) {
  const input = toolInput(tool)
  const inner = compactValue(input.tool || input.toolName)
  if (inner) return inner
  if (tool.name.toLowerCase() === 'call_mcp_tool') return compactValue(input.server || input.serverName || input.serverId)
  return tool.name
}

/** One row's line, split into independent slots.
 *
 *  Interaction is decided on the slots themselves — "is there a path and a
 *  handler?" — and never by reading the rendered string back. Appending a source
 *  therefore cannot take the path's click away, and cannot reorder the line. */
type RuntimeStepSlots = {
  /** The verb/action name only: never the path and never the source. */
  action: string
  /** The file or directory the call names, when the call names one. */
  path: string
  /** The real source: a worksheet, a page title/host, the conversation's name for
   *  an uploaded attachment. Never an internal id, argument body or diagnostic. */
  source: string
  /** A supplementary one-line summary: a query, a command, a URL, a tool name. */
  summary: string
}

/** The flat wording of a slotted row — the accessible text and the line a caller
 *  that renders plain text (the collapsed process title) still needs. */
export function stepSlotsText(slots: RuntimeStepSlots) {
  return [slots.action, slots.path, slots.source, slots.summary].filter(Boolean).join(' · ')
}

/** One verbed line per tool call, split into its slots, e.g.
 *  `读取文件 · src/a.ts · 工作表「明细」`. The action slot is the verb alone; the
 *  path slot is the path the reader can open; the source slot is evidence from the
 *  call's own fields or its result, resolved through the conversation's own tables. */
export function toolRowSlots(tool: RuntimeToolStep, attachmentNames?: ReadonlyMap<string, string>): RuntimeStepSlots {
  const input = toolInput(tool)
  const path = toolSubjectPath(tool)
  const query = compactValue(input.pattern || input.query || input.search)
  const command = compactValue(input.command || input.cmd || input.script)
  const url = compactValue(input.url || input.href || input.link)
  const category = toolCategory(tool)
  const extraSource = stepExtraSource(tool.name, tool.input, tool.output)
  // An attachment id is resolved to the conversation's own name for it; an id that
  // cannot be resolved contributes nothing rather than surfacing as an internal id.
  const attachmentSource = attachmentSourceNames(tool.input, attachmentNames)
  if (category === 'ask') return { action: tool.awaitingUser ? '等待你的回答' : '正在向你提问', path: '', source: '', summary: '' }
  if (tool.isError) {
    // The same whitelist sentence the expanded row shows. `failure.operation`,
    // `target` and `summary` are already display facts; the raw diagnostic body
    // stays in the durable record and is never transcribed into any slot.
    const failure = toolFailurePresentation(tool.name, tool.output)
    return {
      action: '工具调用未成功',
      path,
      source: extraSource || attachmentSource,
      summary: [failure.operation, failure.target, failure.summary].filter(Boolean).join(' · '),
    }
  }
  // Attached-workbook calls name their file: a worksheet the call reported, a
  // project path the caller named, or the conversation's name for the upload.
  const attachment = category === 'attachment' || category === 'compute'
    ? attachmentSubject(tool, attachmentNames)
    : ''
  const source = extraSource || attachmentSource || attachment
  // Named tools stay verbed so every call reads as its own status line.
  switch (category) {
    case 'attachment':
      return { action: tool.completed ? '读取附件' : '正在读取附件', path, source, summary: '' }
    case 'compute':
      return { action: tool.completed ? '计算附件数据' : '正在计算附件数据', path, source, summary: '' }
    case 'office':
      return { action: tool.completed ? '调用工具' : '正在调用', path, source, summary: officeToolSubject(tool) }
    case 'connector':
      return { action: '列出连接器工具', path: '', source, summary: '' }
    case 'directory':
      return { action: tool.completed ? '查看目录' : '正在查看目录', path, source, summary: '' }
    case 'find':
      return { action: tool.completed ? '查找文件' : '正在查找', path, source, summary: query }
    case 'search':
      return { action: tool.completed ? '搜索内容' : '正在搜索', path, source, summary: query }
    case 'command':
      return { action: tool.completed ? '执行命令' : '正在执行', path, source, summary: command }
    case 'web':
      return { action: tool.completed ? '访问网页' : '正在访问', path, source, summary: url }
    case 'skill':
      return { action: tool.completed ? '加载技能' : '正在加载技能', path, source, summary: query || command || url }
    case 'file':
      return { action: tool.completed ? '读取文件' : '正在读取', path, source, summary: query || command || url }
    case 'edit':
      return { action: tool.completed ? '已编辑' : '正在编辑', path, source, summary: query || command || url }
    case 'data':
      return { action: tool.completed ? '读取数据' : '正在读取', path, source, summary: query || command || url }
    default:
      // Generic tools read as `调用工具 · <name>` (single separator, like 深度思考 · …).
      return { action: tool.completed ? '调用工具' : '正在调用', path, source, summary: tool.name }
  }
}

export function toolActivity(tool: RuntimeToolStep, attachmentNames?: ReadonlyMap<string, string>) {
  return stepSlotsText(toolRowSlots(tool, attachmentNames))
}

/** The glyph follows the row's verb family: files stay files and commands stay
 *  commands, while Office writes and generic tool calls share the wand. */
export function toolIcon(tool: RuntimeToolStep) {
  switch (toolCategory(tool)) {
    case 'attachment':
    case 'file':
      return FileText
    case 'directory':
      return FolderOpen
    case 'find':
      return FolderSearch
    case 'search':
      return Search
    case 'command':
      return Terminal
    case 'web':
      return Globe2
    case 'skill':
      return Sparkles
    case 'edit':
      return FileEdit
    default:
      return WandSparkles
  }
}

type RuntimeProcessStep =
  | { kind: 'reasoning'; id: string; seq: number; text: string }
  | { kind: 'tool'; seq: number; id: string }

export function runtimeProcess(events: RunEventRecord[]) {
  const reasoningGroups: Array<{ source: string; text: string; seq: number; lastSeq: number }> = []
  const tools = new Map<string, RuntimeToolStep>()
  const sources = new Map<string, RuntimeSource>()
  // A tool call closes the current thought: reasoning before and after it must
  // render as separate rows (think → tool → think), like other agent harnesses,
  // instead of being merged into one long reasoning block. Anything else that
  // interleaves the stream (`run.phase`, `usage.updated`, …) must not split a
  // thought, so `seq` continuity is deliberately not part of the test.
  let reasoningBoundary = false
  for (const item of events) {
    if (item.eventType === 'reasoning.delta' && typeof item.event.delta === 'string') {
      const source = typeof item.event.source === 'string' ? item.event.source : 'provider'
      const previous = reasoningBoundary ? undefined : reasoningGroups.at(-1)
      if (previous?.source === source) {
        // Same source, no tool in between: one continuous thought stays one row.
        previous.text += item.event.delta
        previous.lastSeq = item.seq
      } else {
        reasoningGroups.push({ source, text: item.event.delta, seq: item.seq, lastSeq: item.seq })
      }
      reasoningBoundary = false
      continue
    }
    if (item.eventType === 'source.added') {
      const id = compactValue(item.event.id, `source-${item.seq}`)
      sources.set(id, {
        id,
        title: compactValue(item.event.title, '资料库来源'),
        knowledgeBaseId: compactValue(item.event.knowledgeBaseId),
        documentId: compactValue(item.event.documentId),
        excerpt: compactValue(item.event.excerpt),
        locator: normalizeKnowledgeSourceLocator(item.event, id),
      })
      continue
    }
    if (!item.eventType.startsWith('tool.')) continue
    // Any tool call ends the current thought so subsequent deltas form a new row.
    reasoningBoundary = true
    const toolCallId = typeof item.event.toolCallId === 'string' ? item.event.toolCallId : `tool-${item.seq}`
    const current = tools.get(toolCallId) ?? {
      id: toolCallId,
      seq: item.seq,
      lastSeq: item.seq,
      name: typeof item.event.tool === 'string' ? item.event.tool : 'tool',
      input: item.event.input ?? {},
      isError: false,
      completed: false,
      awaitingUser: false,
    }
    current.lastSeq = item.seq
    if (item.event.input !== undefined) current.input = item.event.input
    if (item.eventType === 'tool.completed') {
      current.output = item.event.result
      current.isError = item.event.isError === true
      current.completed = true
      current.awaitingUser = Boolean(
        item.event.result
        && typeof item.event.result === 'object'
        && (item.event.result as Record<string, unknown>).status === 'awaiting_user'
      )
    }
    tools.set(toolCallId, current)
  }
  const reasoningItems = reasoningGroups
    .map((group) => group.text.trim())
    .filter(Boolean)
  const reasoning = reasoningItems.join('\n\n')
  const sortedTools = [...tools.values()].sort((left, right) => left.seq - right.seq)
  // One chronological step per thought segment and per tool call.
  const steps: RuntimeProcessStep[] = [
    ...reasoningGroups
      .filter((group) => group.text.trim())
      .map((group) => ({ kind: 'reasoning' as const, id: `${group.source}:${group.seq}`, seq: group.seq, text: group.text.trim() })),
    ...sortedTools.map((tool) => ({ kind: 'tool' as const, seq: tool.seq, id: tool.id })),
  ].sort((left, right) => left.seq - right.seq)
  return {
    reasoning,
    reasoningItems,
    steps,
    tools: sortedTools,
    sources: [...sources.values()],
  }
}
