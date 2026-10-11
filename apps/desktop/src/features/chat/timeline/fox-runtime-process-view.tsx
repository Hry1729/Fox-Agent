import { RunStatusText } from '../components/FoxAssistantAvatar'
import { executionReceiptPresentation } from '../execution-receipt'
import { type ProcessDisplayMode } from '../process-display-mode'
import { OVERFLOW_CLASS, useOverflowFade } from '../row-overflow'
import { toolFailurePresentation } from '../run-status-facts'
import { runtimeProcessActivity } from '../runtime-process-activity'
import { TOOL_ATTEMPT_REASON_LABELS, TOOL_ATTEMPT_STATE_LABELS, type ToolAttemptState, toolAttemptReason, toolAttemptState } from '../tool-attempt-status'
import { useProcessScroll } from '../use-process-scroll'
import { ToolTimeline } from '@/components/assistant-ui/elements/tool-timeline'
import { ToolCall } from '@/components/assistant-ui/elements/tool-call'
import { ToolInput as AIToolInput, ToolOutput as AIToolOutput } from '@/components/ai-elements/tool'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from '@/components/ui/context-menu'
import { type RunEventRecord } from '@/features/conversations/model/types'
import { AlertTriangle, Bot, Brain, Check, ChevronRight, CircleHelp, Database, FileEdit, FileText, FolderOpen, FolderSearch, Globe2, ListTodo, LoaderCircle, Search, Terminal, Wrench } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useRef, useState } from 'react'
import { MarkdownResponse } from '../fox-shared'
import { type RuntimeGroupHelpers } from './fox-runtime-assistant-message'
import { type RuntimeToolStep, runtimeProcess, stepSlotsText, toolActivity, toolIcon, toolRowSlots } from './fox-runtime-process-model'

function RuntimeReasoningItem({ detail, running = false, preview = true }: { detail: string; running?: boolean; preview?: boolean }) {
  const summary = preview ? reasoningSummary(detail) : ''
  // The row's own state supplies the prefix: the live trailing thought is
  // 正在思考, a settled one is 深度思考. The prefix is never derived from the text.
  const prefix = running ? (summary ? '正在思考 ·' : '正在思考') : (summary ? '深度思考 ·' : '深度思考')
  return <RuntimeProcessRow
    icon={<Brain className="fox-runtime-step-icon-glyph" />}
    fixedPrefix={prefix}
    label={summary}
    hideSummaryWhenOpen
    reasoning
    active={running}
    status={running ? 'active' : 'complete'}
  >
    <MarkdownResponse className="fox-reasoning-text">{detail}</MarkdownResponse>
  </RuntimeProcessRow>
}

/** The row paints exactly one line, so the summary is the current thought
 *  collapsed into that line: whitespace and newlines become single spaces and the
 *  text keeps growing in place while it streams. Waiting for a *completed* line
 *  left the row blank for long unbroken replies and made settled thoughts show
 *  only their shortest first line. Short content therefore keeps its natural
 *  spacing and is never padded; anything longer is ellipsized on the right by
 *  CSS. The scan is bounded because a thought can reach tens of kilobytes while
 *  only its first line is ever visible. */
const REASONING_SUMMARY_SCAN_LIMIT = 512
function reasoningSummary(text: string) {
  const window = text.length > REASONING_SUMMARY_SCAN_LIMIT ? text.slice(0, REASONING_SUMMARY_SCAN_LIMIT) : text
  // The summary is the model's own first line, in whatever language it reasoned in:
  // a Chinese status is *not* substituted for foreign reasoning (a script test
  // cannot tell that a thought is still running, and rewriting the line would hide
  // what the model actually said). The row's state prefix carries the state.
  return rawReasoningSummary(window)
}

function rawReasoningSummary(window: string) {
  return window
    .replace(/\s+/gu, ' ')
    .replaceAll('**', '')
    // A cut surrogate pair would render as a replacement glyph at the edge.
    .replace(/[\uD800-\uDBFF]$/u, '')
    .trim()
}

