import { AssistantRuntimeProvider, ComposerPrimitive, ThreadPrimitive } from '@assistant-ui/react'
import type { ArtifactRecord, ConversationMessage } from '../../conversations/model/types'
import { useDesktopConversation } from '../../conversations/hooks/use-desktop-conversation'
import type { RuntimeSource } from '../timeline/fox-runtime-process-model'
import type { GoalProgressData } from '../hooks/use-goal-progress'
import { TimelineRuntimeContext } from '../fox-shared'
import { Button } from '@/components/ui/button'
import { ArrowUp, CircleStop } from 'lucide-react'
import { useContext, useEffect, useMemo, useState } from 'react'
import FoxThread from './fox-thread'
import { useFoxRuntime, type FoxRuntimeCallbacks } from './use-fox-runtime'

type DesktopConversationState = ReturnType<typeof useDesktopConversation>

export interface AssistantUiChatProps {
  conversation: DesktopConversationState
  callbacks: FoxRuntimeCallbacks
  pendingMessage?: ConversationMessage | null
  isSendDisabled?: boolean
  /** Opening suggestions for the empty state (expert-defined). */
  suggestions?: string[]
  /** Latest plan revision for the bottom-dock floater. */
  planRevision?: GoalProgressData['planRevisions'][number]
  onOpenArtifact?: (artifact: ArtifactRecord) => void
  onOpenFileInSidebar?: (path: string) => void
  onRevealFileInExplorer?: (path: string) => void
}

/**
 * The assistant-ui chat branch (migration step 5C). Mounted only while the
 * `fox.ui.chatEngine` switch is set to `assistant-ui`, and loaded as its own
 * lazy chunk so the legacy path and the first-screen budget stay untouched.
 *
 * This container owns the runtime and the Fox thread context (raw run events,
 * attachment names, host actions) plus the interim composer; the message list
 * lives in `fox-thread.tsx` and the composer moves to `thread/composer/` in
 * step 5D.
 */
export default function AssistantUiChat({ conversation, callbacks, pendingMessage, isSendDisabled, suggestions, planRevision, onOpenArtifact, onOpenFileInSidebar, onRevealFileInExplorer }: AssistantUiChatProps) {
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
      <FoxThread fox={fox} suggestions={suggestions} planRevision={planRevision} turnRailMessages={detail?.messages} turnRailArtifacts={detail?.artifacts} />
      <ThreadPrimitive.If running={false}>
        <ComposerPrimitive.Root className="fox-thread-composer">
          <ComposerPrimitive.Input className="fox-thread-composer-input" placeholder="发送消息…" rows={1} autoFocus />
          <ComposerPrimitive.Send className="fox-thread-composer-send" aria-label="发送"><ArrowUp size={16} /></ComposerPrimitive.Send>
        </ComposerPrimitive.Root>
      </ThreadPrimitive.If>
      <ThreadPrimitive.If running>
        <ComposerPrimitive.Root className="fox-thread-composer is-running">
          <ComposerPrimitive.Input className="fox-thread-composer-input" placeholder="Fox 正在回复…" rows={1} />
          <ComposerPrimitive.Cancel className="fox-thread-composer-cancel" aria-label="停止"><CircleStop size={16} /></ComposerPrimitive.Cancel>
        </ComposerPrimitive.Root>
      </ThreadPrimitive.If>
    </AssistantRuntimeProvider>
  )
}

/** Temporary engine switch for the development rounds; removed when the
 * switch default flips in 5E. */
export function ChatEngineSwitch({ engine, onChange }: { engine: 'legacy' | 'assistant-ui'; onChange: (engine: 'legacy' | 'assistant-ui') => void }) {
  return (
    <Button variant="ghost" size="sm" className="fox-thread-engine-switch" onClick={() => onChange(engine === 'legacy' ? 'assistant-ui' : 'legacy')}>
      {engine === 'legacy' ? '试用 assistant-ui 界面' : '返回经典界面'}
    </Button>
  )
}
