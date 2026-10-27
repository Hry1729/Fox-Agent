import { activeModelWaiting, modelWaitingTitle } from '../run-status-facts'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Popover, PopoverAnchor, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Toggle } from '@/components/animate-ui/components/radix/toggle'
import { AnimatedIconTrigger, PanelRight } from '@/components/fox-ui/animated-icons'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { conversationRunState } from '@/features/conversations/model/kernel-snapshot'
import { type ConversationDetail, type ConversationSummary } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { Activity, AlertTriangle, Archive, BookOpen, Bot, ChevronRight, CircleHelp, CircleStop, FileEdit, FileText, Folder, FolderOpen, ListTodo, MoreHorizontal, Pin, PinOff, RotateCcw, Settings, ShieldCheck, Terminal } from 'lucide-react'
import { type ReactNode, useId, useRef, useState } from 'react'
import { type ChatState, type RightMode } from '../fox-shared'
import { runtimeProcess } from '../timeline/fox-runtime-process-model'

function ConversationSummaryPopover({ detail, modelName, onOpenRightMode }: { detail: ConversationDetail | null; modelName: string; onOpenRightMode: (mode: RightMode) => void }) {
  const [open, setOpen] = useState(false)
  const summaryId = useId()
  const anchorRootRef = useRef<HTMLSpanElement>(null)
  // The chat pane is display:none when the right panel is maximized. Anchor
  // to its visible surface instead of letting the hidden button report 0,0.
  // Read geometry on every position update so resize/sidebar changes follow.
  const summaryAnchor = useRef({
    getBoundingClientRect: () => {
      const toggle = anchorRootRef.current
      const surface = toggle?.closest('.fox-content-surface')
      const pane = toggle?.closest('.fox-chat-pane')
      const paneRect = pane?.getBoundingClientRect()
      const rect = paneRect?.width ? paneRect : surface?.getBoundingClientRect()
      return rect ? new DOMRect(rect.right - 12, rect.top + 49, 0, 0) : new DOMRect()
    },
  })
  const artifacts = detail?.artifacts ?? []
  const tasks = detail?.tasks ?? []
  const activeProcesses = (detail?.toolCalls ?? []).filter((tool) => !['completed', 'failed', 'denied', 'cancelled'].includes(tool.status))
  const subAgents = detail?.childRuns ?? []
  const sources = runtimeProcess(detail?.runtimeEvents ?? []).sources
  // Only things a person recognises: produced files, the plan, running
  // processes, sub-agents and sources. Context-window accounting (percent used,
  // remaining room, the usage track), token counts, provider identity,
  // phase/retry/compaction counters and tool-call counts are internal facts:
  // they stay in the durable records, not in an ordinary chat header.
  const summaryItems: Array<{ icon: ReactNode; label: string; count: number; detail: string; mode: RightMode }> = [
    { icon: <FileText size={15} />, label: '生成文件', count: artifacts.length, detail: artifacts.slice(0, 2).map((item) => item.displayName).join('、'), mode: 'files' },
    { icon: <ListTodo size={15} />, label: '任务计划', count: tasks.length, detail: tasks.slice(0, 2).map((item) => item.title).join('、'), mode: 'activity' },
    { icon: <Terminal size={15} />, label: '后台程序', count: activeProcesses.length, detail: activeProcesses.slice(0, 2).map((item) => item.toolName).join('、'), mode: 'activity' },
    { icon: <Bot size={15} />, label: '协作助手', count: subAgents.length, detail: subAgents.slice(-2).map((item) => `${item.workerAgentName} · ${item.status}`).join('、'), mode: 'agents' },
    { icon: <BookOpen size={15} />, label: '资料来源', count: sources.length, detail: sources.slice(0, 2).map((item) => item.title).join('、'), mode: 'knowledge' },
  ]
  return (
    // Only the summary toggles close this controlled, non-modal popover.
    // Keep a toggle inside the portal too: a maximized right panel hides the
    // chat header, but outside clicks, Escape and item actions must not close it.
    <Popover open={open} modal={false}>
      <PopoverAnchor virtualRef={summaryAnchor} />
      <span ref={anchorRootRef} className="inline-flex">
        <Tooltip>
          <TooltipTrigger asChild><Toggle size="icon" className="fox-topbar-toggle fox-icon-button" pressed={open} onPressedChange={setOpen} aria-label="对话摘要" aria-expanded={open} aria-controls={summaryId}><Activity size={16} /></Toggle></TooltipTrigger>
          <TooltipContent side="bottom"><p>对话摘要</p></TooltipContent>
        </Tooltip>
      </span>
      <PopoverContent id={summaryId} aria-label="对话摘要" align="end" sideOffset={8} updatePositionStrategy="always" className="fox-conversation-summary"
        onOpenAutoFocus={(event) => event.preventDefault()} onCloseAutoFocus={(event) => event.preventDefault()}
        onInteractOutside={(event) => event.preventDefault()} onEscapeKeyDown={(event) => event.preventDefault()}>
        <div className="fox-summary-model">
          <div><span>模型</span><strong title={modelName}>{modelName}</strong></div>
          <Toggle size="icon" className="fox-topbar-toggle fox-icon-button" pressed={open} onPressedChange={setOpen} aria-label="收起对话摘要" aria-expanded={open} aria-controls={summaryId}><Activity size={16} /></Toggle>
        </div>
        <div className="fox-summary-list">
          {summaryItems.map((item) => <button type="button" key={item.label} className="fox-summary-item" aria-label={`打开${item.label}侧边栏`} onClick={() => onOpenRightMode(item.mode)}>
            <span className="fox-summary-icon">{item.icon}</span>
            <span><strong>{item.label}</strong><small>{item.detail || '暂无'}</small></span>
            <span className="fox-summary-item-tail"><b>{item.count}</b><ChevronRight /></span>
          </button>)}
        </div>
      </PopoverContent>
    </Popover>
  )
}

