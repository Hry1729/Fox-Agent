import { useState, type ReactNode } from 'react'
import { RefreshCw, Search } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { AGENT_NAMES, STATE_LABELS } from './agent-config-copy'
import type { AgentConfigStatus, ExternalAgentId } from './agent-config-types'

export type ExternalAgentStatus = AgentConfigStatus & { agent: ExternalAgentId }

const agentPrograms = { pi: 'pi', codex: 'codex', deepseek: 'dsh' } as const

export function AgentMark({ agent, large = false }: { agent: ExternalAgentId; large?: boolean }) {
  return <span className={`fox-settings-provider-mark${large ? ' is-large' : ''}`} aria-hidden="true">
    {agent === 'pi' ? 'π' : <img src={`/providers/${agent === 'codex' ? 'openai' : 'deepseek'}.svg`} alt="" />}
  </span>
}

export function AgentEntry({ status, description, children }: { status: ExternalAgentStatus; description?: string; children?: ReactNode }) {
  const ready = status.state === 'ready' && status.enabled
  const label = status.install.blocker === 'system_only' ? '仅检测到系统安装'
    : status.install.blocker === 'runtime_incomplete' ? '运行时不完整'
    : status.state === 'ready' && !status.enabled ? '已停用' : STATE_LABELS[status.state]
  return <div className="fox-agent-entry">
    <AgentMark agent={status.agent} />
    <div className="fox-settings-provider-text">
      <span className="fox-settings-provider-name"><b>{AGENT_NAMES[status.agent]}</b></span>
      {description && <small>{description}</small>}
    </div>
    <div className="fox-agent-entry-actions">
      <span className="fox-agent-entry-status" data-ready={ready}>{label}</span>
      {children}
    </div>
  </div>
}

export function AgentCatalog({ agents, unavailable, loading, error, onDetect, onConfigure }: {
  agents: ExternalAgentStatus[]
  unavailable: boolean
  loading: boolean
  error: string
  onDetect: () => void
  onConfigure: (agent: ExternalAgentStatus) => void
}) {
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState(unavailable ? 'unavailable' : 'all')
  const term = query.trim().toLowerCase()
  const matches = agents.filter(({ agent, install }) =>
    `${AGENT_NAMES[agent]} ${agentPrograms[agent]}`.toLowerCase().includes(term)
    && (filter === 'all' || install.launchable === (filter === 'available')))

  return <>
    <div className="fox-provider-catalog-toolbar fox-agent-catalog-toolbar">
      <div className="fox-provider-search">
        <Search aria-hidden="true" />
        <Input type="search" aria-label="搜索 Agent" placeholder="搜索名称或命令" value={query} onChange={event => setQuery(event.target.value)} />
      </div>
      <div className="fox-agent-catalog-filters">
        <select aria-label="按运行时状态筛选" value={filter} onChange={event => setFilter(event.target.value)}>
          <option value="all">全部</option><option value="available">运行时可用</option><option value="unavailable">运行时不可用</option>
        </select>
        <Button variant="ghost" onClick={onDetect} disabled={loading}><RefreshCw className={loading ? 'animate-spin' : undefined} />{loading ? '检测中…' : '重新检测'}</Button>
      </div>
    </div>
    {error && <p role="alert" className="fox-settings-sheet-error">{error}</p>}
    <div className="fox-agent-directory" aria-busy={loading}>
      {matches.map(status => <AgentEntry key={status.agent} status={status} description={`检测命令 · ${agentPrograms[status.agent]}`}>
        {status.install.launchable && status.state !== 'read_failed' && <Button variant="outline" size="sm" aria-label={`配置 ${AGENT_NAMES[status.agent]}`} onClick={() => onConfigure(status)}>配置</Button>}
      </AgentEntry>)}
      {!matches.length && !loading && !error && <p role="status" className="fox-settings-sheet-hint">没有符合条件的 Agent。</p>}
    </div>
    {matches.some(status => !status.install.launchable) && <p className="fox-settings-sheet-hint">外部 Agent 需要 Fox 自带的运行时。更新或重新安装 Fox 后，可在这里重新检测。</p>}
    {matches.some(status => status.state === 'read_failed') && <p className="fox-settings-sheet-hint">读取配置失败。请重新检测。</p>}
  </>
}
