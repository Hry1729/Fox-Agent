import {
  BookOpen,
  Bot,
  Calculator,
  CheckSquare,
  Database,
  FileEdit,
  FilePen,
  FileText,
  Folder,
  FolderOutput,
  FolderSearch,
  Globe,
  HelpCircle,
  Image,
  Network,
  SquareTerminal
} from 'lucide-vue-next'

export const TOOL_ICON_MAP = {
  ask_user_question: HelpCircle,
  bash: SquareTerminal,
  calculator: Calculator,
  cmd: SquareTerminal,
  edit_file: FilePen,
  execute: SquareTerminal,
  find_kb_document: FolderSearch,
  get_mindmap: Network,
  glob: FolderSearch,
  grep: FolderSearch,
  list_directory: Folder,
  list_kbs: BookOpen,
  ls: Folder,
  mysql_describe_table: Database,
  mysql_list_tables: Database,
  mysql_query: Database,
  open_kb_document: FileText,
  present_artifacts: FolderOutput,
  query_kb: BookOpen,
  read_file: FileText,
  replace: FilePen,
  run_shell_command: SquareTerminal,
  search_file: FolderSearch,
  search_file_content: FolderSearch,
  task: Bot,
  tavily_search: Globe,
  text_to_img_qwen_image: Image,
  write_file: FileEdit,
  write_todos: CheckSquare
}

// Keep intentionally hidden tool calls centralized so group summaries and renderers stay consistent.
export const HIDDEN_TOOL_CALL_IDS = ['present_artifacts']

export const getToolCallId = (toolCall) => toolCall?.name || toolCall?.function?.name || ''

export const isHiddenToolCall = (toolCall) => HIDDEN_TOOL_CALL_IDS.includes(getToolCallId(toolCall))

export const isValidToolCall = (toolCall) => {
  return Boolean(
    toolCall &&
    (toolCall.id || toolCall.name || toolCall.function?.name) &&
    (toolCall.args !== undefined ||
      toolCall.function?.arguments !== undefined ||
      toolCall.tool_call_result !== undefined)
  )
}

export const parseToolCallArgs = (toolCall) => {
  const args = toolCall?.args ?? toolCall?.function?.arguments
  if (!args) return {}
  if (typeof args === 'object') return args
  try {
    return JSON.parse(args)
  } catch {
    return {}
  }
}

export const enrichTaskToolCall = (
  toolCall,
  { subagentRunById, subagentRunByThreadId, subagentOptionBySlug } = {}
) => {
  if (getToolCallId(toolCall) !== 'task') return toolCall

  const args = parseToolCallArgs(toolCall)
  const subagentRun =
    (toolCall.id ? subagentRunById?.get?.(String(toolCall.id)) : null) ||
    (args.thread_id ? subagentRunByThreadId?.get?.(String(args.thread_id)) : null)
  const subagentOption = args.subagent_type
    ? subagentOptionBySlug?.get?.(String(args.subagent_type))
    : null
  const displayLabel =
    subagentRun?.subagent_name || subagentOption?.name || subagentRun?.subagent_type || undefined

  return {
    ...toolCall,
    ...(subagentRun ? { subagent_run: subagentRun } : {}),
    ...(displayLabel ? { display_label: displayLabel } : {})
  }
}

export const normalizeToolCalls = (toolCalls, { includeHidden = false, mapToolCall } = {}) => {
  if (!Array.isArray(toolCalls)) return []

  return toolCalls
    .filter((toolCall) => {
      if (!isValidToolCall(toolCall)) return false
      return includeHidden || !isHiddenToolCall(toolCall)
    })
    .map((toolCall) => (mapToolCall ? mapToolCall(toolCall) : toolCall))
}

export const enrichTaskToolCalls = (toolCalls, options = {}) =>
  normalizeToolCalls(toolCalls, {
    mapToolCall: (toolCall) => enrichTaskToolCall(toolCall, options)
  })

export const getToolIcon = (toolId) => TOOL_ICON_MAP[toolId] || null
