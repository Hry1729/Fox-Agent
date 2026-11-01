import { Check, ChevronDown } from 'lucide-react'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { isAgentEnabled, useEnabledExternalAgents } from '@/features/conversations/model/external-agent-enablement'
import { SELECTABLE_AGENT_BACKEND_IDS, AGENT_BACKEND_LABELS, AGENT_BACKEND_PICKER_HINTS, canPickAgentBackend, isAgentBackendHidden, isAgentBackendRetired, type AgentBackendId, type AgentBackendStatus } from '@/features/conversations/model/agent-backend'

/**
 * The new conversation's Agent (plan C1): Fox (built-in PiNative), plus each
 * external Agent (Pi, Codex, DeepSeek / dsh) the user enabled in 设置 → Agent
 * (UI 1103: none by default). A backend the Host cannot start now (switched off by the
 * administrator) stays listed, greyed with the Host's reason. With a single
 * choice left (an older Host, every external Agent off) the label is shown
 * with a hint where to enable more.
 */
export function ComposerAgentPicker({ value, status, disabled, onSelect }: {
  value: AgentBackendId
  status: AgentBackendStatus | null
  disabled?: boolean
  onSelect: (backend: AgentBackendId) => void
}) {
  const enabled = useEnabledExternalAgents()
  const choices = SELECTABLE_AGENT_BACKEND_IDS.filter((backend) => isAgentEnabled(backend, enabled) && !isAgentBackendHidden(backend, status) && !isAgentBackendRetired(backend, status))
  return <DropdownMenu>
    <DropdownMenuTrigger asChild><button type="button" className="fox-composer-agent-trigger" data-agent-backend={value} disabled={disabled} aria-label={`选择 Agent，当前 ${AGENT_BACKEND_LABELS[value]}`}><span>{AGENT_BACKEND_LABELS[value]}</span><ChevronDown size={13} /></button></DropdownMenuTrigger>
    <DropdownMenuContent align="end" side="top" className="fox-composer-agent-menu">
      <DropdownMenuLabel>本次新对话</DropdownMenuLabel>
      {choices.map(backend => {
        const option = status?.options.find(item => item.backend === backend)
        const available = canPickAgentBackend(backend, status)
        const reason = available ? null : option?.reason ?? '正在读取可用状态…'
        return <DropdownMenuItem key={backend} disabled={!available} title={reason ?? undefined} onSelect={() => onSelect(backend)} data-agent-backend-option={backend}>
          <span className="fox-composer-agent-option"><span>{AGENT_BACKEND_LABELS[backend]}{option?.experimental && <small>实验</small>}</span><small>{reason ?? AGENT_BACKEND_PICKER_HINTS[backend]}</small></span>
          {value === backend && <Check size={14} aria-label="已选择" />}
        </DropdownMenuItem>
      })}
      {choices.length <= 1 && <DropdownMenuLabel className="fox-composer-agent-more" data-agent-picker-hint>在「设置 → Agent」里启用 Pi、Codex 或 dsh 后可以在这里选</DropdownMenuLabel>}
    </DropdownMenuContent>
  </DropdownMenu>
}
