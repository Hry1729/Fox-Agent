import { useCallback, useEffect, useRef, useState } from 'react'
import type { AgentNetworkClient, AgentNetworkStatus, NetworkTarget } from '@/features/conversations/model/agent-network'

const label = (target: NetworkTarget) => `${target.host}:${target.port}`

export function AgentNetworkPanel({ client, conversationId }: { client: AgentNetworkClient; conversationId: string }) {
  const [open, setOpen] = useState(false)
  const [status, setStatus] = useState<AgentNetworkStatus | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const busyRef = useRef(false)
  const refresh = useCallback(async () => {
    try { setStatus(await client.agentBackendNetworkStatus(conversationId)); setLoadError(null) }
    catch (cause) { setLoadError(cause instanceof Error ? cause.message : String(cause)) }
  }, [client, conversationId])
  useEffect(() => {
    if (!open) return
    void refresh()
    const timer = window.setInterval(() => { if (!busyRef.current) void refresh() }, 3000)
    return () => window.clearInterval(timer)
  }, [open, refresh])
  const run = async (action: () => Promise<unknown>) => {
    if (busyRef.current) return
    busyRef.current = true
    setError(null)
    setBusy(true)
    try { await action(); await refresh() }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { busyRef.current = false; setBusy(false) }
  }
  return <details className="fox-agent-network" open={open} onToggle={event => setOpen(event.currentTarget.open)}>
    <summary>网络与本地预览</summary>
    {open && <div className="fox-agent-network-content">
      <p>本地预览</p>
      {status && !status.previews.length && <p>未发现此会话的预览服务。启动后会自动出现在这里。</p>}
      {status?.previews.map(preview => <div key={`${preview.pid}:${preview.sessionKey}:${preview.port}`} className="fox-agent-network-row">
        <code>{preview.url}</code>
        <button type="button" disabled={busy} onClick={() => void run(() => client.agentBackendPreviewAction(conversationId, preview, false))}>打开预览</button>
        <button type="button" disabled={busy} onClick={() => void run(() => client.agentBackendPreviewAction(conversationId, preview, true))}>停止预览和当前 Agent</button>
      </div>)}
      <p>当前对话的网络授权</p>
      {status && !status.conversationRules.length && <p>遇到未授权的目标时，Fox 会显示网络审批。</p>}
      {status?.conversationRules.map(target => <div key={label(target)} className="fox-agent-network-row">
        <code>{label(target)}</code>
        {status.projectId && !status.projectRules.some(rule => label(rule) === label(target)) && <button type="button" disabled={busy} onClick={() => void run(() => client.agentBackendNetworkRule(conversationId, target, true, true))}>此项目长期允许</button>}
        <button type="button" disabled={busy} onClick={() => void run(() => client.agentBackendNetworkRule(conversationId, target, false, false))}>撤销并停止当前 Agent</button>
      </div>)}
      {!!status?.projectRules.length && <><p>此项目长期允许</p>{status.projectRules.map(target => <div key={label(target)} className="fox-agent-network-row">
        <code>{label(target)}</code>
        <button type="button" disabled={busy} onClick={() => void run(() => client.agentBackendNetworkRule(conversationId, target, true, false))}>移除项目规则</button>
      </div>)}<p>移除后，新连接按会话授权重新检查；已有连接在相应 Agent 停止后关闭。</p></>}
      <p>端口由 Host 核验，只自动放行此会话进程的本地服务。预览随该 Agent 结束而停止。</p>
      {(error || loadError) && <p role="alert">{error || loadError}</p>}
    </div>}
  </details>
}
