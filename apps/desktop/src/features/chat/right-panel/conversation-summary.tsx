import { useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from 'react'
import { DndContext, DragOverlay, PointerSensor, closestCenter, useDraggable, useDroppable, useSensor, useSensors } from '@dnd-kit/core'
import { motion, useReducedMotion } from 'motion/react'
import { AlertTriangle, BookOpen, Check, ChevronDown, ChevronRight, Circle, Copy, FileText, FolderOpen, GripVertical, LoaderCircle, Minus, RotateCcw } from 'lucide-react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { command, desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { ArtifactRecord, ConversationDetail, WorkTaskRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { copyTextWithFeedback, type RightMode } from '../fox-shared'
import { runtimeProcess } from '../timeline/fox-runtime-process-model'
import { BackgroundTaskSummary, useBackgroundTasks } from './background-tasks'
import { useContextBudget, type RuntimeContextWindow } from '../hooks/use-context-budget'
import { compactTokenCount, contextBudgetRows, pressurePercent, type ContextBudgetDto } from '../components/context-budget-presentation'
import { useGoalProgress } from '../hooks/use-goal-progress'
import { SummaryAgents } from './summary-agents'

export const SUMMARY_ORDER_KEY = 'fox.summary.order'
export const SUMMARY_GROUPS = ['project', 'model', 'tasks', 'files', 'processes', 'agents', 'sources'] as const
type GroupId = typeof SUMMARY_GROUPS[number]
type SummaryRow = { id: string; title: string; status: string; icon: ReactNode }
type SummaryGroup = { id: GroupId; title: string; value: ReactNode; rows?: SummaryRow[]; mode?: RightMode; content?: ReactNode; tone?: string; hidden?: boolean }

export function readSummaryOrder(): GroupId[] {
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(SUMMARY_ORDER_KEY) || 'null')
    const known = Array.isArray(raw) ? raw.filter((id): id is GroupId => SUMMARY_GROUPS.includes(id)) : []
    return [...new Set([...known, ...SUMMARY_GROUPS])]
  } catch { return [...SUMMARY_GROUPS] }
}

function legacyContextRows(budget: ContextBudgetDto) {
  const sourceRows = contextBudgetRows(budget)
  return ['history', 'tools', 'system', 'append', 'reserve', 'margin'].map(key => {
    const row = sourceRows.find(row => row.key === key)!
    return key === 'margin' ? { ...row, label: '安全余量', tokens: row.tokens + (budget.protocolOverheadTokens ?? 0) } : row
  }).filter(row => row.tokens > 0)
}

function runtimeContextRows(runtime: RuntimeContextWindow, budget: ContextBudgetDto | null) {
  const system = Math.min(runtime.usedTokens, runtime.systemTokens ?? budget?.systemTokens ?? 0)
  const tools = Math.min(runtime.usedTokens - system, runtime.toolsTokens ?? budget?.toolsTokens ?? 0)
  return [
    { key: 'history', label: system || tools ? '消息与其他上下文' : '已用上下文', tokens: runtime.usedTokens - system - tools },
    { key: 'tools', label: '工具定义', tokens: tools },
    { key: 'system', label: '系统提示词', tokens: system },
  ].filter(row => row.tokens > 0)
}

const contextCellCount = 144

