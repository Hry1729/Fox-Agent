import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { RuntimeApprovalPrompt } from '@/features/chat/components/RuntimeApprovalPrompt'
import { useDeveloperUi } from '@/features/chat/developer-ui'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { setExternalAgentEnabled } from '@/features/conversations/model/external-agent-enablement'
import type { ApprovalRecord } from '@/features/conversations/model/types'
import { SettingsGroup } from '../settings-layout'
import { useModelProviders } from '../use-model-providers'
import { AgentBindingForm } from './agent-binding-form'
import { AGENT_NAMES, STATE_LABELS, configErrorText } from './agent-config-copy'
import type { AgentConfigOverview, AgentConfigStatus, AgentId, VerificationReport, VerifyDepth } from './agent-config-types'
import { FullVerificationControl } from './full-verification-control'
import { VerificationLadder } from './verification-ladder'

/** External Agent bindings and verification, kept separate from model services. */
export function AgentServiceSettings({ onOpenModelSettings }: { onOpenModelSettings: () => void }) {
  const providers = useModelProviders()
  const developer = useDeveloperUi()
  const [overview, setOverview] = useState<AgentConfigOverview | null>(null)
  const [error, setError] = useState('')
  const [binding, setBinding] = useState<{ agent: AgentId; native: boolean } | null>(null)
  const [running, setRunning] = useState<AgentId | null>(null)
  const [reports, setReports] = useState<Partial<Record<AgentId, VerificationReport>>>({})
  const [approval, setApproval] = useState<ApprovalRecord | null>(null)
  const active = useRef<AgentId | null>(null)
  const [showReport, setShowReport] = useState<AgentId | null>(null)
  const [enableBusy, setEnableBusy] = useState<AgentId | null>(null)

  const refresh = useCallback(async () => {
    try { setOverview(await desktopClient.agentConfigOverview(developer)); setError('') }
    catch (cause) { setError(configErrorText(cause)) }
  }, [developer])
  useEffect(() => {
    void refresh()
    const update = () => { void refresh() }
    window.addEventListener('focus', update)
    window.addEventListener('fox:model-service-changed', update)
    return () => { window.removeEventListener('focus', update); window.removeEventListener('fox:model-service-changed', update) }
  }, [refresh])
  useEffect(() => () => {
    if (active.current) void desktopClient.agentConfigVerifyCancel(active.current).catch(() => {})
  }, [])
  useEffect(() => {
    if (!running) return
    let disposed = false
    let unlisten: (() => void) | undefined
    void listen<{ backend: AgentId; report: VerificationReport }>('agent-config-verification', event => {
      if (event.payload.backend === running) setReports(previous => ({ ...previous, [running]: event.payload.report }))
    }).then(stop => { if (disposed) stop(); else unlisten = stop }).catch(() => {})
    return () => { disposed = true; unlisten?.() }
  }, [running])

  const verify = async (agent: AgentId, depth: VerifyDepth = 'model', selection?: { permissionMode: string; confirmFullAccess: boolean }) => {
    if (active.current) return
    active.current = agent
    setApproval(null)
    setRunning(agent); setShowReport(agent); setReports(previous => { const next = { ...previous }; delete next[agent]; return next }); setError('')
    try {
      const report = await desktopClient.agentConfigVerify(agent, depth, developer, selection && { ...selection, onApproval: setApproval })
      setReports(previous => ({ ...previous, [agent]: report }))
    } catch (cause) { setError(configErrorText(cause)) }
    finally { active.current = null; setApproval(null); setRunning(null); await refresh() }
  }
  const statusText = (agent: AgentConfigStatus) => {
    if (agent.state === 'read_failed') return '没能读取配置，请重试。'
    if (agent.state === 'not_installed') return agent.install.blocker === 'system_only' ? '电脑上装了这个 Agent，但 Fox 需要自带运行时。请修复 Fox。' : agent.install.detected ? '这个 Agent 的运行时不完整，请修复 Fox 外部运行时。' : 'Fox 运行时里没有这个 Agent。'
    if (agent.binding?.source === 'native') return agent.native.changed ? '本机配置已变化，请重新导入。' : '使用从这台电脑导入的配置'
    const bound = agent.binding
    if (bound?.source === 'fox_service') return `${providers.items.find(provider => provider.id === bound.providerId)?.name ?? '服务已删除'} · ${bound.modelId}`
    if (agent.state === 'native_importable') return `在这台电脑上找到了 ${AGENT_NAMES[agent.agent]} 的登录配置。`
    return agent.candidates.length ? '选择一个模型服务' : `请添加支持 ${agent.agent === 'codex' ? 'Responses' : agent.agent === 'deepseek' ? 'Messages' : '匹配协议'} 的服务`
  }
  const agents = (overview?.agents ?? []).filter((agent): agent is AgentConfigStatus & { agent: Exclude<AgentId, 'fox'> } => agent.agent !== 'fox')

  return <SettingsGroup title="外部 Agent" aside="启用后可在新对话中选择">
    {error && <p role="alert" className="fox-model-error">{error}<Button variant="ghost" size="sm" onClick={() => void refresh()}>重试</Button></p>}
    {!overview && !error && <p className="fox-model-section" role="status">正在读取 Agent 配置…</p>}
    {agents.map(agent => <div className="fox-agent-config-row" key={agent.agent}>
      <div className="fox-agent-config-main"><div><b>{AGENT_NAMES[agent.agent]}</b><small data-ready={agent.state === 'ready'}>{STATE_LABELS[agent.state]}</small></div><div className="fox-agent-description"><span>{statusText(agent)}</span>{agent.enabled && !['ready', 'not_installed', 'read_failed'].includes(agent.state) && <small>启用后需要先配置并验证，才能在对话里使用。</small>}{developer && agent.diagnostic && <code>{agent.diagnostic.code}{agent.diagnostic.detail && ` · ${agent.diagnostic.detail}`}</code>}</div><div className="fox-model-actions">
        {agent.state === 'read_failed' ? <Button variant="outline" size="sm" onClick={() => void refresh()}>重试</Button> : agent.state !== 'not_installed' && <>
          {agent.binding && <Button variant="outline" size="sm" disabled={running !== null} onClick={() => void verify(agent.agent)}>验证</Button>}
          {agent.state === 'native_importable' && <Button variant="outline" size="sm" onClick={() => setBinding({ agent: agent.agent, native: true })}>导入…</Button>}
          <Button variant="ghost" size="sm" disabled={running === agent.agent} onClick={() => { if (!agent.candidates.length && !agent.native.available) onOpenModelSettings(); else setBinding({ agent: agent.agent, native: false }) }}>{agent.binding ? '更改' : agent.candidates.length || agent.native.available ? '配置' : '添加模型'}</Button>
        </>}
        <Switch aria-label={`启用 ${AGENT_NAMES[agent.agent]}`} checked={agent.enabled} disabled={agent.state === 'not_installed' || agent.state === 'read_failed' || enableBusy === agent.agent} onCheckedChange={value => { setEnableBusy(agent.agent); void setExternalAgentEnabled(agent.agent, value).then(refresh).catch(cause => setError(configErrorText(cause))).finally(() => setEnableBusy(null)) }} />
      </div></div>
      {binding?.agent === agent.agent && <AgentBindingForm status={agent} providers={providers.items} developer={developer} initialNative={binding.native} onCancel={() => setBinding(null)} onDone={shouldVerify => { setBinding(null); void refresh(); if (shouldVerify) void verify(agent.agent) }} />}
      {(showReport === agent.agent || agent.verification) && <>
        <VerificationLadder report={running === agent.agent ? reports[agent.agent] ?? null : reports[agent.agent] ?? agent.verification} running={running === agent.agent} developer={developer} onStop={() => { void desktopClient.agentConfigVerifyCancel(agent.agent).catch(cause => setError(configErrorText(cause))) }} />
        {running === agent.agent && approval && <RuntimeApprovalPrompt key={approval.id} approval={approval} onResolve={async (id, decision) => {
          const answered = await desktopClient.agentConfigVerifyAnswer(agent.agent, id, decision === 'allow_once')
          if (answered) setApproval(current => current?.id === id ? null : current)
          return answered
        }} />}
        <FullVerificationControl backend={agent.agent} busy={running !== null} onStart={(permissionMode, confirmFullAccess) => verify(agent.agent, 'tool', { permissionMode, confirmFullAccess })} />
      </>}
    </div>)}
  </SettingsGroup>
}