function RuntimeToolItem({ tool, active, attemptState, attachmentNames, onOpenFileInSidebar, onRevealFileInExplorer }: { tool: RuntimeToolStep; active: boolean; attemptState?: ToolAttemptState; attachmentNames?: ReadonlyMap<string, string>; onOpenFileInSidebar?: (path: string) => void; onRevealFileInExplorer?: (path: string) => void }) {
  const [open, setOpen] = useState(false)
  const execution = executionReceiptPresentation(tool.output)
  const failure = tool.isError ? toolFailurePresentation(tool.name, tool.output) : null
  const ToolIcon = tool.isError || execution?.requiresAttention
    ? AlertTriangle
    : tool.awaitingUser
      ? CircleHelp
      : toolIcon(tool)
  // The row is built from independent slots (action / path / source / summary).
  // A path is interactive because the row *has* a path and a handler — the
  // decision never reads the rendered string, so gaining a source cannot take the
  // click away and the visible order can never change underneath the reader.
  const slots = toolRowSlots(tool, attachmentNames)
  const { ref: tailRef, overflowing: tailOverflowing } = useOverflowFade<HTMLSpanElement>(`${slots.source} ${slots.summary}`, Boolean(slots.source || slots.summary))
  const pathIsInteractive = Boolean(slots.path && (onOpenFileInSidebar || onRevealFileInExplorer))
  const openPathInSidebar = () => onOpenFileInSidebar?.(slots.path)
  const pathContent = !slots.path ? undefined : pathIsInteractive
    ? (
      /* The file name stays a span: it lives inside the row's disclosure
          button, and clicking it must never toggle that row. */
      <ContextMenu>
        <ContextMenuTrigger asChild>
          <span
            className="fox-runtime-step-path"
            role="button"
            tabIndex={0}
            title={slots.path}
            onClick={(event) => { event.stopPropagation(); openPathInSidebar() }}
            onKeyDown={(event) => {
              if (event.key !== 'Enter' && event.key !== ' ') return
              event.preventDefault()
              event.stopPropagation()
              openPathInSidebar()
            }}
          >{slots.path}</span>
        </ContextMenuTrigger>
        <ContextMenuContent className="fox-runtime-path-menu">
          <ContextMenuItem onSelect={() => onRevealFileInExplorer?.(slots.path)}><FolderOpen />在文件管理器打开</ContextMenuItem>
          <ContextMenuItem onSelect={() => onRevealFileInExplorer?.(slots.path)}><FolderSearch />打开文件位置</ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    )
    // No handler: the path is still shown, as plain row text rather than as a
    // control that could not do anything.
    : <span className="fox-runtime-step-path-plain" title={slots.path}>{slots.path}</span>
  const attemptContext = tool.isError && attemptState
    ? `${TOOL_ATTEMPT_REASON_LABELS[toolAttemptReason(tool.output, tool.name)]}问题 · ${TOOL_ATTEMPT_STATE_LABELS[attemptState]}` : undefined
  const executionError = execution?.requiresAttention
    ? execution.statusLabel
    : tool.isError && execution
      ? `${execution.statusLabel} · ${attemptContext ?? '工具调用未成功'}`
      : tool.isError
        ? attemptContext ?? failure?.summary ?? '工具调用未成功'
        : undefined
  const action = <span className="fox-run-step-action">{slots.action}</span>
  return <ToolCall className="fox-element-tool-call fox-runtime-step-row max-w-none" open={open} onOpenChange={setOpen} running={active} completed={tool.completed && !tool.awaitingUser} label={action} activeLabel={action} ariaLabel={stepSlotsText(slots)}
    icon={<ToolIcon className="fox-runtime-step-icon-glyph" />}
    query={<><span className="fox-run-step-path-slot">{pathContent}</span>{(slots.source || slots.summary) && <><span aria-hidden="true"> · </span><span ref={tailRef} className={`fox-run-step-tail${tailOverflowing ? ` ${OVERFLOW_CLASS}` : ''}`}>{slots.source && <span className="fox-run-step-source">{slots.source}</span>}{slots.source && slots.summary && <span aria-hidden="true"> · </span>}{slots.summary && <span className="fox-run-step-summary">{slots.summary}</span>}</span></>}</>}
    request={!tool.isError ? <AIToolInput className="fox-runtime-tool-detail" input={tool.input as never} /> : undefined}
    result={tool.completed ? <AIToolOutput className="fox-runtime-tool-detail" output={(failure
      ? { 说明: failure.summary, ...(failure.hint ? { 处理建议: failure.hint } : {}), ...(failure.target ? { 对象: failure.target } : {}) }
      : tool.output) as never} errorText={undefined} /> : undefined} error={executionError} />
}

