import { AssistantRuntimeProvider } from '@assistant-ui/react'
import type { ApprovalDecision, ArtifactRecord, ConversationMessage } from '../../conversations/model/types'
import { useContacts } from '@/features/companions/context'
import { splitAssistantContent } from '../timeline/fox-assistant-content'
import { useDesktopConversation } from '../../conversations/hooks/use-desktop-conversation'
import { type RuntimeSource } from '../timeline/fox-runtime-process-model'
import type { GoalProgressData } from '../hooks/use-goal-progress'
import { TimelineRuntimeContext } from '../fox-shared'
import { useContext, useEffect, useMemo, useState } from 'react'
import FoxThread from './fox-thread'
import { useFoxRuntime, type FoxRuntimeCallbacks } from './use-fox-runtime'

type DesktopConversationState = ReturnType<typeof useDesktopConversation>

export interface AssistantUiChatProps {
  onResolveApproval?: (id: string, decision: ApprovalDecision) => void | Promise<boolean>
  onSubmitQuestion?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>
  statusBar?: import('react').ReactNode
  /** 待处理的审批卡（UI 1104-07 移入对话流）。 */
  approvalSlot?: import('react').ReactNode
  conversation: DesktopConversationState
  callbacks: FoxRuntimeCallbacks
  pendingMessage?: ConversationMessage | null
  isSendDisabled?: boolean
  /** Opening suggestions for the empty state (expert-defined). */
  suggestions?: string[]
  /** Fills the composer draft when an empty-state suggestion is clicked. */
  onStart?: (suggestion: string) => void
  /** Latest plan revision for the bottom-dock floater. */
  planRevision?: GoalProgressData['planRevisions'][number]
  onOpenArtifact?: (artifact: ArtifactRecord) => void
  onOpenFileInSidebar?: (path: string) => void
  onRevealFileInExplorer?: (path: string) => void
}

/**
 * The assistant-ui message list (migration step 5C). It is the only chat path;
 * loaded as its own lazy chunk so the first-screen budget stays untouched.
 *
 * After the hybrid rollback this container renders ONLY the message list: the
 * empty state and the composer are the classic legacy components again, mounted
 * by the workbench around this thread. The runtime (external store over
 * `useDesktopConversation`) stays here, so legacy-composer sends still stream
 * into the assistant-ui message list.
 */
