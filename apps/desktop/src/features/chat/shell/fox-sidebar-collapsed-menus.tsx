import { sidebarDateGroups } from '../sidebar-date-groups'
import { useEffect, useRef, useState } from 'react'
import { FolderOpen, MessageCircleMore } from 'lucide-react'
import { AnimatedIconTrigger, Folders, MessageCircleMore as AnimatedMessageCircleMore } from '@/components/fox-ui/animated-icons'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuPortal, DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import type { ConversationSummary } from '@/features/conversations/model/types'
import type { SidebarProjectGroup } from '../sidebar-project-groups'
import type { ConversationRunIndicator } from '../conversation-run-indicator'
import { ConversationRunStatus, CONVERSATION_RUN_LABELS } from '../conversation-run-status'

type Item = ConversationSummary & { active?: boolean; time?: string }

export function SidebarCollapsedMenus({ projects, conversations, indicators, onOpen }: {
  projects: SidebarProjectGroup<Item>[]
  conversations: Item[]
  indicators?: ReadonlyMap<string, ConversationRunIndicator>
  onOpen: (id: string) => void
}) {
  const [open, setOpen] = useState<'projects' | 'conversations' | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const hovered = useRef(false)
  const cancelClose = () => clearTimeout(timer.current)
  const closeLater = () => { cancelClose(); timer.current = setTimeout(() => setOpen(null), 180) }
  useEffect(() => () => clearTimeout(timer.current), [])
  const rows = (items: Item[]) => items.map(item => {
    const indicator = indicators?.get(item.id)
    return <DropdownMenuItem key={item.id} className={item.active ? 'is-active' : undefined} onSelect={() => onOpen(item.id)} title={indicator ? `${item.title} · ${CONVERSATION_RUN_LABELS[indicator]}` : item.title}>
      <ConversationRunStatus indicator={indicator} className="fox-sidebar-collapsed-run" /><span className="fox-sidebar-flyout-name">{item.title}</span>
      {item.time && <time>{item.time}</time>}
    </DropdownMenuItem>
  })
  return <div className="fox-sidebar-collapsed-sections" aria-label="项目与对话">
    {(['projects', 'conversations'] as const).map(section => {
      const projectMenu = section === 'projects'
      const label = projectMenu ? '项目' : '对话'
      const count = projectMenu ? projects.length : conversations.length
      return <DropdownMenu key={section} modal={false} open={open === section} onOpenChange={next => { cancelClose(); setOpen(next ? section : null) }}>
        <div className="fox-sidebar-collapsed-entry" onMouseEnter={() => { cancelClose(); hovered.current = true; setOpen(section) }} onMouseLeave={closeLater}>
          <AnimatedIconTrigger><DropdownMenuTrigger asChild><button type="button" aria-label={`展开${label}列表，共 ${count} 个${label}`} title={`${label} · ${count}`} className={open === section ? 'is-active' : undefined} onKeyDown={() => { hovered.current = false }}>
            {projectMenu ? <Folders /> : <AnimatedMessageCircleMore />}
          </button></DropdownMenuTrigger></AnimatedIconTrigger>
        </div>
        <DropdownMenuContent className="fox-sidebar-flyout" side="right" align="start" sideOffset={8} collisionPadding={8} onMouseEnter={cancelClose} onMouseLeave={closeLater}
          onCloseAutoFocus={event => { if (hovered.current) event.preventDefault() }} aria-label={`${label}列表`}>
          <DropdownMenuLabel>{label}</DropdownMenuLabel>
          {projectMenu ? projects.map(project => <DropdownMenuSub key={project.key}>
            <DropdownMenuSubTrigger title={project.root}><FolderOpen /><span className="fox-sidebar-flyout-name">{project.name}</span><small>{project.items.length}</small></DropdownMenuSubTrigger>
            <DropdownMenuPortal><DropdownMenuSubContent className="fox-sidebar-flyout" sideOffset={4} collisionPadding={8} onMouseEnter={cancelClose} onMouseLeave={closeLater} aria-label={`${project.name} 对话`}>
              <DropdownMenuLabel>{project.name}</DropdownMenuLabel>{rows(project.items)}
            </DropdownMenuSubContent></DropdownMenuPortal>
          </DropdownMenuSub>) : sidebarDateGroups(conversations).map(group => <div key={group.key}><DropdownMenuLabel className="fox-sidebar-date-heading"><MessageCircleMore size={14} />{group.label}</DropdownMenuLabel>{rows(group.items)}</div>)}
          {!count && <DropdownMenuItem disabled>{projectMenu ? '暂无项目' : '暂无对话'}</DropdownMenuItem>}
        </DropdownMenuContent>
      </DropdownMenu>
    })}
  </div>
}