/** A step row is exactly one line tall, however long the thought grows. Plain text
 *  is clipped at the right edge by CSS; a row built from slots carries its own
 *  structure, so only its source/summary region is clipped and faded while the
 *  action wording stays fully painted. Both regions fade only when a real
 *  measurement says the line is cut (row-overflow.ts) — a short summary is never
 *  dimmed. The row never scrolls sideways, so a streaming thought cannot drift or
 *  jitter. */
function StepStatusText({ text, active, fixedPrefix, slots }: { text: string; active: boolean; fixedPrefix?: string; slots?: ReactNode }) {
  const slotted = slots !== undefined
  // An interactive tool path must never be rendered a second time. Only a row
  // without slots can afford one inert decorative layer for a shared title sweep.
  const decorate = active && !slotted
  // A plain label clips its own single line, so it is the element that has to earn
  // the trailing fade. A slotted label never carries one: its source/summary tail
  // does, and it is measured there (RuntimeProcessRow), which is what keeps the
  // action name and the clickable path fully painted.
  const { ref: labelRef, overflowing } = useOverflowFade<HTMLSpanElement>(text, !slotted)
  const labelClass = `fox-run-status-label${slotted ? ' has-slots' : ''}${!slotted && overflowing ? ` ${OVERFLOW_CLASS}` : ''}`
  return <span className={`fox-run-status-text fox-run-status-step${fixedPrefix ? ' has-fixed-prefix' : ''}${decorate ? ' fox-row-shimmer' : ''}`} data-active={active}>
    {fixedPrefix && <span className="fox-run-status-prefix">{fixedPrefix}</span>}
    <span className="fox-run-status-viewport">
      <span ref={labelRef} className={labelClass}>{slots ?? text}</span>
    </span>
    {decorate && <span className="fox-row-shimmer-decoration" aria-hidden="true" inert>
      <span className="fox-row-shimmer-sweep">
        <span className={`fox-row-shimmer-highlight fox-run-status-step-highlight${fixedPrefix ? ' has-fixed-prefix' : ''}`}>
          {fixedPrefix && <span className="fox-run-status-prefix" data-shimmer-text={fixedPrefix} />}
          <span className="fox-run-status-viewport">
            {/* The decorative copy paints the same glyphs, so it takes the same
                measured state; a second measurement would only drift from it. */}
            <span className={labelClass} data-shimmer-text={text} />
          </span>
        </span>
      </span>
    </span>}
  </span>
}

/** Every thought segment and every tool call is one independently collapsible row.
 *
 *  `label` is the row's whole line as text (accessible name, shimmer, plain-text
 *  fallback). `actionText` / `pathContent` / `sourceContent` / `summaryContent` are
 *  the same line as independent slots, and they — never `label` — carry the
 *  structure the row's interactions are decided on. */
