import { Check, ChevronDown } from 'lucide-react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { SELECTABLE_AGENT_BACKEND_IDS, AGENT_BACKEND_LABELS, canPickAgentBackend, isAgentBackendHidden, isAgentBackendRetired, type AgentBackendId, type AgentBackendStatus } from '@/features/conversations/model/agent-backend'

/** A single supported driver keeps its label without offering a menu. */
export function ComposerAgentPicker({ value, status, disabled, onSelect }: {
  value: AgentBackendId
  status: AgentBackendStatus | null
  disabled?: boolean
  onSelect: (backend: AgentBackendId) => void
}) {
  const choices = SELECTABLE_AGENT_BACKEND_IDS.filter((backend) => !isAgentBackendHidden(backend, status) && !isAgentBackendRetired(backend, status))
  if (choices.length <= 1) {
    const label = AGENT_BACKEND_LABELS[value] ?? AGENT_BACKEND_LABELS[SELECTABLE_AGENT_BACKEND_IDS[0]]
    return (
      <span className="fox-composer-agent-trigger fox-composer-agent-static" data-agent-backend={value} title="本版本只有一种受支持的 Agent 驱动" aria-label={`当前 Agent：${label}`}>
        <span>{label}</span>
      </span>
    )
  }
  return <DropdownMenu>
    <DropdownMenuTrigger asChild><button type="button" className="fox-composer-agent-trigger" disabled={disabled} aria-label={`选择 Agent，当前 ${AGENT_BACKEND_LABELS[value]}`}><span>{AGENT_BACKEND_LABELS[value]}</span><ChevronDown size={13} /></button></DropdownMenuTrigger>
    <DropdownMenuContent align="end" side="bottom" className="fox-composer-agent-menu">
      <DropdownMenuLabel>本次新对话</DropdownMenuLabel>
      {choices.map(backend => {
        const option = status?.options.find(item => item.backend === backend)
        const available = canPickAgentBackend(backend, status)
        const reason = available ? null : option?.reason ?? '正在读取可用状态…'
        return <DropdownMenuItem key={backend} disabled={!available} title={reason ?? undefined} onSelect={() => onSelect(backend)}>
          <span className="fox-composer-agent-option"><span>{AGENT_BACKEND_LABELS[backend]}{option?.experimental && <small>实验</small>}</span>{reason && <small>{reason}</small>}</span>
          {value === backend && <Check size={14} aria-label="已选择" />}
        </DropdownMenuItem>
      })}
    </DropdownMenuContent>
  </DropdownMenu>
}