/**
 * Where the "重置上下文" item is offered.
 *
 * The Host owns the boundary (Yuxi conversations keep their context remotely,
 * a legacy `format_version 1` snapshot is not a Pi session, and a live Run owns
 * the session a reset would retire). The menu only mirrors it, with the Host's
 * own reason, so a click can never be the first time the user learns a reset is
 * unavailable.
 */
function resetContextItemState(
  detail: ConversationDetail | null,
  state: ChatState,
  conversation: ConversationSummary | null | undefined,
): { available: boolean; reason: string | null } {
  if (!conversation) return { available: false, reason: '先打开一个对话' }
  const availability = detail?.contextReset
  if (availability && !availability.supported) {
    return { available: false, reason: availability.reason ?? '该对话暂不支持重置上下文' }
  }
  if (state === 'running') {
    return { available: false, reason: detail?.contextResetBlockedReason ?? '当前对话正在运行，请先停止生成' }
  }
  if (detail?.contextResetBlockedReason) {
    return { available: false, reason: detail.contextResetBlockedReason }
  }
  return { available: true, reason: null }
}

export function ChatTopbar({ rightSidebarCollapsed, onRightSidebarExpand, onRightSidebarCollapse, onOpenRightMode, conversation, detail, modelName, state, onPinConversation, onRenameConversation, onResetContext, onArchiveConversation }: { rightSidebarCollapsed: boolean; onRightSidebarExpand: () => void; onRightSidebarCollapse: () => void; onOpenRightMode: (mode: RightMode) => void; conversation?: ConversationSummary | null; detail: ConversationDetail | null; modelName: string; state: ChatState; onPinConversation: (conversation: ConversationSummary) => void; onRenameConversation: (conversation: ConversationSummary) => void; onResetContext: (conversation: ConversationSummary) => void; onArchiveConversation: (conversation: ConversationSummary) => void }) {
  const [projectPopoverOpen, setProjectPopoverOpen] = useState(false)
  const waiting = detail && detail.conversation.id === conversation?.id
    ? activeModelWaiting(detail.runtimeEvents, { runId: detail.lastRun?.id, runState: conversationRunState(detail) }) : null
  const resetContextItem = resetContextItemState(detail, state, conversation)
  const projectRoot = conversation?.projectRoot?.trim() ?? ''
  const normalizedProjectRoot = projectRoot.replace(/[\\/]+$/, '')
  const projectName = normalizedProjectRoot.split(/[\\/]/).at(-1) || '未绑定项目'
  const openProjectFolder = async () => {
    if (!conversation || !projectRoot) return
    try {
      await desktopClient.openProjectFolder(conversation.id)
      setProjectPopoverOpen(false)
    } catch (cause) {
      toast.error('无法打开项目文件夹', { description: cause instanceof Error ? cause.message : String(cause) })
    }
  }
  return (
    <header className="fox-chat-topbar">
      <div className="fox-workspace-heading">
        <div className="fox-chat-heading-main">
          <Popover open={projectPopoverOpen} onOpenChange={setProjectPopoverOpen}>
            <PopoverTrigger asChild><button type="button" className="fox-chat-project-trigger" aria-label="查看对话项目"><FolderOpen size={14} /></button></PopoverTrigger>
            <PopoverContent align="start" sideOffset={7} className="fox-chat-project-popover">
              <div className="fox-chat-project-row is-summary"><Folder size={15} /><strong>{projectName}</strong></div>
              <button type="button" className="fox-chat-project-row" disabled={!projectRoot} title={projectRoot || '当前对话未绑定项目目录'} onClick={() => void openProjectFolder()}><FolderOpen size={15} /><span>{projectRoot || '当前对话未绑定项目目录'}</span></button>
              <button type="button" className="fox-chat-project-row" disabled={!conversation} onClick={() => { if (!conversation) return; setProjectPopoverOpen(false); onRenameConversation(conversation) }}><Settings size={15} /><span>编辑名称</span></button>
            </PopoverContent>
          </Popover>
          <span className="fox-chat-title">{conversation?.title || '新对话'}</span>
          <DropdownMenu>
            <Tooltip>
              <TooltipTrigger asChild><DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-icon-button fox-chat-title-more" data-testid="conversation-menu" aria-label="更多"><MoreHorizontal size={16} /></Button></DropdownMenuTrigger></TooltipTrigger>
              <TooltipContent side="bottom"><p>更多</p></TooltipContent>
            </Tooltip>
            <DropdownMenuContent align="start" className="fox-top-menu">
              <DropdownMenuLabel>当前对话</DropdownMenuLabel>
              <DropdownMenuItem disabled={!conversation} onSelect={() => conversation && onPinConversation(conversation)}>{conversation?.pinned ? <PinOff /> : <Pin />}{conversation?.pinned ? '取消置顶' : '置顶对话'}</DropdownMenuItem>
              <DropdownMenuItem disabled={!conversation} onSelect={() => conversation && onRenameConversation(conversation)}><FileEdit />重命名</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                disabled={!resetContextItem.available}
                title={resetContextItem.reason ?? undefined}
                data-testid="reset-context"
                aria-label={resetContextItem.reason ? `重置上下文（不可用：${resetContextItem.reason}）` : '重置上下文'}
                onSelect={() => conversation && resetContextItem.available && onResetContext(conversation)}
              ><RotateCcw />重置上下文{!resetContextItem.available && resetContextItem.reason ? <span className="fox-menu-hint">{resetContextItem.reason}</span> : null}</DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem disabled={!conversation} onSelect={() => conversation && onArchiveConversation(conversation)}><Archive />归档对话</DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      <div className="fox-chat-top-actions">
        <ConversationSummaryPopover detail={detail} modelName={modelName} onOpenRightMode={onOpenRightMode} />
        {state === 'running' && <Badge className="fox-running-badge" title={waiting ? modelWaitingTitle(waiting) : undefined}><span />{waiting ? modelWaitingTitle(waiting) : '运行中'}</Badge>}
        {state === 'question' && <Badge className="fox-question-badge"><CircleHelp size={12} />需要你回答</Badge>}
        {state === 'approval' && <Badge className="fox-approval-badge"><ShieldCheck size={12} />需要你批准</Badge>}
        {state === 'error' && <Badge className="fox-error-badge"><AlertTriangle size={12} />处理失败</Badge>}
        {state === 'denied' && <Badge className="fox-denied-badge"><CircleStop size={12} />已取消</Badge>}
        <Tooltip><TooltipTrigger asChild><AnimatedIconTrigger><Toggle size="icon" className="fox-topbar-toggle fox-right-sidebar-expand" aria-label={rightSidebarCollapsed ? "展开右侧边栏" : "收起右侧边栏"} aria-expanded={!rightSidebarCollapsed} pressed={!rightSidebarCollapsed} onPressedChange={(pressed) => pressed ? onRightSidebarExpand() : onRightSidebarCollapse()}><PanelRight size={16} /></Toggle></AnimatedIconTrigger></TooltipTrigger><TooltipContent side="bottom">{rightSidebarCollapsed ? "展开右侧边栏" : "收起右侧边栏"}</TooltipContent></Tooltip>
      </div>
    </header>
  )
}