export function SummaryContextBudget({ budget, runtime }: { budget: ContextBudgetDto | null; runtime?: RuntimeContextWindow | null }) {
  const [highlight, setHighlight] = useState<string>()
  if (!runtime && !budget) return <p className="fox-side-note">暂无上下文用量。模型响应后更新。</p>
  const limit = runtime?.limitTokens ?? budget!.modelWindowTokens
  const used = runtime?.usedTokens ?? Math.max(0, limit - budget!.availableTokens)
  const free = Math.max(0, limit - used)
  const fraction = (tokens: number) => limit > 0 ? tokens / limit * 100 : 0
  const percent = fraction(used)
  const rows = runtime ? runtimeContextRows(runtime, budget) : legacyContextRows(budget!)
  const classified = rows.some(row => row.key === 'tools' || row.key === 'system')
  const legend = [...rows, { key: 'free', label: '剩余可用', tokens: free }]
  let tokensThrough = 0
  const boundaries = rows.map(row => {
    tokensThrough += row.tokens
    return { key: row.key, end: Math.min(contextCellCount, Math.ceil(fraction(tokensThrough) * contextCellCount / 100)) }
  })
  return <div className="fox-summary-context">
    <div className="fox-summary-context-top" data-pressure={percent >= 90 ? 'high' : percent >= 80 ? 'warning' : undefined}>
      <strong>{percent.toFixed(1).replace(/\.0$/u, '')}%</strong><span>上下文已用</span>
      <small title={`${used.toLocaleString()} / ${limit.toLocaleString()} Tokens`}>{compactTokenCount(used)} / {compactTokenCount(limit)}</small>
    </div>
    <div className="fox-summary-context-grid" role="meter" aria-label="上下文占用" aria-valuenow={Math.min(100, percent)} aria-valuemin={0} aria-valuemax={100} aria-valuetext={`${used.toLocaleString()} / ${limit.toLocaleString()} Tokens`}>
      {Array.from({ length: contextCellCount }, (_, index) => {
        const key = boundaries.find(boundary => index < boundary.end)?.key ?? 'free'
        return <i key={index} className={`is-${key}`} data-muted={highlight && key !== highlight ? true : undefined} aria-hidden="true" />
      })}
    </div>
    {legend.map(row => <div className={`fox-summary-context-row${row.key === 'free' ? ' is-free' : ''}`} key={row.key} onMouseEnter={() => setHighlight(row.key)} onMouseLeave={() => setHighlight(undefined)}><i className={`is-${row.key}`} /><span>{row.label}</span><b title={row.tokens.toLocaleString()}>{compactTokenCount(row.tokens)}</b><small>{fraction(row.tokens).toFixed(1)}%</small></div>)}
    {runtime && <p className="fox-summary-context-note">{runtime.source === 'session' ? classified ? '当前会话' : '按当前会话估算' : '按最近一次主模型请求统计，含缓存'}{classified && ' · 分类占用为估算'}</p>}
  </div>
}

function SummaryProjectFolder({ path, conversationId }: { path: string; conversationId: string }) {
  return <DropdownMenu modal={false}>
    <DropdownMenuTrigger asChild>
      <button type="button" className="fox-side-text-button fox-summary-project-trigger" title={path} aria-label="项目目录操作">
        <FolderOpen aria-hidden="true" /><span>{path}</span><ChevronDown aria-hidden="true" />
      </button>
    </DropdownMenuTrigger>
    <DropdownMenuContent className="fox-top-menu fox-summary-project-menu">
      <DropdownMenuLabel className="fox-summary-project-path">{path}</DropdownMenuLabel>
      <DropdownMenuSeparator />
      <DropdownMenuItem onSelect={() => void desktopClient.openProjectFolder(conversationId).catch(() => toast.error('无法打开项目文件夹，请重试。'))}><FolderOpen />打开项目文件夹</DropdownMenuItem>
      <DropdownMenuItem onSelect={() => void copyTextWithFeedback(path, '已复制项目路径')}><Copy />复制路径</DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>
}

type SummaryFile = Pick<ArtifactRecord, 'id' | 'displayName' | 'createdAt' | 'updatedAt'>
const emptySummaryFiles: SummaryFile[] = []

function useSummaryFiles(detail: ConversationDetail | null) {
  const conversationId = detail?.conversation.id
  const loaded = detail?.artifacts ?? emptySummaryFiles
  const hasEarlierMessages = detail?.hasEarlierMessages
  const [history, setHistory] = useState<{ conversationId: string; files: SummaryFile[] }>()
  useEffect(() => {
    if (!conversationId || !hasEarlierMessages || !desktopRuntimeAvailable) return
    let current = true
    void command<SummaryFile[]>('conversation_summary_files', { conversationId }, true).then(files => {
      if (current) setHistory({ conversationId, files })
    }).catch(() => {})
    return () => { current = false }
  }, [conversationId, hasEarlierMessages, loaded])
  return hasEarlierMessages && history && history.conversationId === conversationId ? history.files : loaded
}

