import type { KeyboardEvent } from 'react'
import { Bot, CheckCircle2, Database, Files, Plug, Settings2, Wrench } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader } from '@/components/ui/card'
import type { AgentRecord, KnowledgeBaseRecord } from '@/features/conversations/model/types'

export interface AgentCardData extends AgentRecord { image: string; tools: number; knowledge: number; mcps: number; skills: number; recent: string; active?: boolean }
export interface KnowledgeCardData extends KnowledgeBaseRecord { types: string; progress: number; agents: number; recent: string; tone: string }

function activateCard(event: KeyboardEvent, action: () => void) {
  if (event.key !== 'Enter' && event.key !== ' ') return
  event.preventDefault()
  action()
}

export function AgentCard({ agent, onUse, onManage }: { agent: AgentCardData; onUse: () => void; onManage?: () => void }) {
  const summon = () => {
    if (!agent.available) {
      toast.error('该专家暂不可用', { description: agent.runtimeType === 'yuxi' ? '请确认知识库服务已连接后再试' : '请稍后重试' })
      return
    }
    onUse()
  }
  const isLocal = agent.runtimeType === 'pi'
  return (
    <Card className={`fox-shadcn-kb-card fox-shadcn-agent-card ${agent.available ? '' : 'is-unavailable'}`} onClick={summon} onKeyDown={(event) => activateCard(event, summon)} role="button" tabIndex={0} aria-label={`召唤${agent.name}`} aria-disabled={!agent.available}>
      <CardHeader className="fox-shadcn-kb-head">
        <span className="fox-shadcn-kb-icon fox-shadcn-agent-icon"><img src={agent.image} alt="" /></span>
        <span className="fox-shadcn-kb-heading">
          <span className="fox-agent-name-row"><strong>{agent.name}</strong><Badge className={`fox-agent-source ${isLocal ? 'is-local' : 'is-knowledge'}`} variant="secondary">{isLocal ? '本机' : '知识库'}</Badge></span>
          <small className="fox-agent-subtitle">{isLocal ? 'Fox' : 'Yuxi'}</small>
        </span>
      </CardHeader>
      <Button className="fox-agent-summon" size="sm" disabled={!agent.available} onClick={(event) => { event.stopPropagation(); summon() }}>召唤</Button>
      {onManage && <Button className="fox-agent-manage" variant="ghost" size="icon" aria-label={`管理${agent.name}`} onClick={(event) => { event.stopPropagation(); onManage() }}><Settings2 /></Button>}
      <CardContent className="fox-shadcn-kb-content">
        <p className="fox-agent-description" title={agent.description}>{agent.description}</p>
        <div className="fox-shadcn-kb-stats">
          <span><Wrench /><b>{agent.tools}</b><small>Tools</small></span>
          <span><Database /><b>{agent.knowledge}</b><small>知识库</small></span>
          <span><Plug /><b>{agent.mcps}</b><small>MCP</small></span>
          <span><CheckCircle2 /><b>{agent.skills}</b><small>Skills</small></span>
        </div>
      </CardContent>
    </Card>
  )
}

export function KnowledgeCard({ knowledge, onOpen }: { knowledge: KnowledgeCardData; onOpen: () => void }) {
  const ready = knowledge.progress >= 100
  return (
    <Card className="fox-shadcn-kb-card fox-shadcn-knowledge-card" onClick={onOpen} onKeyDown={(event) => activateCard(event, onOpen)} role="button" tabIndex={0} aria-label={`打开知识库${knowledge.name}`}>
      <CardHeader className="fox-shadcn-kb-head">
        <span className={`fox-shadcn-kb-icon is-${knowledge.tone}`}><Database /></span>
        <span className="fox-shadcn-kb-heading">
          <span className="fox-agent-name-row"><strong>{knowledge.name}</strong><Badge className="fox-agent-source is-knowledge" variant="secondary">知识库</Badge></span>
          <small className="fox-agent-subtitle">{ready ? '已就绪' : '处理中'}</small>
        </span>
      </CardHeader>
      <CardContent className="fox-shadcn-kb-content">
        <p className="fox-agent-description" title={knowledge.description}>{knowledge.description}</p>
        <div className="fox-shadcn-kb-stats">
          <span><Files /><b>{knowledge.fileCount}</b><small>文件</small></span>
          <span><CheckCircle2 /><b>{knowledge.progress}%</b><small>已处理</small></span>
          <span><Bot /><b>{knowledge.agents}</b><small>专家</small></span>
        </div>
      </CardContent>
    </Card>
  )
}
