import { deriveRunTerminalStates } from '../../conversations/model/run-failure'
import { ExpertActivationCard, type ExpertBindingView } from '../components/ExpertBindingChip'
import { UserMessageBubble } from '../components/UserMessageBubble'
import { mergeExpertBindingsIntoTimeline } from '../components/expert-binding-ui'
import { type GoalProgressData } from '../hooks/use-goal-progress'
import { EMPTY_RUNTIME_ARTIFACTS, EMPTY_RUNTIME_EVENTS, groupAssistantContinuations, groupRuntimeRecords, latestRunAssistantId } from '../runtime-timeline-performance'
import { Attachment, AttachmentInfo, AttachmentPreview, Attachments } from '@/components/ai-elements/attachments'
import { Conversation, ConversationAutoScroll, ConversationContent, ConversationViewportState } from '@/components/ai-elements/conversation'
import { Message, MessageAction, MessageContent } from '@/components/ai-elements/message'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { FoxThinkingDots } from '../components/FoxThinkingDots'
import { EditMessage } from '@/components/assistant-ui/elements/edit-message'
import { File as MessageFile } from '@/components/assistant-ui/elements/file'
import { splitAssistantContent } from './fox-assistant-content'
import { type AgentRecord, type ArtifactRecord, type AttachmentRecord, type ConversationMessage, type DesktopErrorDetails, type RunEventRecord, type RunRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { ArrowDown, ArrowUp, Copy, FileEdit, GitFork, ListTodo, LoaderCircle } from 'lucide-react'
import { useContext, useEffect, useMemo, useRef, useState } from 'react'
import { useStickToBottomContext } from 'use-stick-to-bottom'
import { ErrorPrompt } from '../demo/fox-demo-prompts'
import { type ChatState, EMPTY_CONVERSATION_ATTACHMENTS, TimelineRuntimeContext, copyTextWithFeedback, requestConversationFork } from '../fox-shared'
import { MemoizedRuntimeAssistantMessage, type RuntimeGroupHelpers } from './fox-runtime-assistant-message'
import { type RuntimeSource } from './fox-runtime-process-model'
import { ConversationTurnRail } from './fox-timeline'

// Re-exported from the pure module so existing imports keep working.
export { splitAssistantContent } from './fox-assistant-content'

function SentMessageAttachments({ attachments }: { attachments: AttachmentRecord[] }) {
  if (attachments.length === 0) return null
  return <Attachments variant="inline" className="fox-message-attachments">
    {attachments.map((attachment) => (
      <MessageFile key={attachment.id} name={attachment.displayName} mediaType={attachment.mediaType} bytes={attachment.byteSize} />
    ))}
  </Attachments>
}

/** 底栏里的滚动到底按钮。思考中显示点阵、空闲显示箭头；类名沿用 ai-elements 版本，
 *  这样深色模式底色不会因为换了实现而偏移。 */
function FoxConversationScrollButton({ running }: { running: boolean }) {
  const { isAtBottom, scrollToBottom } = useStickToBottomContext()
  if (isAtBottom) return null
  return (
    <Button
      className={cn(
        'absolute bottom-4 left-[50%] translate-x-[-50%] rounded-full dark:bg-background dark:hover:bg-muted',
        'fox-scroll-button'
      )}
      onClick={() => scrollToBottom()}
      size="icon"
      type="button"
      variant="outline"
      aria-label="定位到最新消息"
    >
      {running ? <FoxThinkingDots /> : <ArrowDown className="size-4" />}
    </Button>
  )
}

export function ConversationBottomDock({ plan, running = false }: { plan?: GoalProgressData['planRevisions'][number]; running?: boolean }) {
  const planStatus = plan?.status === 'proposed'
    ? '待批准'
    : plan?.status === 'approved'
      ? '已批准'
      : plan?.status === 'rejected'
        ? '已退回'
        : '已更新'
  return (
    <div className={`fox-conversation-bottom-dock ${plan ? 'has-plan' : ''}`}>
      <FoxConversationScrollButton running={running} />
      {plan && <div className={`fox-plan-floater is-${plan.status}`} role="status" title={[plan.title, plan.summary].filter(Boolean).join('\n')}>
        <ListTodo size={12} />
        <strong>{plan.title || `计划 v${plan.revision}`}</strong>
        <span>v{plan.revision} · {planStatus}</span>
      </div>}
    </div>
  )
}

// Exported for the profile-build performance harness (synthetic long-conversation
// fixture). Renders the real streaming message path without any Tauri dependency.

export function RuntimeTimeline({ messages, attachments, artifacts, expertBindings = [], agents = [], pendingMessage, events, runs, activeRunId, activeRunModel, runtimeRunning, state, streamingText, runtimeError, runtimeErrorDetails, assistantName = 'Fox 默认助手', planRevision, hasEarlierMessages, loadingEarlierMessages, onLoadEarlierMessages, onRetry, onRerun, onFork, onOpenSource, onOpenArtifact, onViewExpert, onOpenFileInSidebar, onRevealFileInExplorer }: { messages: ConversationMessage[]; attachments: AttachmentRecord[]; artifacts: ArtifactRecord[]; expertBindings?: ExpertBindingView[]; agents?: AgentRecord[]; pendingMessage?: ConversationMessage | null; events: RunEventRecord[]; runs?: RunRecord[]; activeRunId?: string; activeRunModel?: string; runtimeRunning: boolean; state: ChatState; streamingText: string; runtimeError?: string | null; runtimeErrorDetails?: DesktopErrorDetails | null; assistantName?: string; planRevision?: GoalProgressData['planRevisions'][number]; hasEarlierMessages?: boolean; loadingEarlierMessages?: boolean; onLoadEarlierMessages?: () => void; onRetry: () => void; onRerun: (messageId: string, prompt: string) => Promise<boolean>; onFork?: (messageId: string) => void; onOpenSource?: (source: RuntimeSource) => void; onOpenArtifact?: (artifact: ArtifactRecord) => void; onViewExpert?: (expertId: string) => void; onOpenFileInSidebar?: (path: string) => void; onRevealFileInExplorer?: (path: string) => void }) {
  const runtimeContext = useContext(TimelineRuntimeContext)
  const [groupHelpers, setGroupHelpers] = useState<RuntimeGroupHelpers | null>(null)
  const hasRuntimeEvents = events.length > 0 || messages.some(message => message.role === 'assistant' && Boolean(message.kernelPreview))
  useEffect(() => {
    if (!hasRuntimeEvents) return
    let mounted = true
    void import('../runtime-process-groups').then(helpers => { if (mounted) setGroupHelpers(helpers) })
    return () => { mounted = false }
  }, [hasRuntimeEvents])
  const [editingMessageId, setEditingMessageId] = useState<string | null>(null)
  const [editingText, setEditingText] = useState('')
  const [rerunning, setRerunning] = useState(false)
  const eventGroupsRef = useRef(new Map<string, RunEventRecord[]>)
  const artifactGroupsRef = useRef(new Map<string, ArtifactRecord[]>)
  const syntheticReasoningRef = useRef(new Map<string, RunEventRecord[]>)
  const parsedContentCacheRef = useRef(new Map<string, ReturnType<typeof splitAssistantContent>>())
  const eventsByRunId = useMemo(() => {
    const next = groupRuntimeRecords(events, eventGroupsRef.current)
    eventGroupsRef.current = next
    return next
  }, [events])
  const runTerminalStates = useMemo(
    () => deriveRunTerminalStates({ persistedRuns: runs, events }),
    [events, runs],
  )
  const failureByRunId = runTerminalStates.failures
  const cancelledRunIds = runTerminalStates.cancelled
  const artifactsByRunId = useMemo(() => {
    const next = groupRuntimeRecords(artifacts, artifactGroupsRef.current)
    artifactGroupsRef.current = next
    return next
  }, [artifacts])
  const resolvedAssistantName = runtimeContext.assistantName ?? assistantName
  const resolvedRunModel = runtimeContext.runModel ?? activeRunModel
  const storedMessages = useMemo(() => messages.filter((message) => message.role === 'user' || message.role === 'assistant'), [messages])
  const attachmentsByMessageId = useMemo(() => {
    const grouped = new Map<string, AttachmentRecord[]>()
    for (const attachment of attachments) {
      if (!attachment.messageId) continue
      const bucket = grouped.get(attachment.messageId)
      if (bucket) bucket.push(attachment)
      else grouped.set(attachment.messageId, [attachment])
    }
    return grouped
  }, [attachments])
  // Attachment tools only receive ids; the rows resolve the file names from here.
  const attachmentNames = useMemo(() => new Map(attachments.map((attachment) => [attachment.id, attachment.displayName])), [attachments])
  const agentsById = useMemo(() => new Map(agents.map((agent) => [agent.id, agent])), [agents])
  const visibleMessages = useMemo(() => {
    const pendingAlreadyStored = Boolean(pendingMessage && storedMessages.some((message) =>
      message.role === 'user' && message.content === pendingMessage.content && message.createdAt >= pendingMessage.createdAt - 3000
    ))
    const visible = pendingMessage && !pendingAlreadyStored ? [...storedMessages, pendingMessage] : [...storedMessages]
    const currentRunAssistant = activeRunId
      ? visible.find((message) => message.role === 'assistant' && message.runId === activeRunId)
      : undefined
    const currentRunHasProcess = Boolean(activeRunId && eventsByRunId.has(activeRunId))
    if (activeRunId && !currentRunAssistant && (runtimeRunning || Boolean(streamingText) || currentRunHasProcess)) {
      const latestOrdinal = visible.reduce((highest, message) => Math.max(highest, message.ordinal), 0)
      const now = Date.now()
      visible.push({
        id: `assistant-${activeRunId}`,
        conversationId: visible.at(-1)?.conversationId ?? 'pending',
        runId: activeRunId,
        role: 'assistant',
        kind: 'text',
        content: streamingText,
        status: runtimeRunning ? 'streaming' : 'completed',
        ordinal: latestOrdinal + 1,
        createdAt: now,
        updatedAt: now,
      })
    }
    return visible
  }, [activeRunId, eventsByRunId, pendingMessage, runtimeRunning, storedMessages, streamingText])
  // A run's terminal notice renders exactly once, on its last assistant message.
  const lastAssistantMessageIdByRun = useMemo(() => {
    const last = new Map<string, string>()
    for (const message of visibleMessages) {
      if (message.role !== 'assistant' || !message.runId) continue
      last.set(message.runId, message.id)
    }
    return last
  }, [visibleMessages])
  const assistantMessageCountByRun = useMemo(() => {
    const counts = new Map<string, number>()
    for (const message of visibleMessages) {
      if (message.role === 'assistant' && message.runId) counts.set(message.runId, (counts.get(message.runId) ?? 0) + 1)
    }
    return counts
  }, [visibleMessages])
  const streamTargetId = useMemo(() => latestRunAssistantId(visibleMessages, activeRunId), [visibleMessages, activeRunId])
  const groupedMessages = useMemo(() => groupAssistantContinuations(visibleMessages, streamTargetId, streamingText), [visibleMessages, streamTargetId, streamingText])
  const latestAssistantId = groupedMessages.findLast(message => message.role === 'assistant')?.id
  const interleavedRuns = useMemo(() => {
    const seen = new Set<string>()
    const interleaved = new Set<string>()
    for (const item of groupedMessages) {
      if (item.role === 'assistant' && item.runId) {
        if (seen.has(item.runId)) interleaved.add(item.runId)
        seen.add(item.runId)
      } else if (item.role === 'user' && item.runId && seen.has(item.runId)) {
        interleaved.add(item.runId)
      }
    }
    return interleaved
  }, [groupedMessages])
  const timelineEntries = useMemo(() => mergeExpertBindingsIntoTimeline(groupedMessages, expertBindings), [expertBindings, groupedMessages])
  const latestUserMessageId = useMemo(() => {
    for (let index = visibleMessages.length - 1; index >= 0; index -= 1) {
      if (visibleMessages[index].role === 'user') return visibleMessages[index].id
    }
    return undefined
  }, [visibleMessages])
  useEffect(() => {
    if (editingMessageId && editingMessageId !== latestUserMessageId) {
      setEditingMessageId(null)
      setEditingText('')
    }
  }, [editingMessageId, latestUserMessageId])
  const beginEditing = (message: ConversationMessage) => {
    if (runtimeRunning) return
    setEditingMessageId(message.id)
    setEditingText(message.content)
  }
  const cancelEditing = () => {
    if (rerunning) return
    setEditingMessageId(null)
    setEditingText('')
  }
  const rerunEditedMessage = async () => {
    const prompt = editingText.trim()
    if (!prompt) {
      toast.error('消息内容不能为空')
      return
    }
    if (runtimeRunning || rerunning || !editingMessageId) return
    setRerunning(true)
    try {
      const started = await onRerun(editingMessageId, prompt)
      if (started) {
        setEditingMessageId(null)
        setEditingText('')
      }
    } finally {
      setRerunning(false)
    }
  }
  // The Run row and its user message are written in one transaction, so the
  // prompt's timestamp is the enqueue time: the difference to `run.started` is the
  // real queue wait, not a guess.
  const queuedAtByMessageId = useMemo(() => {
    const queuedAt = new Map<string, number>()
    let promptedAt: number | null = null
    for (const message of visibleMessages) {
      if (message.role === 'user') promptedAt = Number.isFinite(message.createdAt) ? message.createdAt : null
      else if (message.role === 'assistant' && promptedAt !== null) queuedAt.set(message.id, promptedAt)
    }
    return queuedAt
  }, [visibleMessages])
  return (
    <Conversation className="fox-conversation">
      <ConversationAutoScroll scrollKey={latestUserMessageId} />
      <ConversationViewportState />
      <ConversationTurnRail messages={groupedMessages} artifacts={artifacts} />
      <ConversationContent className="fox-conversation-content" scrollClassName="fox-conversation-scroll">
        {hasEarlierMessages && <div className="fox-history-loader"><Button variant="ghost" size="sm" disabled={loadingEarlierMessages} onClick={onLoadEarlierMessages}>{loadingEarlierMessages && <LoaderCircle className="animate-spin" />}加载更早消息</Button></div>}
        <div className="fox-thread-date"><span>今天</span></div>
        {timelineEntries.map((entry) => {
          if (entry.kind === 'expert') {
            const expert = agentsById.get(entry.binding.expertId)
            return <ExpertActivationCard key={`expert-binding-${entry.binding.id}`} binding={entry.binding} expert={expert} onView={(expertId) => onViewExpert?.(expertId)} />
          }
          const message = entry.message
          if (message.role === 'user') {
            const messageAttachments = attachmentsByMessageId.get(message.id) ?? EMPTY_CONVERSATION_ATTACHMENTS
            const latest = message.id === latestUserMessageId
            const editing = latest && editingMessageId === message.id
            return <div id={`fox-turn-${message.id}`} key={message.id} className="fox-turn-anchor" data-turn-message-id={message.id}>
              <Message from="user" className={`fox-message fox-user-message ${editing ? 'is-editing' : ''}`}>
                {editing ? <MessageContent className="fox-user-edit">
                  <EditMessage value={editingText} onValueChange={setEditingText} onSave={() => void rerunEditedMessage()} onCancel={cancelEditing} saving={rerunning} discardedReplies={visibleMessages.slice(visibleMessages.findIndex(item => item.id === message.id) + 1).filter(item => item.role === 'assistant').length} />
                </MessageContent> : <UserMessageBubble>{message.content}</UserMessageBubble>}
                <SentMessageAttachments attachments={messageAttachments} />
                {!editing && <div className="fox-user-meta">
                  <span>Fox · {message.status === 'completed' ? '已发送' : message.status === 'sending' ? '发送中' : message.status}</span>
                  <span className={`fox-user-actions ${latest ? 'is-latest' : ''}`}>
                    <MessageAction tooltip="复制" onClick={() => void copyTextWithFeedback(message.content, '已复制消息')}><Copy size={13} /></MessageAction>
                    <MessageAction tooltip={runtimeRunning ? '运行完成后可创建分支' : '从此消息创建分支'} disabled={runtimeRunning} onClick={() => onFork ? onFork(message.id) : requestConversationFork(message.id)}><GitFork size={13} /></MessageAction>
                    {latest && <MessageAction tooltip={runtimeRunning ? '运行完成后可编辑' : '编辑并重新发送'} disabled={runtimeRunning} onClick={() => beginEditing(message)}><FileEdit size={13} /></MessageAction>}
                  </span>
                </div>}
              </Message>
            </div>
          }
          const isCurrentRun = Boolean(activeRunId && message.runId === activeRunId)
          const rawContent = message.content
          const parsedCacheKey = `${message.id}:${rawContent}`
          let parsed = parsedContentCacheRef.current.get(parsedCacheKey)
          if (!parsed) {
            parsed = splitAssistantContent(rawContent)
            if (parsedContentCacheRef.current.size >= 256) {
              const oldestKey = parsedContentCacheRef.current.keys().next().value
              if (typeof oldestKey === 'string') parsedContentCacheRef.current.delete(oldestKey)
            }
            parsedContentCacheRef.current.set(parsedCacheKey, parsed)
          }
          const storedProcessEvents = message.runId ? eventsByRunId.get(message.runId) ?? EMPTY_RUNTIME_EVENTS : EMPTY_RUNTIME_EVENTS
          const hasStoredReasoning = storedProcessEvents.some((item) => item.eventType === 'reasoning.delta')
          let processEvents = storedProcessEvents
          if (parsed.reasoning && !hasStoredReasoning) {
            const cacheKey = `${message.id}:${parsed.reasoning}`
            const cached = syntheticReasoningRef.current.get(cacheKey)
            if (cached) {
              processEvents = cached
            } else {
              processEvents = [...storedProcessEvents, {
                runId: message.runId ?? `history-${message.id}`,
                seq: storedProcessEvents.reduce((highest, item) => Math.max(highest, item.seq), 0) + 1,
                eventType: 'reasoning.delta',
                event: { type: 'reasoning.delta', delta: parsed.reasoning, source: 'yuxi-history' },
                createdAt: message.updatedAt,
              }]
              syntheticReasoningRef.current.set(cacheKey, processEvents)
            }
          }
          const running = message.id === streamTargetId && runtimeRunning
          const messageArtifacts = message.runId ? artifactsByRunId.get(message.runId) ?? EMPTY_RUNTIME_ARTIFACTS : EMPTY_RUNTIME_ARTIFACTS
          const eventModel = processEvents.find((item) => item.eventType === 'run.started' && typeof item.event.model === 'string')?.event.model as string | undefined
          const displayMessage = rawContent === message.content ? message : { ...message, content: rawContent }
          const isRunTail = Boolean(message.runId && lastAssistantMessageIdByRun.get(message.runId) === message.id)
          const failureReason = isRunTail && message.runId ? failureByRunId.get(message.runId) : undefined
          const cancelled = Boolean(isRunTail && message.runId && cancelledRunIds.has(message.runId))
          return <div id={`fox-turn-${message.id}`} key={message.timelineKey} className="fox-turn-anchor" data-turn-message-id={message.id}><MemoizedRuntimeAssistantMessage message={displayMessage} processEvents={processEvents} groupHelpers={groupHelpers} kernelMessages={message.kernelMessages} singleRunMessage={Boolean(message.runId && assistantMessageCountByRun.get(message.runId) === 1)} running={running} runInProgress={isCurrentRun && runtimeRunning} hasInterleavedInput={Boolean(message.runId && interleavedRuns.has(message.runId))} artifacts={messageArtifacts} assistantName={resolvedAssistantName} modelName={eventModel ?? (isCurrentRun ? resolvedRunModel : undefined)} knowledgeBindings={runtimeContext.knowledgeBindings} attachmentNames={attachmentNames} onOpenSource={onOpenSource ?? runtimeContext.onOpenSource} onOpenArtifact={onOpenArtifact} onOpenFileInSidebar={onOpenFileInSidebar} onRevealFileInExplorer={onRevealFileInExplorer} onFork={onFork} failureReason={failureReason} cancelled={cancelled} queuedAt={queuedAtByMessageId.get(message.id) ?? null} onRetry={message.id === latestAssistantId ? onRetry : undefined} /></div>
        })}
        {state === 'error' && <div className="fox-turn-anchor"><Message from="assistant" className="fox-message fox-assistant-message"><MessageContent className="fox-assistant-content"><ErrorPrompt onRetry={onRetry} error={runtimeError} errorDetails={runtimeErrorDetails} /></MessageContent></Message></div>}
        {runtimeContext.recoveryPanel}
      </ConversationContent>
      <ConversationBottomDock plan={planRevision} running={runtimeRunning} />
    </Conversation>
  )
}