function SummaryFiles({ artifacts }: { artifacts: SummaryFile[] }) {
  const [limit, setLimit] = useState(5)
  const files = artifacts.toSorted((a, b) => b.createdAt - a.createdAt || b.updatedAt - a.updatedAt)
  return <>
    {files.slice(0, limit).map(file => <div className="fox-summary-item" key={file.id}><FileText /><span title={file.displayName}>{file.displayName}</span></div>)}
    {files.length > 5 && <div className="fox-summary-file-actions">
      {files.length > limit && <button type="button" className="fox-side-text-button" onClick={() => setLimit(limit === 5 ? 10 : Infinity)}>查看更多 <ChevronDown /></button>}
      {limit > 5 && <button type="button" className="fox-side-text-button" onClick={() => setLimit(5)}>收起</button>}
    </div>}
  </>
}

type PlanStep = Pick<WorkTaskRecord, 'id' | 'title' | 'status'>

function SummaryPlan({ steps }: { steps: PlanStep[] }) {
  return <ol className="fox-summary-list">
    {steps.map(({ id, title, status }) => {
      const Icon = status === 'completed' ? Check : status === 'in_progress' ? LoaderCircle
        : status === 'blocked' || status === 'interrupted' ? AlertTriangle : status === 'skipped' ? Minus : Circle
      return <li className={`fox-summary-item fox-summary-plan-step is-${status}`} key={id}>
        <div className="fox-summary-plan-marker"><Icon className={status === 'in_progress' ? 'motion-safe:animate-spin' : undefined} aria-hidden="true" /></div>
        <span>{title}</span>
        <small className={status === 'queued' || status === 'completed' ? 'sr-only' : undefined}>{summaryStatusLabel(status)}</small>
      </li>
    })}
  </ol>
}

function SortableSummaryGroup({ group, open, lifted, onToggle, onKeyDown, onMode }: { group: SummaryGroup; open: boolean; lifted: boolean; onToggle: () => void; onKeyDown: (event: KeyboardEvent<HTMLButtonElement>) => void; onMode: (mode: RightMode) => void }) {
  const { attributes, listeners, setNodeRef: dragRef, setActivatorNodeRef } = useDraggable({ id: group.id })
  const { setNodeRef: dropRef } = useDroppable({ id: group.id })
  const reduced = useReducedMotion()
  return <motion.section layout={reduced ? false : 'position'} transition={{ duration: reduced ? 0 : 0.16 }} ref={node => { dragRef(node); dropRef(node) }} className={`fox-summary-group${lifted ? ' is-placeholder' : ''}`} aria-label={group.title} data-group={group.id}>
    <div className="fox-summary-row">
      <button type="button" className="fox-summary-toggle" aria-expanded={open} onClick={onToggle}><span>{group.title}</span><ChevronRight className={open ? 'is-open' : ''} /><span className={`fox-summary-value ${group.tone || ''}`}>{group.value}</span></button>
      <button type="button" className="fox-summary-grip" ref={setActivatorNodeRef} {...attributes} {...listeners} onKeyDown={onKeyDown} aria-label={`拖动调整「${group.title}」的顺序`}><GripVertical /></button>
    </div>
    {open && <div className="fox-summary-expanded">
      {group.content}
      {group.rows?.slice(0, 3).map(row => <div className={`fox-summary-item is-${row.status}`} key={row.id}>{row.icon}<span title={row.title}>{row.title}</span>{row.status && <small>{summaryStatusLabel(row.status)}</small>}</div>)}
      {group.mode && group.rows && group.rows.length > 0 && <button type="button" className="fox-side-text-button" onClick={() => onMode(group.mode!)}>查看全部 {group.rows.length} 条 <ChevronRight /></button>}
    </div>}
  </motion.section>
}