export default function AssistantUiChat({ statusBar, approvalSlot, conversation, callbacks, pendingMessage, isSendDisabled, suggestions, onStart, planRevision, onOpenArtifact, onOpenFileInSidebar, onRevealFileInExplorer, onResolveApproval, onSubmitQuestion }: AssistantUiChatProps) {
  const companion = useContacts()
  const runtime = useFoxRuntime(conversation, callbacks, { pendingMessage, isSendDisabled })
  // The workbench provides knowledge bindings and the open-source action through
  // this context for the whole chat stage; reuse them instead of new props.
  const timelineContext = useContext(TimelineRuntimeContext)
  const detail = conversation.detail
  // Tool rows and the process group read the raw run events; grouping once per
  // detail change keeps every message's event array reference stable.
  const events = useMemo(() => detail?.runtimeEvents ?? [], [detail])
  const toolCalls = detail?.toolCalls
  const eventsByRun = useMemo(() => {
    const grouped = new Map<string, typeof events>()
    for (const event of events) {
      const bucket = grouped.get(event.runId)
      if (bucket) bucket.push(event)
      else grouped.set(event.runId, [event])
    }
    return grouped
  }, [events])
  // Committed contact bubbles are the authority for this window. Live preview
  // bubbles keep rendering from run events until the run settles.
  const contactBubblesByRun = useMemo(() => {
    const grouped = new Map<string, NonNullable<typeof detail>['bubbles']>()
    for (const bubble of detail?.bubbles ?? []) {
      if (bubble.author !== 'companion') continue
      const key = bubble.runId ?? bubble.messageId
      const bucket = grouped.get(key)
      if (bucket) bucket.push(bubble)
      else grouped.set(key, [bubble])
    }
    return grouped as Map<string, import('../../conversations/model/types').ContactBubbleRecord[]>
  }, [detail])
  // Live Kernel previews already carry the durable bubble identity, so the
  // settled row (and later the committed Rust bubble) never remounts them.
  const contactKernelPreviewsByRun = useMemo(() => {
    const grouped = new Map<string, { id: string; text: string; createdAt: number }[]>()
    for (const message of detail?.messages ?? []) {
      if (!message.kernelPreview || !message.runId || message.role !== 'assistant') continue
      const text = splitAssistantContent(message.content).answer
      if (!text.trim()) continue
      const entry = { id: `${message.runId}:kernel:${message.kernelPreview.checkpointSeq}:response`, text, createdAt: message.createdAt }
      const bucket = grouped.get(message.runId)
      if (bucket) bucket.push(entry)
      else grouped.set(message.runId, [entry])
    }
    return grouped
  }, [detail])
  // C07: a user message is markable only once its own bubble is committed; an
  // attachment-only message has no bubble at all.
  const contactUserBubbleIds = useMemo(() => new Set((detail?.bubbles ?? []).filter((bubble) => bubble.author === 'user').map((bubble) => bubble.id)), [detail])
  // C07: the user's own marks and the quotes of the loaded window, keyed by the
  // durable identity they address (a user bubble's id is its message id).
  const contactBubbleMarks = useMemo(() => new Map((detail?.bubbleMarks ?? []).map((mark) => [mark.bubbleId, mark])), [detail])
  const contactMessageQuotes = useMemo(() => new Map((detail?.messageQuotes ?? []).map((quote) => [quote.messageId, quote])), [detail])
  const attachments = useMemo(() => detail?.attachments ?? [], [detail])
  const attachmentNames = useMemo(() => new Map(attachments.map((attachment) => [attachment.id, attachment.displayName])), [attachments])
  const attachmentsByMessage = useMemo(() => {
    const grouped = new Map<string, typeof attachments>()
    for (const attachment of attachments) {
      if (!attachment.messageId) continue
      const bucket = grouped.get(attachment.messageId)
      if (bucket) bucket.push(attachment)
      else grouped.set(attachment.messageId, [attachment])
    }
    return grouped
  }, [attachments])
  // Same lazy load as the legacy timeline: grouping helpers arrive only when
  // the conversation actually carries runtime events.
  const [groupHelpers, setGroupHelpers] = useState<typeof import('../runtime-process-groups') | null>(null)
  const hasRuntimeEvents = events.length > 0
  useEffect(() => {
    if (!hasRuntimeEvents) return
    let mounted = true
    void import('../runtime-process-groups').then((helpers) => { if (mounted) setGroupHelpers(helpers) })
    return () => { mounted = false }
  }, [hasRuntimeEvents])
  const fox = useMemo(() => ({
    contactChat: companion?.privateChat && detail && detail.conversation.id === companion.current?.conversationId
      ? { detail, onResolveApproval, onSubmitQuestion } : undefined,
    events,
    eventsByRun,
    contactBubblesByRun,
    contactKernelPreviewsByRun,
    contactUserBubbleIds,
    contactBubbleMarks,
    contactMessageQuotes,
    toolCalls,
    attachmentNames,
    attachmentsByMessage,
    knowledgeBindings: timelineContext.knowledgeBindings,
    groupHelpers,
    onOpenSource: timelineContext.onOpenSource,
    onOpenArtifact,
    onOpenFileInSidebar,
    onRevealFileInExplorer,
  }), [companion?.privateChat, companion?.current?.conversationId, detail, onResolveApproval, onSubmitQuestion, events, eventsByRun, contactBubblesByRun, contactKernelPreviewsByRun, contactUserBubbleIds, contactBubbleMarks, contactMessageQuotes, toolCalls, attachmentNames, attachmentsByMessage, timelineContext.knowledgeBindings, timelineContext.onOpenSource, groupHelpers, onOpenArtifact, onOpenFileInSidebar, onRevealFileInExplorer])
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <FoxThread statusBar={statusBar} approvalSlot={approvalSlot} fox={fox} suggestions={suggestions} onStart={onStart} planRevision={planRevision} turnRailMessages={detail?.messages} turnRailArtifacts={detail?.artifacts} />
    </AssistantRuntimeProvider>
  )
}

