import { AssistantRuntimeProvider } from '@assistant-ui/react'
import type { ArtifactRecord, ConversationMessage } from '../../conversations/model/types'
import { useDesktopConversation } from '../../conversations/hooks/use-desktop-conversation'
import type { RuntimeSource } from '../timeline/fox-runtime-process-model'
import type { GoalProgressData } from '../hooks/use-goal-progress'
import { TimelineRuntimeContext } from '../fox-shared'
import { useContext, useEffect, useMemo, useState } from 'react'
import FoxThread from './fox-thread'
import { useFoxRuntime, type FoxRuntimeCallbacks } from './use-fox-runtime'

type DesktopConversationState = ReturnType<typeof useDesktopConversation>

export interface AssistantUiChatProps {
  statusBar?: import('react').ReactNode
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
export default function AssistantUiChat({ statusBar, conversation, callbacks, pendingMessage, isSendDisabled, suggestions, onStart, planRevision, onOpenArtifact, onOpenFileInSidebar, onRevealFileInExplorer }: AssistantUiChatProps) {
  const runtime = useFoxRuntime(conversation, callbacks, { pendingMessage, isSendDisabled })
  // The workbench provides knowledge bindings and the open-source action through
  // this context for the whole chat stage; reuse them instead of new props.
  const timelineContext = useContext(TimelineRuntimeContext)
  const detail = conversation.detail
  // Tool rows and the process group read the raw run events; grouping once per
  // detail change keeps every message's event array reference stable.
  const events = useMemo(() => detail?.runtimeEvents ?? [], [detail])
  const eventsByRun = useMemo(() => {
    const grouped = new Map<string, typeof events>()
    for (const event of events) {
      const bucket = grouped.get(event.runId)
      if (bucket) bucket.push(event)
      else grouped.set(event.runId, [event])
    }
    return grouped
  }, [events])
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
    events,
    eventsByRun,
    attachmentNames,
    attachmentsByMessage,
    knowledgeBindings: timelineContext.knowledgeBindings,
    groupHelpers,
    onOpenSource: timelineContext.onOpenSource,
    onOpenArtifact,
    onOpenFileInSidebar,
    onRevealFileInExplorer,
  }), [events, eventsByRun, attachmentNames, attachmentsByMessage, timelineContext.knowledgeBindings, timelineContext.onOpenSource, groupHelpers, onOpenArtifact, onOpenFileInSidebar, onRevealFileInExplorer])
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <FoxThread statusBar={statusBar} fox={fox} suggestions={suggestions} onStart={onStart} planRevision={planRevision} turnRailMessages={detail?.messages} turnRailArtifacts={detail?.artifacts} />
    </AssistantRuntimeProvider>
  )
}
