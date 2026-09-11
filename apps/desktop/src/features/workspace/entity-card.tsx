import type { KeyboardEvent } from 'react'
import { Database, Settings2 } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader } from '@/components/ui/card'
import type { AgentRecord, KnowledgeBaseRecord } from '@/features/conversations/model/types'

export interface AgentCardData extends AgentRecord { image: string; tools: number; knowledge: number; mcps: number; skills: number; recent: string; active?: boolean }
export interface KnowledgeCardData extends KnowledgeBaseRecord { types: string; progress: number; agents: number; recent: string; tone: string }

function activateCard(event: KeyboardEvent, action: () => void) {
  if (event.target !== event.currentTarget) return
  if (event.key !== 'Enter' && event.key !== ' ') return
  event.preventDefault()
  action()
}

export function AgentCard({ agent, onUse, onManage, disabled = false, showAction = true }: { agent: AgentCardData; onUse: () => void; onManage?: () => void; disabled?: boolean; showAction?: boolean }) {
  const summon = () => {
    if (disabled) return
    if (!agent.available) {
      toast.error('该专家暂不可用', { description: agent.runtimeType === 'yuxi' ? '请确认知识库服务已连接后再试' : '请稍后重试' })
      return
    }
    onUse()
  }
  return (
    <Card className={`fox-library-card fox-library-card--stacked-meta fox-shadcn-kb-card fox-shadcn-agent-card ${agent.available ? '' : 'is-unavailable'}`} onClick={summon} onKeyDown={(event) => activateCard(event, summon)} role="button" tabIndex={0} aria-label={`召唤${agent.name}`} aria-disabled={disabled || !agent.available}>
      <CardHeader className="fox-shadcn-kb-head">
        <span className="fox-shadcn-kb-icon fox-shadcn-agent-icon"><img src={agent.image} alt="" /></span>
        <span className="fox-shadcn-kb-heading">
          <span className="fox-agent-name-row"><strong>{agent.name}</strong></span>
        </span>
      </CardHeader>
      {showAction && <Button className="fox-agent-summon" size="sm" disabled={disabled || !agent.available} onClick={(event) => { event.stopPropagation(); summon() }}>召唤</Button>}
      {onManage && <Button className="fox-agent-manage" variant="ghost" size="icon" aria-label={`管理${agent.name}`} onClick={(event) => { event.stopPropagation(); onManage() }}><Settings2 /></Button>}
      <CardContent className="fox-shadcn-kb-content">
        <p className="fox-agent-description" title={agent.description}>{agent.description}</p>
        <span className="fox-library-card-tags">
          {agent.tools > 0 && <small>工具 {agent.tools}</small>}
          {agent.knowledge > 0 && <small>知识库 {agent.knowledge}</small>}
          {agent.mcps > 0 && <small>连接器 {agent.mcps}</small>}
          {agent.skills > 0 && <small>技能 {agent.skills}</small>}
          {agent.tools + agent.knowledge + agent.mcps + agent.skills === 0 && <small>通用专家</small>}
        </span>
      </CardContent>
    </Card>
  )
}

export function KnowledgeCard({ knowledge, onOpen }: { knowledge: KnowledgeCardData; onOpen: () => void }) {
  const ready = knowledge.progress >= 100
  return (
    <Card className="fox-library-card fox-library-card--stacked-meta fox-shadcn-kb-card fox-shadcn-agent-card fox-shadcn-knowledge-card" onClick={onOpen} onKeyDown={(event) => activateCard(event, onOpen)} role="button" tabIndex={0} aria-label={`打开知识库${knowledge.name}`}>
      <CardHeader className="fox-shadcn-kb-head">
        <span className={`fox-shadcn-kb-icon fox-shadcn-agent-icon is-${knowledge.tone}`}><Database /></span>
        <span className="fox-shadcn-kb-heading">
          <span className="fox-agent-name-row"><strong>{knowledge.name}</strong></span>
        </span>
      </CardHeader>
      <CardContent className="fox-shadcn-kb-content">
        <p className="fox-agent-description" title={knowledge.description}>{knowledge.description}</p>
        <span className="fox-library-card-tags"><small>知识库</small><small>{knowledge.fileCount} 文件</small><small>{ready ? '已就绪' : `${knowledge.progress}%`}</small>{knowledge.agents > 0 && <small>{knowledge.agents} 专家</small>}</span>
      </CardContent>
    </Card>
  )
}