export function ConversationSummary({ detail, modelName, onMode }: { detail: ConversationDetail | null; modelName: string; onMode: (mode: RightMode) => void }) {
  const background = useBackgroundTasks(detail)
  const running = ['running', 'streaming', 'queued', 'pending'].includes(detail?.lastRun?.status ?? '')
  const { budget, snapshot } = useContextBudget(detail?.conversation.id, running, Boolean(detail?.lastRun || detail?.messages?.length))
  const contextWindow = snapshot?.contextWindow
  const [order, setOrder] = useState(readSummaryOrder)
  const [collapsedGroups, setCollapsedGroups] = useState<Partial<Record<GroupId, boolean>>>({})
  const [dragged, setDragged] = useState<GroupId | null>(null)
  const [keyboardDrag, setKeyboardDrag] = useState<GroupId | null>(null)
  const [announcement, setAnnouncement] = useState('')
  const originalOrder = useRef(order)
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }))
  const reduced = useReducedMotion()
  const goal = useGoalProgress(detail)
  const pendingPlan = goal?.planRevisions.find(plan => plan.status === 'proposed')
  const tasks: PlanStep[] = pendingPlan ? pendingPlan.tasks.flatMap((step, index) => (
    step && typeof step === 'object' && 'title' in step && typeof step.title === 'string' && step.title.trim()
      ? [{ id: `${pendingPlan.id}:${index}`, title: step.title, status: 'queued' }]
      : []
  )) : goal?.tasks ?? (detail?.goals?.length ? [] : detail?.tasks ?? [])
  const artifacts = useSummaryFiles(detail)
  const agents = detail?.childRuns ?? []
  const sources = runtimeProcess(detail?.runtimeEvents ?? []).sources
  const bindings = (detail?.knowledgeBindings ?? []).filter(binding => binding.enabled !== false)
  const completed = tasks.filter(task => task.status === 'completed').length
  const pendingAgentIds = new Set((detail?.approvals ?? []).filter(approval => approval.status === 'pending').map(approval => approval.conversationId))
  const pendingAgents = agents.filter(agent => pendingAgentIds.has(agent.childConversationId)).length
  const projectPath = detail?.conversation.projectRoot?.replace(/^[\\/]{2}\?[\\/]UNC[\\/]/iu, '\\\\').replace(/^[\\/]{2}\?[\\/]/u, '')
  const pressure = contextWindow ? Math.round(contextWindow.usedTokens / contextWindow.limitTokens * 100) : budget ? pressurePercent(budget) : 0
  const groups: SummaryGroup[] = [
    { id: 'project', title: '项目', value: projectPath?.split(/[\\/]/).filter(Boolean).at(-1) || '未绑定项目',
      content: projectPath && detail ? <SummaryProjectFolder path={projectPath} conversationId={detail.conversation.id} />
        : <p className="fox-side-note">当前对话未绑定项目。</p> },
    { id: 'model', title: '模型', value: <>{modelName}{pressure >= 80 && <span className={pressure >= 90 ? 'is-error' : 'is-warning'}>{pressure}%</span>}</>, content: <SummaryContextBudget budget={budget} runtime={contextWindow} /> },
    { id: 'tasks', title: '任务进度', value: pendingPlan ? '等待批准' : `${completed} / ${tasks.length}`, hidden: !tasks.length, content: <SummaryPlan steps={tasks} /> },
    { id: 'files', title: '生成文件', value: `${artifacts.length} 个`, hidden: !artifacts.length, content: <SummaryFiles key={detail?.conversation.id} artifacts={artifacts} /> },
    { id: 'processes', title: '后台任务', value: background.running.length ? `${background.running.length} 个运行中` : null, hidden: !background.running.length && !background.finished.length, content: <BackgroundTaskSummary {...background} onOpen={() => onMode('background')} /> },
    { id: 'agents', title: '协作助手', value: pendingAgents ? `${pendingAgents} 个需要你批准` : `${agents.length} 个`, tone: pendingAgents ? 'is-warning' : '', hidden: !agents.length, content: <SummaryAgents agents={agents} pendingAgentIds={pendingAgentIds} onOpen={() => onMode('agents')} /> },
    { id: 'sources', title: '资料来源', mode: 'knowledge', value: `${sources.length + bindings.length} 条`, rows: [...bindings.map(binding => ({ id: binding.knowledgeBaseId, title: binding.knowledgeBaseName || binding.knowledgeBaseId, status: '', icon: <BookOpen /> })), ...sources.map(source => ({ id: source.id, title: source.title, status: '', icon: <BookOpen /> }))] },
  ]
  const visible = order.map(id => groups.find(group => group.id === id)!).filter(group => !group.hidden && (!group.rows || group.rows.length > 0))
  const save = (next: GroupId[]) => { setOrder(next); localStorage.setItem(SUMMARY_ORDER_KEY, JSON.stringify(next)) }
  const move = (id: GroupId, over: GroupId) => {
    setOrder(current => { const next = [...current]; next.splice(next.indexOf(id), 1); next.splice(current.indexOf(over), 0, id); return next })
  }
  const keyboard = (event: KeyboardEvent<HTMLButtonElement>, id: GroupId) => {
    if (![' ', 'Enter', 'Escape', 'ArrowUp', 'ArrowDown'].includes(event.key)) return
    event.preventDefault()
    if (!keyboardDrag && (event.key === ' ' || event.key === 'Enter')) { originalOrder.current = order; setKeyboardDrag(id); setAnnouncement('已拿起，使用上下方向键调整顺序'); return }
    if (keyboardDrag !== id) return
    if (event.key === 'Escape') { setOrder(originalOrder.current); setKeyboardDrag(null); setAnnouncement('已取消调整'); return }
    if (event.key === ' ' || event.key === 'Enter') { save(order); setKeyboardDrag(null); setAnnouncement(`已放下，位于第 ${visible.findIndex(group => group.id === id) + 1} 位`); return }
    const index = visible.findIndex(group => group.id === id)
    const target = index + (event.key === 'ArrowUp' ? -1 : 1)
    if (visible[target]) { move(id, visible[target].id); setAnnouncement(`已移到第 ${target + 1} 位`) }
  }
  const draggedGroup = groups.find(group => group.id === dragged)
  return <div className="fox-summary-page">
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragStart={({ active }) => { originalOrder.current = order; setDragged(active.id as GroupId) }} onDragOver={({ active, over }) => { if (over && active.id !== over.id) move(active.id as GroupId, over.id as GroupId) }} onDragEnd={() => { save(order); setDragged(null); setAnnouncement('顺序已保存') }} onDragCancel={() => { setOrder(originalOrder.current); setDragged(null) }}>
      {visible.map(group => <SortableSummaryGroup key={group.id} group={group} open={!collapsedGroups[group.id]} lifted={dragged === group.id} onMode={onMode} onKeyDown={event => keyboard(event, group.id)} onToggle={() => setCollapsedGroups(current => ({ ...current, [group.id]: !current[group.id] }))} />)}
      <DragOverlay dropAnimation={reduced ? null : undefined}>{draggedGroup && <div className="fox-summary-drag-overlay" style={{ rotate: reduced ? undefined : '-0.6deg' }}><span>{draggedGroup.title}</span><ChevronRight /><span className="fox-summary-value">{draggedGroup.value}</span><GripVertical /></div>}</DragOverlay>
    </DndContext>
    <span className="sr-only" role="status" aria-live="polite">{announcement}</span>
    {visible.length === 2 && <p className="fox-side-note">开始对话后，任务计划、生成文件和协作助手会显示在这里。</p>}
    {order.some((id, index) => id !== SUMMARY_GROUPS[index]) && <button type="button" className="fox-side-text-button fox-summary-reset" onClick={() => { save([...SUMMARY_GROUPS]); setAnnouncement('已恢复默认顺序') }}><RotateCcw />恢复默认顺序</button>}
  </div>
}

function summaryStatusLabel(status: string) {
  const labels: Record<string, string> = { completed: '已完成', running: '进行中', in_progress: '进行中', queued: '待开始', pending: '等待处理', awaiting_approval: '等待批准', awaiting_question: '等待回答', blocked: '已暂停', failed: '失败', denied: '已拒绝', cancelled: '已取消', cancelling: '取消中', interrupted: '已中断', skipped: '已跳过' }
  return labels[status] ?? status
}
