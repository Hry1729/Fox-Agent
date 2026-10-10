import { type RunFailure } from '../../conversations/model/run-failure'
import { FoxAssistantAvatar } from '../components/FoxAssistantAvatar'
import { useProcessDisplayMode } from '../process-display-mode'
import { runPhaseTiming, runPhaseTitle } from '../run-phase'
import { activeModelWaiting, compactionFailurePresentation, modelWaitingTitle } from '../run-status-facts'
import { RuntimeArtifacts } from '../runtime-artifacts'
import { runtimeProcessActivity } from '../runtime-process-activity'
import { toolAttemptState } from '../tool-attempt-status'
import { TurnProcessHeader } from '../turn-process-header'
import { Message, MessageAction, MessageActions, MessageContent } from '@/components/ai-elements/message'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Textarea } from '@/components/ui/textarea'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { type ArtifactRecord, type ConversationMessage, type KnowledgeBindingRecord, type RunEventRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { Copy, GitFork, LoaderCircle, ThumbsDown, ThumbsUp } from 'lucide-react'
import { memo, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { MarkdownResponse, TimelineRuntimeContext, copyTextWithFeedback, requestConversationFork } from '../fox-shared'
import { type RuntimeSource, runtimeProcess } from './fox-runtime-process-model'
import { RuntimeProcess, useLiveNow } from './fox-runtime-process-view'
import { RuntimeSources } from './fox-runtime-sources'
import { splitAssistantContent } from './fox-runtime-timeline'

export type RuntimeGroupHelpers = typeof import('../runtime-process-groups')

function isFoldableProcessTarget(root: HTMLElement | null, node: Node | null) {
  const element = node?.nodeType === 1 ? node as Element : node?.parentElement
  return Boolean(root && node && root.contains(node)
    && element?.closest('.fox-process-stage, .fox-answer-segment:not(.is-final)'))
}

function RuntimeAssistantMessage({ message, processEvents, groupHelpers, singleRunMessage, kernelMessages, running, runInProgress = false, hasInterleavedInput = false, artifacts, assistantName, modelName, knowledgeBindings, attachmentNames, onOpenSource, onOpenArtifact, onOpenFileInSidebar, onRevealFileInExplorer, onFork, failureReason, cancelled, queuedAt }: { message: ConversationMessage; processEvents: RunEventRecord[]; groupHelpers: RuntimeGroupHelpers | null; singleRunMessage: boolean; kernelMessages?: ConversationMessage[]; running: boolean; runInProgress?: boolean; hasInterleavedInput?: boolean; artifacts: ArtifactRecord[]; assistantName: string; modelName?: string; knowledgeBindings?: KnowledgeBindingRecord[]; attachmentNames?: ReadonlyMap<string, string>; onOpenSource?: (source: RuntimeSource) => void; onOpenArtifact?: (artifact: ArtifactRecord) => void; onOpenFileInSidebar?: (path: string) => void; onRevealFileInExplorer?: (path: string) => void; onFork?: (messageId: string) => void; failureReason?: RunFailure; cancelled?: boolean; /** When the prompt that started this run was written, so queue wait is real. */ queuedAt?: number | null }) {
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
  const processContentRef = useRef<HTMLDivElement>(null)
  const [processFocusInside, setProcessFocusInside] = useState(false)
  const [processSelectionInside, setProcessSelectionInside] = useState(false)
  const processGroupCount = orderedGroups?.filter(group => group.kind === 'process').length ?? 0
  const observeProcessSelection = processGroupCount > 0 && (running || runInProgress)
    && turnProcessChoice === null && !hasInterleavedInput && !failureReason && !cancelled
    && message.status !== 'failed' && message.status !== 'cancelled' && processDisplayMode !== 'verbose'
  useEffect(() => {
    if (!observeProcessSelection) return
    const updateSelection = () => {
      const root = processContentRef.current
      const selection = document.getSelection()
      setProcessSelectionInside(Boolean(root && selection && !selection.isCollapsed
        && (isFoldableProcessTarget(root, selection.anchorNode) || isFoldableProcessTarget(root, selection.focusNode))))
    }
    updateSelection()
    document.addEventListener('selectionchange', updateSelection)
    return () => document.removeEventListener('selectionchange', updateSelection)
  }, [observeProcessSelection])
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
  const canToggleTurn = processGroupCount > 0 && processDisplayMode !== 'verbose'
  const defaultCollapsed = completed && !runInProgress && message.status === 'completed'
    && !failureReason && !cancelled && !hasInterleavedInput
  const turnProcessCollapsed = canToggleTurn && (turnProcessChoice === 'closed'
    || turnProcessChoice === null && defaultCollapsed && !processFocusInside && !processSelectionInside)
  useLayoutEffect(() => {
    if (canToggleTurn && defaultCollapsed && turnProcessChoice === null
      && (processFocusInside || processSelectionInside)) setTurnProcessChoice('open')
  }, [canToggleTurn, defaultCollapsed, turnProcessChoice, processFocusInside, processSelectionInside])
  // The current trailing reply stays readable while the process is folded,
  // including when it is still streaming. Earlier replies remain in the fold.
  const visibleResponseIndex = orderedGroups?.reduce((last, group, index) =>
    group.kind === 'response' && index > lastProcessIndex ? index : last, -1) ?? -1
  const turnProcessStatus = failureReason || message.status === 'failed' ? '过程未完成'
    : cancelled || message.status === 'cancelled' ? '过程已取消'
      : message.status === 'interrupted' ? '过程已中断'
        : modelWaiting ? modelWaitingTitle(modelWaiting) : runInProgress || running ? '正在处理' : '工作过程'
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
          <>
            <TurnProcessHeader avatar={<FoxAssistantAvatar />} events={processEvents} active={running || runInProgress}
              canToggle={canToggleTurn} collapsed={turnProcessCollapsed} status={turnProcessStatus}
              toolStatus={pendingToolAttempt ? '工具调用待调整' : undefined} onToggle={() => {
                if (!turnProcessCollapsed) setCollapseVersion(value => value + 1)
                setProcessFocusInside(false)
                setProcessSelectionInside(false)
                setTurnProcessChoice(turnProcessCollapsed ? 'open' : 'closed')
              }} />
            <div ref={processContentRef} className="fox-process-turn-content"
              onFocusCapture={(event) => setProcessFocusInside(isFoldableProcessTarget(event.currentTarget, event.target))}
              onBlurCapture={(event) => setProcessFocusInside(isFoldableProcessTarget(event.currentTarget, event.relatedTarget))}
              onMouseUpCapture={() => {
                const root = processContentRef.current
                const selection = document.getSelection()
                setProcessSelectionInside(Boolean(root && selection && !selection.isCollapsed
                  && (isFoldableProcessTarget(root, selection.anchorNode) || isFoldableProcessTarget(root, selection.focusNode))))
              }}>
            {renderGroups?.map(({ group, renderKey }, index) => group.kind === 'process'
              ? <div key={`${renderKey}:${collapseVersion}`} hidden={turnProcessCollapsed} className="fox-process-stage">
                  <RuntimeProcess
                    events={group.events} grouped groupHelpers={groupHelpers} displayMode={processDisplayMode} runInProgress={runInProgress} running={runActive && index === orderedGroups.length - 1}
                    outcome={index === lastProcessIndex ? failureReason ? 'failed' : cancelled ? 'cancelled' : undefined : undefined}
                    attachmentNames={attachmentNames}
                    phaseTitle={phaseTitle}
                    onOpenFileInSidebar={onOpenFileInSidebar} onRevealFileInExplorer={onRevealFileInExplorer}
                  />
                </div>
               : group.kind === 'notice' ? <div key={group.key} className="fox-process-notice" role="status">
                   {group.label}
                 </div>
               : <div key={group.key} hidden={turnProcessCollapsed && index !== visibleResponseIndex} className={`fox-answer-body fox-answer-segment${index === visibleResponseIndex ? ' is-final' : ''}`}>
                  <MarkdownResponse className="fox-answer-response">{group.text}</MarkdownResponse>
                </div>)}
            </div>
          </>
          {compactionFailures.map(({ item, presentation }) => presentation && <div key={`compaction-failure-${item.seq}`} className="fox-process-notice" role="status">
            {presentation.label}
          </div>)}
          {!running && <RuntimeArtifacts artifacts={artifacts} onOpenArtifact={onOpenArtifact} />}
          <RuntimeSources sources={process.sources} knowledgeBindings={knowledgeBindings} onOpenSource={onOpenSource} />
        </MessageContent>
        <div className="fox-message-footer">
          <MessageActions className="fox-message-actions">
            <MessageAction tooltip="复制" onClick={() => parsed.answer ? void copyTextWithFeedback(parsed.answer, '已复制回复') : toast('暂无可复制内容')}><Copy size={14} /></MessageAction>
            <MessageAction tooltip={running ? '运行完成后可创建分支' : '从此消息创建分支'} disabled={running} onClick={() => onFork ? onFork(message.id) : requestConversationFork(message.id)}><GitFork size={14} /></MessageAction>
            <MessageAction tooltip="这条回答有帮助" className={feedback === 'positive' ? 'is-feedback-selected' : ''} aria-pressed={feedback === 'positive'} disabled={running || feedbackSaving} onClick={() => void submitPositiveFeedback()}><ThumbsUp size={14} /></MessageAction>
            <MessageAction tooltip="这条回答需要改进" className={feedback === 'negative' ? 'is-feedback-selected' : ''} aria-pressed={feedback === 'negative'} disabled={running || feedbackSaving} onClick={() => setFeedbackOpen(true)}><ThumbsDown size={14} /></MessageAction>
          </MessageActions>
          <div className="fox-message-meta"><span>{assistantName}</span>{modelName && modelName !== assistantName && <span>{modelName}</span>}<span>{replyTime}</span>{!completed && <span className="fox-message-live-meta">处理中</span>}{message.status === 'interrupted' && !failureReason && !cancelled && <span>已中断 · 内容已保存</span>}{cancelled && <span className="fox-message-cancelled-meta">运行已取消</span>}{failureReason && <span className="fox-message-failure-meta">{failureReason.kind === 'interrupted' ? '运行中断' : '运行失败'} · {failureReason.message}</span>}</div>
        </div>
      </Message>
      <Dialog open={feedbackOpen} onOpenChange={setFeedbackOpen}>
        <DialogContent className="fox-feedback-dialog">
          <DialogHeader><DialogTitle>这条回答哪里需要改进？</DialogTitle><DialogDescription>只保存分类、备注和运行标识，不保存回答正文、文件内容或推理过程。</DialogDescription></DialogHeader>
          <div className="fox-feedback-categories" role="radiogroup" aria-label="反馈分类">
            {([
              ['irrelevant', '答非所问'],
              ['code_error', '代码错误'],
              ['misunderstanding', '理解偏差'],
              ['other', '其他'],
            ] as const).map(([value, label]) => <button type="button" role="radio" aria-checked={feedbackCategory === value} className={feedbackCategory === value ? 'is-selected' : ''} key={value} onClick={() => setFeedbackCategory(value)}>{label}</button>)}
          </div>
          <label className="fox-feedback-comment"><span>补充说明 <small>可选</small></span><Textarea value={feedbackComment} maxLength={1000} placeholder="告诉我们具体问题，最多 1000 字" onChange={(event) => setFeedbackComment(event.target.value)} /><small>{feedbackComment.length} / 1000</small></label>
          <DialogFooter><Button variant="outline" onClick={() => setFeedbackOpen(false)}>取消</Button><Button disabled={feedbackSaving} onClick={() => void submitNegativeFeedback()}>{feedbackSaving && <LoaderCircle className="animate-spin" />}提交反馈</Button></DialogFooter>
        </DialogContent>
      </Dialog>
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
))
