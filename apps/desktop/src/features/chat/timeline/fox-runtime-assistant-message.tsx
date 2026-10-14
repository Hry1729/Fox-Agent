import { type RunFailure } from '../../conversations/model/run-failure'
import { FoxAssistantAvatar } from '../components/FoxAssistantAvatar'
import { useProcessDisplayMode } from '../process-display-mode'
import { runPhaseTiming, runPhaseTitle } from '../run-phase'
import { activeModelWaiting, compactionFailurePresentation, modelWaitingTitle } from '../run-status-facts'
import { RuntimeArtifacts } from '../runtime-artifacts'
import { runtimeProcessActivity } from '../runtime-process-activity'
import { toolAttemptState } from '../tool-attempt-status'
import { TurnProcessHeader } from '../turn-process-header'
import { StreamingText } from '../components/StreamingText'
import { RunActivity } from '@/components/assistant-ui/elements/run-activity'
import { StoppedRun } from '@/components/assistant-ui/elements/error-state'
import { MessageActions as AssistantMessageActions, copyMessageResponse } from '@/components/assistant-ui/elements/message-actions'
import { FeedbackDialog as FeedbackDialogSurface } from '@/components/assistant-ui/elements/feedback-dialog'
import { ErrorPrompt } from '../demo/fox-demo-prompts'
import { Message, MessageContent } from '@/components/ai-elements/message'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { type ArtifactRecord, type ConversationMessage, type KnowledgeBindingRecord, type RunEventRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { memo, useContext, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { TimelineRuntimeContext, requestConversationFork } from '../fox-shared'
import { type RuntimeSource, runtimeProcess } from './fox-runtime-process-model'
import { RuntimeProcess, useLiveNow } from './fox-runtime-process-view'
import { RuntimeSources } from './fox-runtime-sources'
import { splitAssistantContent } from './fox-assistant-content'

export type RuntimeGroupHelpers = typeof import('../runtime-process-groups')

function RuntimeAssistantMessage({ message, processEvents, groupHelpers, singleRunMessage, kernelMessages, running, runInProgress = false, hasInterleavedInput = false, artifacts, assistantName, modelName, knowledgeBindings, attachmentNames, onOpenSource, onOpenArtifact, onOpenFileInSidebar, onRevealFileInExplorer, onFork, failureReason, cancelled, queuedAt, hideFooter = false, onRetry }: { message: ConversationMessage; processEvents: RunEventRecord[]; groupHelpers: RuntimeGroupHelpers | null; singleRunMessage: boolean; kernelMessages?: ConversationMessage[]; running: boolean; runInProgress?: boolean; hasInterleavedInput?: boolean; artifacts: ArtifactRecord[]; assistantName: string; modelName?: string; knowledgeBindings?: KnowledgeBindingRecord[]; attachmentNames?: ReadonlyMap<string, string>; onOpenSource?: (source: RuntimeSource) => void; onOpenArtifact?: (artifact: ArtifactRecord) => void; onOpenFileInSidebar?: (path: string) => void; onRevealFileInExplorer?: (path: string) => void; onFork?: (messageId: string) => void; failureReason?: RunFailure; cancelled?: boolean; /** When the prompt that started this run was written, so queue wait is real. */ queuedAt?: number | null; hideFooter?: boolean; onRetry?: () => void }) {
  const { authoritativeRunId, authoritativeRunState } = useContext(TimelineRuntimeContext)
  const processDisplayMode = useProcessDisplayMode()
  const parsed = useMemo(() => splitAssistantContent(message.content ?? ''), [message.content])
  const process = useMemo(() => runtimeProcess(processEvents), [processEvents])
  // Stage of the whole Run, not of one projected group: the group that is still
  // live can be a later slice that no longer contains `run.started`.
  const liveNow = useLiveNow(running)
  const phaseTitle = useMemo(
    () => runPhaseTitle(runPhaseTiming(processEvents, { queuedAt, now: liveNow,
      runState: message.runId === authoritativeRunId ? authoritativeRunState : undefined })),
    [liveNow, processEvents, queuedAt, message.runId, authoritativeRunId, authoritativeRunState],
  )
  const modelWaiting = useMemo(() => running || runInProgress ? activeModelWaiting(processEvents, {
    runId: message.runId ?? undefined, runState: message.runId === authoritativeRunId ? authoritativeRunState : undefined,
  }) : null, [processEvents, running, runInProgress, message.runId, authoritativeRunId, authoritativeRunState])
  const compactionFailures = useMemo(() => processEvents.map(item => ({ item, presentation: compactionFailurePresentation(item) }))
    .filter(({ presentation }) => presentation !== null), [processEvents])
  const projectedGroups = useMemo(() => {
    if (groupHelpers && message.runId && (kernelMessages?.length || !message.content && (message.runId === authoritativeRunId
      || processEvents.some(item => typeof item.event.kernelCheckpointSeq === 'string')))) {
      return groupHelpers.projectKernelGroups(message.runId, kernelMessages ?? [], processEvents)
    }
    // Legacy keeps its verified append-only answer boundaries.
    if (!groupHelpers || !message.runId || !singleRunMessage || message.runId === authoritativeRunId
      || message.id !== `assistant-${message.runId}` || message.kernelPreview
      || /<\/?(?:(?:mm:)?(?:think|thinking|analysis))>/i.test(message.content)) return null
    const groups = groupHelpers.projectRuntimeGroups(message.runId, message.content, processEvents)
    return groups?.length ? groups : null
  }, [message.id, message.runId, message.kernelPreview, message.content, singleRunMessage, kernelMessages, authoritativeRunId, processEvents, groupHelpers])
  // Incomplete/older histories cannot prove reply boundaries. Keep their
  // process in one group and the saved answer intact, using the SAME turn UI.
  // Never fall back to the old live-reasoning card while helpers load or after
  // an error; no inferred interleaving or duplicated answer text.
  const orderedGroups = useMemo(() => projectedGroups ?? [
    ...(processEvents.length || running ? [{ kind: 'process' as const, key: `${message.id}:fallback-process`, events: processEvents }] : []),
    ...(parsed.answer ? [{ kind: 'response' as const, key: `${message.id}:fallback-answer`, text: parsed.answer }] : []),
  ], [projectedGroups, processEvents, running, message.id, parsed.answer])
  const [turnProcessChoice, setTurnProcessChoice] = useState<'open' | 'closed' | null>(null)
  const [collapseVersion, setCollapseVersion] = useState(0)
  const processGroupCount = orderedGroups?.filter(group => group.kind === 'process').length ?? 0
  const previousProcessGroups = useRef<{ sourceKey: string; renderKey: string; eventIds: Set<string> }[]>([])
  const renderGroups = useMemo(() => {
    const used = new Set<string>()
    return orderedGroups?.map(group => {
      if (group.kind !== 'process') return { group, renderKey: group.key, eventIds: new Set<string>() }
      const eventIds = new Set(group.events.map(item => `${item.eventType}:${item.seq}:${item.event.source ?? ''}`))
      const previous = previousProcessGroups.current.find(item => !used.has(item.renderKey) && item.sourceKey === group.key)
        ?? previousProcessGroups.current.find(item => !used.has(item.renderKey) && [...eventIds].some(id => item.eventIds.has(id)))
      const renderKey = previous?.renderKey ?? group.key
      used.add(renderKey)
      return { group, renderKey, eventIds }
    })
  }, [orderedGroups])
  useLayoutEffect(() => {
    previousProcessGroups.current = renderGroups?.filter(item => item.group.kind === 'process')
      .map(item => ({ sourceKey: item.group.key, renderKey: item.renderKey, eventIds: item.eventIds })) ?? []
  }, [renderGroups])
  const lastProcessIndex = orderedGroups?.reduce((last, group, index) => group.kind === 'process' ? index : last, -1) ?? -1
  const latestProcessSeq = processEvents.reduce((latest, event) => Math.max(latest, event.seq), 0)
  const runAttemptState = failureReason ? 'failed' : cancelled ? 'cancelled' : running || runInProgress ? 'running' : 'completed'
  const pendingToolAttempt = process.tools.some((tool) => tool.isError
    && toolAttemptState(tool, process.tools, latestProcessSeq, runAttemptState) === 'pending')
  const runActive = useMemo(() => runtimeProcessActivity(processEvents, running).running, [processEvents, running])
  const completed = !running && ['completed', 'interrupted', 'failed', 'cancelled'].includes(message.status)
  const canToggleTurn = processGroupCount > 0
  const turnProcessCollapsed = canToggleTurn && turnProcessChoice !== 'open'
  // The current trailing reply stays readable while the process is folded,
  // including when it is still streaming. Earlier replies remain in the fold.
  const visibleResponseIndex = orderedGroups?.reduce((last, group, index) =>
    group.kind === 'response' && index > lastProcessIndex ? index : last, -1) ?? -1
  const turnProcessStatus = failureReason || message.status === 'failed' ? '过程未完成'
    : cancelled || message.status === 'cancelled' ? '过程已取消'
      : message.status === 'interrupted' ? '过程已中断'
        : modelWaiting ? modelWaitingTitle(modelWaiting) : runInProgress || running ? '正在处理' : '处理过程'
  const replyTime = new Date(message.updatedAt || message.createdAt).toLocaleString([], { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
  const [feedback, setFeedback] = useState<'positive' | 'negative' | null>(null)
  const [feedbackOpen, setFeedbackOpen] = useState(false)
  const [feedbackCategory, setFeedbackCategory] = useState<'irrelevant' | 'code_error' | 'misunderstanding' | 'other'>('irrelevant')
  const [feedbackComment, setFeedbackComment] = useState('')
  const [feedbackSaving, setFeedbackSaving] = useState(false)
  const submitPositiveFeedback = async () => {
    setFeedbackSaving(true)
    try {
      await desktopClient.saveMessageFeedback(message.id, 'positive')
      setFeedback('positive')
      toast.success('感谢反馈')
    } catch (cause) {
      toast.error('反馈保存失败', { description: desktopErrorDetails(cause).message })
    } finally {
      setFeedbackSaving(false)
    }
  }
  const submitNegativeFeedback = async () => {
    setFeedbackSaving(true)
    try {
      await desktopClient.saveMessageFeedback(message.id, 'negative', feedbackCategory, feedbackComment.trim() || undefined)
      setFeedback('negative')
      setFeedbackOpen(false)
      toast.success('反馈已保存')
    } catch (cause) {
      toast.error('反馈保存失败', { description: desktopErrorDetails(cause).message })
    } finally {
      setFeedbackSaving(false)
    }
  }

  return (
    <div className="fox-turn-anchor">
      <Message from="assistant" className="fox-message fox-assistant-message">
        <MessageContent className="fox-assistant-content">
            <RunActivity status={runAttemptState === 'completed' ? 'complete' : runAttemptState === 'failed' ? 'error' : runAttemptState} statusLabel={turnProcessStatus} open={!turnProcessCollapsed} onOpenChange={next => setTurnProcessChoice(next ? 'open' : 'closed')} className="fox-process-turn-content"
              header={
            <TurnProcessHeader avatar={<FoxAssistantAvatar />} events={processEvents} active={running || runInProgress}
              canToggle={canToggleTurn} collapsed={turnProcessCollapsed} status={turnProcessStatus}
              toolStatus={pendingToolAttempt ? '工具调用待调整' : undefined} onToggle={() => {
                if (!turnProcessCollapsed) setCollapseVersion(value => value + 1)
                setTurnProcessChoice(turnProcessCollapsed ? 'open' : 'closed')
              }} />}
              entries={renderGroups.flatMap(({ group, renderKey }, index) => group.kind === 'notice' || index === visibleResponseIndex ? [] : [{
                id: group.kind === 'process' ? `${renderKey}:${collapseVersion}` : group.key,
                kind: group.kind === 'process' ? 'tool' as const : 'commentary' as const,
                label: group.kind === 'process' ? '' : group.text,
                content: group.kind === 'process' ? <div hidden={turnProcessCollapsed} className="fox-process-stage">
                  <RuntimeProcess
                    events={group.events} grouped groupHelpers={groupHelpers} displayMode={processDisplayMode} runInProgress={runInProgress} running={runActive && index === orderedGroups.length - 1}
                    outcome={index === lastProcessIndex ? failureReason ? 'failed' : cancelled ? 'cancelled' : undefined : undefined}
                    attachmentNames={attachmentNames}
                    phaseTitle={phaseTitle}
                    onOpenFileInSidebar={onOpenFileInSidebar} onRevealFileInExplorer={onRevealFileInExplorer}
                  />
                </div> : <div hidden={turnProcessCollapsed} className="fox-answer-body fox-answer-segment"><StreamingText className="fox-answer-response" text={group.text} streaming={false} /></div>,
              }])}
              attention={renderGroups.filter(({ group }) => group.kind === 'notice').map(({ group }) => group.kind === 'notice' && <div key={group.key} className="fox-process-notice" role="status">{group.label}</div>)}
            >
            {renderGroups.map(({ group }, index) => group.kind === 'response' && index === visibleResponseIndex && <div key={group.key} className="fox-answer-body fox-answer-segment is-final"><StreamingText className="fox-answer-response" text={group.text} streaming={running} /></div>)}
            </RunActivity>
          {compactionFailures.map(({ item, presentation }) => presentation && <div key={`compaction-failure-${item.seq}`} className="fox-process-notice" role="status">
            {presentation.label}
          </div>)}
          {!running && <RuntimeArtifacts artifacts={artifacts} onOpenArtifact={onOpenArtifact} />}
          {failureReason && !hideFooter && <ErrorPrompt error={failureReason.message} onRetry={onRetry} />}
          {cancelled && <StoppedRun reason="运行已取消，已生成内容保留" />}
          <RuntimeSources sources={process.sources} knowledgeBindings={knowledgeBindings} onOpenSource={onOpenSource} />
        </MessageContent>
        {!hideFooter && !running && !runInProgress && <div className="fox-message-footer">
          <AssistantMessageActions reaction={feedback} running={running || runInProgress} busy={feedbackSaving}
            onCopy={() => { if (parsed.answer) return copyMessageResponse(parsed.answer); toast('暂无可复制内容'); return false }}
            onFork={() => onFork ? onFork(message.id) : requestConversationFork(message.id)}
            onPositive={() => void submitPositiveFeedback()} onNegative={() => setFeedbackOpen(true)} onRegenerate={onRetry} />
          <div className="fox-message-meta"><span>{assistantName}</span>{modelName && modelName !== assistantName && <span>{modelName}</span>}<span>{replyTime}</span>{!completed && <span className="fox-message-live-meta">处理中</span>}{message.status === 'interrupted' && !failureReason && !cancelled && <span>已中断</span>}{cancelled && <span className="fox-message-cancelled-meta">已取消</span>}{failureReason && <span className="fox-message-failure-meta">{failureReason.kind === 'interrupted' ? '运行中断' : '运行失败'}</span>}</div>
        </div>}
      </Message>
      <FeedbackDialogSurface open={feedbackOpen} onOpenChange={setFeedbackOpen} category={feedbackCategory} onCategoryChange={setFeedbackCategory} note={feedbackComment} onNoteChange={setFeedbackComment} saving={feedbackSaving} onSubmit={() => void submitNegativeFeedback()} />
    </div>
  )
}

export const MemoizedRuntimeAssistantMessage = memo(RuntimeAssistantMessage, (previous, next) => (
  previous.message.id === next.message.id
  && previous.message.content === next.message.content
  && previous.message.status === next.message.status
  && previous.message.runId === next.message.runId
  && previous.message.createdAt === next.message.createdAt
  && previous.message.updatedAt === next.message.updatedAt
  && previous.processEvents === next.processEvents
  && previous.groupHelpers === next.groupHelpers
  && previous.singleRunMessage === next.singleRunMessage
  && previous.kernelMessages === next.kernelMessages
  && previous.running === next.running
  && previous.runInProgress === next.runInProgress
  && previous.hasInterleavedInput === next.hasInterleavedInput
  && previous.artifacts === next.artifacts
  && previous.assistantName === next.assistantName
  && previous.modelName === next.modelName
  && previous.knowledgeBindings === next.knowledgeBindings
  && previous.attachmentNames === next.attachmentNames
  && previous.onOpenSource === next.onOpenSource
  && previous.onOpenArtifact === next.onOpenArtifact
  && previous.onOpenFileInSidebar === next.onOpenFileInSidebar
  && previous.onRevealFileInExplorer === next.onRevealFileInExplorer
  && previous.onFork === next.onFork
  && previous.failureReason === next.failureReason
  && previous.cancelled === next.cancelled
  && previous.queuedAt === next.queuedAt
  && previous.hideFooter === next.hideFooter
  && previous.onRetry === next.onRetry
))
