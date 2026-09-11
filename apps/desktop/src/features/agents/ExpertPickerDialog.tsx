import { useEffect, useRef, useState } from 'react'
import { Search } from 'lucide-react'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import type { AgentRecord } from '@/features/conversations/model/types'
import { AgentCard } from '@/features/workspace/entity-card'
import { expertCatalogAgents } from './agent-classification'
import { asAgentCard } from './agent-card-data'

export function ExpertPickerDialog({ open, onOpenChange, agents, selectedExpertId, readOnly, onSelect }: {
  open: boolean
  onOpenChange: (open: boolean) => void
  agents: AgentRecord[]
  selectedExpertId?: string | null
  readOnly: boolean
  onSelect: (expertId: string) => Promise<boolean>
}) {
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const busyRef = useRef(false)
  useEffect(() => { if (open) { setQuery(''); setError(null) } }, [open])
  const choices = expertCatalogAgents(agents).filter((agent) => `${agent.name} ${agent.description}`.toLocaleLowerCase().includes(query.trim().toLocaleLowerCase()))
  const select = async (expertId: string) => {
    if (busyRef.current || readOnly) return
    busyRef.current = true
    setBusy(true)
    setError(null)
    try { if (await onSelect(expertId)) onOpenChange(false) }
    catch { setError('专家暂时无法启用，请重试。') }
    finally { busyRef.current = false; setBusy(false) }
  }
  return <Dialog open={open} onOpenChange={(next) => { if (!busy) onOpenChange(next) }}>
    <DialogContent className="fox-expert-picker-dialog">
      <DialogHeader><DialogTitle>添加专家</DialogTitle><DialogDescription>{readOnly ? '任务结束后可更换专家。' : '选择专家参与当前对话。'}</DialogDescription></DialogHeader>
      <label className="fox-expert-picker-search"><Search size={16} /><Input aria-label="搜索专家" placeholder="搜索专家" value={query} onChange={(event) => setQuery(event.target.value)} /></label>
      {error && <p role="alert" className="text-sm text-destructive">{error}</p>}
      <div className="fox-expert-picker-grid" aria-busy={busy}>
        {choices.map((agent) => <div key={agent.id} className={agent.id === selectedExpertId ? 'fox-expert-picker-choice is-selected' : 'fox-expert-picker-choice'}>
          <AgentCard agent={asAgentCard(agent)} disabled={busy || readOnly} showAction={false} onUse={() => void select(agent.id)} />
          {agent.id === selectedExpertId && <span className="fox-expert-picker-selected">当前专家</span>}
        </div>)}
        {!choices.length && <p className="fox-expert-picker-empty">{query ? '没有找到匹配的专家' : '暂时没有可选择的专家'}</p>}
      </div>
    </DialogContent>
  </Dialog>
}
