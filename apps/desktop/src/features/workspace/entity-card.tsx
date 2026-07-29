import { Bot, CheckCircle2, ChevronRight, Database, Files, Plug, Wrench } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardFooter, CardHeader } from '@/components/ui/card'
import { Separator } from '@/components/ui/separator'
import type { AgentRecord, KnowledgeBaseRecord } from '@/features/conversations/model/types'

export interface AgentCardData extends AgentRecord { image: string; tools: number; knowledge: number; mcps: number; skills: number; recent: string; active?: boolean }
export interface KnowledgeCardData extends KnowledgeBaseRecord { types: string; progress: number; agents: number; recent: string; tone: string }

export function AgentCard({ agent, onOpen, onUse }: { agent: AgentCardData; onOpen: () => void; onUse: () => void }) {
  return (
    <Card className="fox-shadcn-kb-card fox-shadcn-agent-card" onClick={onOpen} role="button" tabIndex={0}>
      <CardHeader className="fox-shadcn-kb-head">
        <span className="fox-shadcn-kb-icon fox-shadcn-agent-icon"><img src={agent.image} alt="" /></span>
        <span className="fox-shadcn-kb-heading"><strong>{agent.name}</strong><small>{agent.description}</small></span>
        <ChevronRight className="fox-shadcn-kb-chevron" />
      </CardHeader>
      <CardContent className="fox-shadcn-kb-content">
        <div className="fox-shadcn-kb-types"><span>默认模型</span><b>{agent.defaultModel}</b></div>
        <div className="fox-shadcn-kb-stats">
          <span><Wrench /><b>{agent.tools}</b><small>Tools</small></span>
          <span><Database /><b>{agent.knowledge}</b><small>知识库</small></span>
          <span><Plug /><b>{agent.mcps}</b><small>MCP</small></span>
          <span><CheckCircle2 /><b>{agent.skills}</b><small>Skills</small></span>
        </div>
      </CardContent>
      <Separator />
      <CardFooter className="fox-shadcn-kb-footer">
        <Badge variant="secondary" className={`fox-agent-availability ${agent.available ? 'is-ready' : 'is-processing'}`}>{agent.active ? '正在使用' : agent.available ? '可使用' : '服务离线'}</Badge>
        <Badge variant="secondary" className={`fox-agent-source ${agent.runtimeType === 'pi' ? 'is-local' : 'is-knowledge'}`}>{agent.recent}</Badge>
        <Button size="sm" variant="outline" onClick={(event) => { event.stopPropagation(); onUse() }}>使用</Button>
      </CardFooter>
    </Card>
  )
}

export function KnowledgeCard({ knowledge, onOpen }: { knowledge: KnowledgeCardData; onOpen: () => void }) {
  const ready = knowledge.progress >= 100
  return (
    <Card className="fox-shadcn-kb-card" onClick={onOpen} role="button" tabIndex={0}>
      <CardHeader className="fox-shadcn-kb-head">
        <span className={`fox-shadcn-kb-icon is-${knowledge.tone}`}><Database /></span>
        <span className="fox-shadcn-kb-heading"><strong>{knowledge.name}</strong><small>{knowledge.description}</small></span>
        <ChevronRight className="fox-shadcn-kb-chevron" />
      </CardHeader>
      <CardContent className="fox-shadcn-kb-content">
        <div className="fox-shadcn-kb-types"><span>资料类型</span><b>{knowledge.types}</b></div>
        <div className="fox-shadcn-kb-stats">
          <span><Files /><b>{knowledge.fileCount}</b><small>文件</small></span>
          <span><CheckCircle2 /><b>{knowledge.progress}%</b><small>已处理</small></span>
          <span><Bot /><b>{knowledge.agents}</b><small>Agent</small></span>
        </div>
      </CardContent>
      <Separator />
      <CardFooter className="fox-shadcn-kb-footer">
        <Badge variant="secondary" className={ready ? 'is-ready' : 'is-processing'}><i />{ready ? '已就绪' : '处理中'}</Badge>
        <small>{knowledge.recent}</small>
        <Button size="sm" variant="outline" onClick={(event) => { event.stopPropagation(); onOpen() }}>浏览</Button>
      </CardFooter>
    </Card>
  )
}