function RuntimeProcessRow({ icon, label, fixedPrefix, actionText, pathContent, sourceContent, summaryContent, hideSummaryWhenOpen = false, reasoning = false, active = false, status = 'complete', children }: { icon: ReactNode; label: string; fixedPrefix?: string; actionText?: ReactNode; pathContent?: ReactNode; sourceContent?: ReactNode; summaryContent?: ReactNode; hideSummaryWhenOpen?: boolean; reasoning?: boolean; active?: boolean; status?: 'complete' | 'active' | 'pending'; children: ReactNode }) {
  const [open, setOpen] = useState(false)
  const [visited, setVisited] = useState(false)
  const summaryHidden = hideSummaryWhenOpen && open
  const hasTail = sourceContent !== undefined || summaryContent !== undefined
  // The tail is the one region allowed to fade, so it is also the one region that is
  // measured: the fade appears only when the browser says this element really cuts
  // its own line, never because the text merely looks long.
  const { ref: tailRef, overflowing: tailOverflowing } = useOverflowFade<HTMLSpanElement>(label, hasTail)
  // Reading order: action · path · source · summary. The action and the path are
  // their own elements; the trailing source/summary region is the only part that
  // is clipped and faded, so neither the verb nor the glyph can ever be masked.
  const slots = actionText === undefined ? undefined : <>
    <span className="fox-run-step-action">{actionText}</span>
    {pathContent !== undefined && <>
      <span className="fox-run-step-separator" aria-hidden="true"> · </span>
      <span className="fox-run-step-path-slot">{pathContent}</span>
    </>}
    {hasTail && <>
      <span className="fox-run-step-separator" aria-hidden="true"> · </span>
      <span ref={tailRef} className={`fox-run-step-tail${tailOverflowing ? ` ${OVERFLOW_CLASS}` : ''}`}>
        {sourceContent !== undefined && <span className="fox-run-step-source">{sourceContent}</span>}
        {sourceContent !== undefined && summaryContent !== undefined && <span className="fox-run-step-separator" aria-hidden="true"> · </span>}
        {summaryContent !== undefined && <span className="fox-run-step-summary">{summaryContent}</span>}
      </span>
    </>}
  </>
  return (
    <Collapsible
      className={`fox-runtime-step-detail fox-runtime-step-row is-${status}${reasoning ? ' is-reasoning' : ''}`}
      open={open}
      onOpenChange={(next) => { setOpen(next); if (next) setVisited(true) }}
    >
      <div className="fox-runtime-step-icon">
        {/* Hovering the row turns the step glyph into that row's own disclosure toggle. */}
        <CollapsibleTrigger className="fox-runtime-step-icon-toggle" aria-label={open ? '收起这一步的详情' : '展开这一步的详情'}>
          {icon}
          <ChevronRight className="fox-runtime-step-icon-chevron" />
        </CollapsibleTrigger>
        <div className="fox-runtime-step-connector" />
      </div>
      <div className="fox-runtime-step-body">
        <CollapsibleTrigger className="fox-runtime-step-trigger">
          {/* Opening a row hides its one-line summary but must never rewrite the
              state prefix: an open live row is still 正在思考, not 深度思考. */}
          <StepStatusText text={summaryHidden ? '' : label} active={active} fixedPrefix={fixedPrefix} slots={summaryHidden ? undefined : slots} />
        </CollapsibleTrigger>
        {visited && <CollapsibleContent forceMount className="fox-runtime-step-content" aria-hidden={!open} inert={!open}>
          <div className="fox-runtime-detail-clip"><div className="fox-runtime-detail-panel">{children}</div></div>
        </CollapsibleContent>}
      </div>
    </Collapsible>
  )
}

function useStableProcessTitle(desired: string, running: boolean) {
  const [displayed, setDisplayed] = useState(desired)
  const shown = useRef({ value: desired, at: Date.now() })
  useEffect(() => {
    if (!running) {
      shown.current = { value: desired, at: Date.now() }
      setDisplayed(desired)
      return
    }
    if (shown.current.value === desired) return
    const remaining = Math.max(0, 150 - (Date.now() - shown.current.at))
    const timer = window.setTimeout(() => {
      shown.current = { value: desired, at: Date.now() }
      setDisplayed(desired)
    }, remaining)
    return () => window.clearTimeout(timer)
  }, [desired, running])
  return running ? displayed : desired
}

