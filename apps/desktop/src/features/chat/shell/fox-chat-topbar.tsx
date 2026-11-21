import { activeModelWaiting, modelWaitingTitle } from '../run-status-facts'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { conversationRunState } from '@/features/conversations/model/kernel-snapshot'
import { type ConversationDetail, type ConversationSummary, type ProjectRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { AlertTriangle, Archive, Check, CircleHelp, CircleStop, FileEdit, ListCollapse, PanelRight, Folder, FolderInput, FolderOpen, FolderPlus, FolderX, MoreHorizontal, Pin, PinOff, RotateCcw, Settings, ShieldCheck } from 'lucide-react'
import { useState } from 'react'
import { type ChatState, type RightMode } from '../fox-shared'

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

/** 13.38 R2-4：「更换项目」什么时候能用。运行中不能换（Host 同样会拒绝）。 */
export function changeProjectItemState(state: ChatState, conversation: ConversationSummary | null | undefined): { available: boolean; reason: string | null } {
  if (!conversation) return { available: false, reason: '先打开一个对话' }
  if (state === 'running') return { available: false, reason: '对话正在运行，请先停止' }
  return { available: true, reason: null }
}

function sameRoot(left: string | null | undefined, right: string | null | undefined) {
  const normalize = (value: string | null | undefined) => (value ?? '').trim().replace(/[\\/]+$/, '').replace(/\\/g, '/').toLowerCase()
  return Boolean(left && right) && normalize(left) === normalize(right)
}

/** 「更换项目」子菜单的项目列表：当前项目标出来且不可选（路径大小写、分隔符、末尾斜杠不敏感）。 */
export function changeProjectEntries(projects: readonly ProjectRecord[], currentRoot: string | null | undefined) {
  return projects.map((project) => ({ project, current: sameRoot(project.rootPath, currentRoot) }))
}

export function ChatTopbar({ summaryOpen = false, rightSidebarCollapsed, onRightSidebarExpand, onRightSidebarCollapse, onOpenSummary, conversation, detail, state, onPinConversation, onRenameConversation, onResetContext, onArchiveConversation, projects = [], onChangeProject, onPickProjectFolder }: { summaryOpen?: boolean; rightSidebarCollapsed: boolean; onRightSidebarExpand: () => void; onOpenSummary?: () => void; onRightSidebarCollapse: () => void; onOpenRightMode: (mode: RightMode) => void; conversation?: ConversationSummary | null; detail: ConversationDetail | null; modelName: string; state: ChatState; onPinConversation: (conversation: ConversationSummary) => void; onRenameConversation: (conversation: ConversationSummary) => void; onResetContext: (conversation: ConversationSummary) => void; onArchiveConversation: (conversation: ConversationSummary) => void; /** 「更换项目」子菜单里列出的项目。 */ projects?: readonly ProjectRecord[]; onChangeProject?: (conversation: ConversationSummary, projectRoot: string | null, permissionMode?: ProjectRecord['permissionMode']) => void; onPickProjectFolder?: (conversation: ConversationSummary) => void }) {
  const [projectPopoverOpen, setProjectPopoverOpen] = useState(false)
  const waiting = detail && detail.conversation.id === conversation?.id
    ? activeModelWaiting(detail.runtimeEvents, { runId: detail.lastRun?.id, runState: conversationRunState(detail) }) : null
  const resetContextItem = resetContextItemState(detail, state, conversation)
  const changeProjectItem = changeProjectItemState(state, conversation)
  const canChangeProject = Boolean(onChangeProject)
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
              {canChangeProject && (changeProjectItem.available ? (
                <DropdownMenuSub>
                  <DropdownMenuSubTrigger data-testid="change-project"><FolderInput />更换项目</DropdownMenuSubTrigger>
                  <DropdownMenuSubContent className="fox-top-menu fox-change-project-menu">
                    {changeProjectEntries(projects, conversation?.projectRoot).map(({ project, current }) => {
                      return <DropdownMenuItem key={project.id} disabled={current} title={project.rootPath}
                        onSelect={() => conversation && !current && onChangeProject?.(conversation, project.rootPath, project.permissionMode)}>
                        <Folder /><span className="fox-change-project-name">{project.name}</span>{current ? <Check className="ml-auto" aria-label="当前项目" /> : null}
                      </DropdownMenuItem>
                    })}
                    {projects.length > 0 && <DropdownMenuSeparator />}
                    <DropdownMenuItem onSelect={() => conversation && runAfterMenuClose(() => onPickProjectFolder?.(conversation))}><FolderPlus />选择文件夹…</DropdownMenuItem>
                    {projectRoot && <DropdownMenuItem onSelect={() => conversation && onChangeProject?.(conversation, null)}><FolderX />不绑定项目</DropdownMenuItem>}
                  </DropdownMenuSubContent>
                </DropdownMenuSub>
              ) : (
                <DropdownMenuItem disabled data-testid="change-project" title={changeProjectItem.reason ?? undefined}
                  aria-label={`更换项目（不可用：${changeProjectItem.reason ?? ''}）`}><FolderInput />更换项目{changeProjectItem.reason ? <span className="fox-menu-hint">{changeProjectItem.reason}</span> : null}</DropdownMenuItem>
              ))}
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
        {state === 'running' && <Badge className="fox-running-badge" title={waiting ? modelWaitingTitle(waiting) : undefined}><span />{waiting ? modelWaitingTitle(waiting) : '运行中'}</Badge>}
        {state === 'question' && <Badge className="fox-question-badge"><CircleHelp size={12} />需要你回答</Badge>}
        {state === 'approval' && <Badge className="fox-approval-badge"><ShieldCheck size={12} />需要你批准</Badge>}
        {state === 'error' && <Badge className="fox-error-badge"><AlertTriangle size={12} />处理失败</Badge>}
        {state === 'denied' && <Badge className="fox-denied-badge"><CircleStop size={12} />已取消</Badge>}
        <Tooltip><TooltipTrigger asChild><Button variant="ghost" size="icon" className="fox-topbar-toggle" aria-label="对话摘要" aria-pressed={summaryOpen} onClick={onOpenSummary ?? onRightSidebarExpand}><ListCollapse size={16} strokeWidth={1.5} /></Button></TooltipTrigger><TooltipContent side="bottom">对话摘要</TooltipContent></Tooltip>
        <Tooltip><TooltipTrigger asChild><Button variant="ghost" size="icon" className="fox-topbar-toggle fox-right-sidebar-expand" aria-label={rightSidebarCollapsed ? "展开右侧边栏" : "收起右侧边栏"} aria-expanded={!rightSidebarCollapsed} aria-pressed={!rightSidebarCollapsed} onClick={() => rightSidebarCollapsed ? onRightSidebarExpand() : onRightSidebarCollapse()}><PanelRight size={16} strokeWidth={1.5} /></Button></TooltipTrigger><TooltipContent side="bottom">{rightSidebarCollapsed ? "展开右侧边栏" : "收起右侧边栏"}</TooltipContent></Tooltip>
      </div>
    </header>
  )
}
