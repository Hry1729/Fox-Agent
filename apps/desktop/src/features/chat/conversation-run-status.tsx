import { conversationAttentionLabel, type ConversationRunIndicator } from './conversation-run-indicator'
import { ConversationStatusIcon } from './conversation-status-icon'

export const CONVERSATION_RUN_LABELS: Record<ConversationRunIndicator, string> = {
  running: '运行中',
  queued: '运行中',
  approval: '等你处理',
  reply: '等待审批或回复',
  completed: '任务结束',
}

/**
 * The activity slot of one sidebar row. It is always laid out so a conversation
 * starting or finishing a run never shifts the rows around it, and it reports the
 * state as a `status` only when there is something to announce.
 */
export function ConversationRunStatus({ indicator, className = 'fox-sidebar-conversation-status' }: { indicator?: ConversationRunIndicator; className?: string }) {
  const label = indicator ? CONVERSATION_RUN_LABELS[indicator] : undefined
  const state = !indicator ? 'idle' : indicator === 'approval' || indicator === 'reply' ? 'waiting' : indicator === 'completed' ? 'complete' : 'running'
  return <span
    className={`${className}${indicator ? ` is-${indicator}` : ''}`}
    role={label ? 'status' : undefined}
    aria-label={label}
    title={label}
    data-run-indicator={indicator ?? undefined}
  >
    <ConversationStatusIcon state={state} size={9.8} />
  </span>
}

/** Small text badge after the title when the conversation waits for the user's approval. */
export function ConversationAttentionBadge({ indicator }: { indicator?: ConversationRunIndicator }) {
  const label = conversationAttentionLabel(indicator)
  if (!label) return null
  return <span className="fox-sidebar-attention" data-attention="approval" title="有待处理的审批">{label}</span>
}
