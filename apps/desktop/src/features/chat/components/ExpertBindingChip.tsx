import { CheckCircle2, Eye, PackageX, RefreshCw, Sparkles, Trash2, WifiOff } from 'lucide-react'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import type { AgentRecord, ConversationExpertBinding } from '@/features/conversations/model/types'
import { FOX_ASSISTANT_AVATAR } from './FoxAssistantAvatar'
import type { ExpertToolAvailability } from './expert-binding-ui'

export type ExpertBindingView = ConversationExpertBinding

function ExpertIcon({ icon, name }: { icon?: string | null; name: string }) {
  return <span className="fox-expert-icon" aria-hidden="true">
    {icon ? <img src={icon} alt="" /> : <Sparkles size={11} />}
    <span>{name.slice(0, 1)}</span>
  </span>
}

// Removal stays optional for callers that allow an unbound expert state.
export function ExpertBindingChip({ expert, readOnly, onView, onChange, onRemove, defaultLabel = false }: {
  expert: AgentRecord
  readOnly: boolean
  toolAvailability?: ExpertToolAvailability
  onView: () => void
  onChange: () => void
  onRemove?: () => void | Promise<void>
  defaultLabel?: boolean
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger className="fox-expert-chip" data-default-expert={defaultLabel || undefined} title={`当前专家：${expert.name}`}>
        <ExpertIcon icon={expert.icon || (defaultLabel ? FOX_ASSISTANT_AVATAR : undefined)} name={expert.name} />
        <span>{defaultLabel ? '默认专家' : expert.name}</span>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" side="top" sideOffset={7} className="fox-expert-menu">
        <DropdownMenuItem onSelect={onView}><Eye />查看详情</DropdownMenuItem>
        <DropdownMenuItem disabled={readOnly} onSelect={onChange}><RefreshCw />更换专家</DropdownMenuItem>
        {onRemove && <><DropdownMenuSeparator />
        <DropdownMenuItem className="fox-expert-remove" disabled={readOnly} onSelect={() => void onRemove()}><Trash2 />移除专家</DropdownMenuItem></>}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function capabilitySummary(expert?: AgentRecord) {
  if (!expert) return []
  return [
    expert.resources.skills.length ? `${expert.resources.skills.length} 技能` : '',
    expert.resources.knowledges.length ? `${expert.resources.knowledges.length} 知识` : '',
    expert.resources.mcps.length ? `${expert.resources.mcps.length} 连接器` : '',
    expert.resources.tools.length ? `${expert.resources.tools.length} 工具` : '',
  ].filter(Boolean).slice(0, 3)
}

function availability(expert?: AgentRecord) {
  if (!expert) return { label: '版本不可用', status: 'missing', icon: PackageX }
  if (!expert.available) return { label: '无法连接', status: 'offline', icon: WifiOff }
  const resources = expert.resources.skills.length + expert.resources.knowledges.length + expert.resources.mcps.length + expert.resources.tools.length
  if (resources === 0) return { label: '缺少资源', status: 'missing', icon: PackageX }
  return { label: '可用', status: 'available', icon: CheckCircle2 }
}

function activationTime(value: string | number) {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return ''
  return new Intl.DateTimeFormat('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' }).format(date)
}

export function ExpertActivationCard({ binding, expert, onView }: {
  binding: ExpertBindingView
  expert?: AgentRecord
  onView: (expertId: string) => void
}) {
  const snapshot = binding.displaySnapshot
  const state = availability(expert)
  const StatusIcon = state.icon
  const capabilities = capabilitySummary(expert)
  const historicalState = binding.state === 'replaced' ? '后续已更换' : binding.state === 'removed' ? '后续已移除' : ''
  const time = activationTime(binding.activatedAt)

  return (
    <aside className="fox-expert-activation" data-host-event="expert-enabled">
      <ExpertIcon icon={snapshot.icon ?? expert?.icon} name={snapshot.name} />
      <div className="fox-expert-activation-copy">
        <header>
          <strong>已启用 {snapshot.name}</strong>
          <span className={`fox-expert-availability is-${state.status}`}><StatusIcon size={11} />{state.label}</span>
        </header>
        <p>{snapshot.description || expert?.description || '此专家已作为当前对话的专家包启用。'}</p>
        <footer>
          {(snapshot.category || expert?.category) && <span>{snapshot.category || expert?.category}</span>}
          {capabilities.map((item) => <span key={item}>{item}</span>)}
          {historicalState && <span>{historicalState}</span>}
          {time && <time>{time}</time>}
        </footer>
      </div>
      <button type="button" onClick={() => onView(binding.expertId)}>查看详情</button>
    </aside>
  )
}
