import { Check, ChevronDown } from 'lucide-react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { SELECTABLE_AGENT_BACKEND_IDS, AGENT_BACKEND_LABELS, canPickAgentBackend, isAgentBackendHidden, isAgentBackendRetired, type AgentBackendId, type AgentBackendStatus } from '@/features/conversations/model/agent-backend'

/**
 * The new-conversation Agent picker. Since the single Pi mainline there is
 * exactly one supported driver, and a select with one option is not a choice:
 * nothing is rendered then (the composer shows no picker at all). The rest of
 * the component stays for the records and tests of the multi-driver era, and
 * for a Host that ever supports more than one backend again.
 */
export function ComposerAgentPicker({ value, status, disabled, onSelect }: {
  value: AgentBackendId
  status: AgentBackendStatus | null
  disabled?: boolean
  onSelect: (backend: AgentBackendId) => void
}) {
  const choices = SELECTABLE_AGENT_BACKEND_IDS.filter((backend) => !isAgentBackendHidden(backend, status) && !isAgentBackendRetired(backend, status))
  if (choices.length <= 1) return null
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