/** One tick a second, and only while a Run is live, so a phase label can age. */
export function useLiveNow(active: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!active) return
    setNow(Date.now())
    const timer = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(timer)
  }, [active])
  return now
}

export function RuntimeProcess({ events, process: preparedProcess, groupHelpers, displayMode = 'standard', runInProgress = false, running: runtimeRunning, answerStarted = false, leading, grouped = false, outcome, attachmentNames, onOpenFileInSidebar, onRevealFileInExplorer, phaseTitle }: { events: RunEventRecord[]; process?: ReturnType<typeof runtimeProcess>; groupHelpers?: RuntimeGroupHelpers | null; displayMode?: ProcessDisplayMode; runInProgress?: boolean; running: boolean; answerStarted?: boolean; leading?: ReactNode; grouped?: boolean; outcome?: 'failed' | 'cancelled'; attachmentNames?: ReadonlyMap<string, string>; onOpenFileInSidebar?: (path: string) => void; onRevealFileInExplorer?: (path: string) => void; /** Stage label for the whole Run, computed where its full event list is known. */ phaseTitle?: string }) {
  const [open, setOpen] = useState(false)
  const scrollRef = useRef<HTMLDivElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)
  const process = useMemo(() => preparedProcess ?? runtimeProcess(events), [events, preparedProcess])
  const activity = useMemo(() => runtimeProcessActivity(events, runtimeRunning, answerStarted), [events, runtimeRunning, answerStarted])
  const { running, terminalEvent } = activity
  const hasLifecycle = running || Boolean(terminalEvent) || events.some((item) => (
    item.eventType === 'run.started' || item.eventType === 'message.started'
  ))
  const processStepCount = process.steps.length
  const lifecycleOnly = processStepCount === 0 && hasLifecycle
  const groupOpen = open
  const { edges: scrollEdges, events: scrollEvents, initialize: initializeScroll } = useProcessScroll(
    scrollRef, contentRef, groupOpen, true, events,
  )
  const activeTool = [...process.tools].reverse().find((tool) => activity.activeToolIds.has(tool.id) && !tool.completed && !tool.awaitingUser)
  const lifecycleFailed = terminalEvent?.eventType === 'run.failed' || terminalEvent?.eventType === 'run.interrupted'
  const lifecycleCancelled = terminalEvent?.eventType === 'run.cancelled'
  const awaitingUser = process.tools.some(tool => tool.awaitingUser) || terminalEvent?.eventType === 'run.completed'
    && terminalEvent.event.completionReason === 'awaiting_user'
  const lifecycleLabel = running
    ? phaseTitle ?? '正在理解你的请求'
    : awaitingUser
      ? '等待你的回答'
    : lifecycleFailed
      ? '处理请求时遇到问题'
      : lifecycleCancelled
        ? '请求已取消'
        : '已完成请求处理'
  const liveDetail = grouped && running && displayMode === 'standard'
    ? activeTool ? groupHelpers?.activeProcessDetail(activeTool.name, activeTool.input)
      : activity.reasoning ? groupHelpers?.activeProcessDetail('reasoning', { title: process.reasoning.split(/\n\s*\n/).at(-1)?.replaceAll('**', '') }) : ''
    : ''
  const baseProcessTitle = grouped
    ? outcome === 'failed' ? '过程未完成' : outcome === 'cancelled' ? '过程已取消' : awaitingUser ? '等待你的回答' : running
      ? activeTool
        ? `${groupHelpers?.activeProcessGroupTitle(activeTool.name, activeTool.input) ?? '正在分析请求'}${liveDetail ? ` · ${liveDetail}` : ''}`
        : `${phaseTitle ?? '正在处理'}${liveDetail ? ` · ${liveDetail}` : ''}`
      : groupHelpers?.processGroupTitle(events) ?? '已完成分析'
    : activeTool
    ? toolActivity(activeTool, attachmentNames)
    : running
      ? phaseTitle ?? '正在处理'
      : awaitingUser
        ? '等待你的回答'
      : lifecycleFailed
        ? '工作过程未完成'
        : lifecycleCancelled
          ? '工作过程已取消'
          : '工作过程已完成'
  const desiredTitle = baseProcessTitle
  const processTitle = useStableProcessTitle(desiredTitle, running)
  const activityKind = activeTool ? groupHelpers?.processActivityKind(activeTool.name, activeTool.input)
    : running ? undefined : groupHelpers?.summarizeProcessActivity(events)[0]?.kind
  const GroupIcon = activityKind === 'read' || activityKind === 'readImage' ? FileText : activityKind === 'search' ? Search
    : activityKind === 'write' || activityKind === 'edit' ? FileEdit : activityKind === 'command' || activityKind === 'code' ? Terminal
    : activityKind === 'web' || activityKind === 'webSearch' || activityKind === 'webFetch' ? Globe2 : activityKind === 'question' ? CircleHelp
    : activityKind === 'knowledge' || activityKind === 'localKnowledge' || activityKind === 'remoteKnowledge' ? Database
    : activityKind === 'office' ? FileEdit : activityKind === 'plan' ? ListTodo
    : activityKind === 'subagents' ? Bot : activityKind === 'other' ? Wrench : Brain

  if (processStepCount === 0 && !running && !lifecycleOnly) return leading ? <>{leading}</> : null
  return (
    <ToolTimeline open={groupOpen} onOpenChange={(next) => { if (next) initializeScroll(running ? 'bottom' : 'top'); setOpen(next) }} className="max-w-none fox-chain-of-thought fox-runtime-process"
      leading={leading} groupIcon={<GroupIcon className="fox-runtime-group-icon" size={15} aria-hidden="true" />}
      streaming={running} visibleSteps={processStepCount || 1} restingLabel={processTitle} activeLabel={processTitle} stats={[]}
      renderContent={content => <div className="fox-runtime-process-clip" data-scroll-up={scrollEdges.up || undefined} data-scroll-down={scrollEdges.down || undefined}><div ref={scrollRef} className="fox-runtime-process-scroll" {...scrollEvents}><div ref={contentRef} className="fox-element-process-content">{content}</div></div></div>}
      steps={processStepCount > 0 ? process.steps.map(step => {
        let content: ReactNode
            if (step.kind === 'reasoning') {
              // Only the trailing thought can be live; earlier rows are complete.
              const isLastReasoning = [...process.steps].reverse().find((item) => item.kind === 'reasoning') === step
              const reasoningRunning = isLastReasoning && activity.reasoning
              content = <RuntimeReasoningItem detail={step.text} running={reasoningRunning} preview />
            } else {
            const tool = process.tools.find(item => item.id === step.id)!
            const runState = outcome === 'failed' || lifecycleFailed ? 'failed' : outcome === 'cancelled' || lifecycleCancelled ? 'cancelled' : runInProgress || running ? 'running' : 'completed'
            const attemptState = tool.isError ? toolAttemptState(tool, process.tools, events.reduce((latest, event) => Math.max(latest, event.seq), 0), runState) : undefined
            content = <RuntimeToolItem tool={tool} attemptState={attemptState} active={activity.activeToolIds.has(tool.id) && !tool.completed && !tool.awaitingUser} attachmentNames={attachmentNames} onOpenFileInSidebar={onOpenFileInSidebar} onRevealFileInExplorer={onRevealFileInExplorer} />
            }
        return { id: step.id, verb: '', chip: '', icon: GroupIcon, content }
      }) : [{ id: 'lifecycle', verb: lifecycleLabel, chip: '', icon: GroupIcon, content: <div className={`fox-runtime-empty-step ${running ? 'is-running' : 'is-complete'}`} role="status"><RunStatusText text={lifecycleLabel} active={running} /></div> }]} />
  )
}
