import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { Bot, ChevronRight, Plus } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Switch } from '@/components/ui/switch'
import { RuntimeApprovalPrompt } from '@/features/chat/components/RuntimeApprovalPrompt'
import { useDeveloperUi } from '@/features/chat/developer-ui'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { setExternalAgentEnabled } from '@/features/conversations/model/external-agent-enablement'
import type { ApprovalRecord } from '@/features/conversations/model/types'
import { SettingsGroup, SettingsRow } from '../settings-layout'
import { useModelProviders } from '../use-model-providers'
import { AgentBindingForm } from './agent-binding-form'
import { AgentCatalog, AgentEntry, AgentMark, type ExternalAgentStatus } from './agent-catalog'
import { AGENT_NAMES, configErrorText } from './agent-config-copy'
import type { AgentConfigOverview, ExternalAgentId, VerificationReport, VerifyDepth } from './agent-config-types'
import { FullVerificationControl } from './full-verification-control'
import { VerificationLadder } from './verification-ladder'
import './model-agent.css'

type AgentPanel = { kind: 'catalog'; unavailable: boolean } | { kind: 'configure'; agent: ExternalAgentId }

/** External Agent bindings and verification, kept separate from model services. */
export function AgentServiceSettings({ onOpenModelSettings }: { onOpenModelSettings: () => void }) {
  const providers = useModelProviders()
  const developer = useDeveloperUi()
  const [overview, setOverview] = useState<AgentConfigOverview | null>(null)
  const [error, setError] = useState('')
  const [panel, setPanel] = useState<AgentPanel | null>(null)
  const [detecting, setDetecting] = useState(false)
  const [running, setRunning] = useState<ExternalAgentId | null>(null)
  const [reports, setReports] = useState<Partial<Record<ExternalAgentId, VerificationReport>>>({})
  const [approval, setApproval] = useState<ApprovalRecord | null>(null)
  const active = useRef<ExternalAgentId | null>(null)
  const panelTrigger = useRef<HTMLButtonElement | null>(null)
  const [agentError, setAgentError] = useState<{ agent: ExternalAgentId; message: string } | null>(null)
  const [enableBusy, setEnableBusy] = useState<ExternalAgentId | null>(null)

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
    void listen<{ backend: ExternalAgentId; report: VerificationReport }>('agent-config-verification', event => {
      if (event.payload.backend === running) setReports(previous => ({ ...previous, [running]: event.payload.report }))
    }).then(stop => { if (disposed) stop(); else unlisten = stop }).catch(() => {})
    return () => { disposed = true; unlisten?.() }
  }, [running])

  const verify = async (agent: ExternalAgentId, depth: VerifyDepth = 'model', selection?: { permissionMode: string; confirmFullAccess: boolean }) => {
    if (active.current) return
    active.current = agent
    setApproval(null)
    setRunning(agent)
    setReports(previous => ({ ...previous, [agent]: undefined }))
    setAgentError(null)
    try {
      const report = await desktopClient.agentConfigVerify(agent, depth, developer, selection && { ...selection, onApproval: setApproval })
      setReports(previous => ({ ...previous, [agent]: report }))
    } catch (cause) { setAgentError({ agent, message: configErrorText(cause) }) }
    finally { active.current = null; setApproval(null); setRunning(null); await refresh() }
  }
  const statusText = (agent: ExternalAgentStatus) => {
    if (agent.state === 'read_failed' || agent.state === 'not_installed') return undefined
    if (agent.binding?.source === 'native') return agent.native.changed ? '本机配置已变化，请重新导入。' : '使用从这台电脑导入的配置'
    const bound = agent.binding
    if (bound?.source === 'fox_service') return `${providers.items.find(provider => provider.id === bound.providerId)?.name ?? '服务已删除'} · ${bound.modelId}`
    if (agent.state === 'native_importable') return `在这台电脑上找到了 ${AGENT_NAMES[agent.agent]} 的登录配置。`
    return agent.candidates.length ? '选择一个模型服务' : `请添加支持 ${agent.agent === 'codex' ? 'Responses' : agent.agent === 'deepseek' ? 'Messages' : '匹配协议'} 的服务`
  }
  const agents = (overview?.agents ?? []).filter((agent): agent is ExternalAgentStatus => agent.agent !== 'fox')
  const configured = panel?.kind === 'configure' ? agents.find(agent => agent.agent === panel.agent) : undefined
  const report = configured ? reports[configured.agent] ?? (running === configured.agent ? null : configured.verification) : null
  const failedLayer = report?.layers.find(layer => layer.status === 'failed')
  const operationError = configured && agentError?.agent === configured.agent ? agentError.message : undefined
  const detect = () => {
    setDetecting(true)
    void refresh().finally(() => setDetecting(false))
  }
  const openPanel = (next: AgentPanel, trigger: HTMLButtonElement) => {
    panelTrigger.current = trigger
    setPanel(next)
  }
  const closePanel = () => setPanel(null)

  return <>
    <SettingsGroup title="外部 Agent" aside={<Button variant="outline" size="sm" aria-haspopup="dialog" onClick={event => openPanel({ kind: 'catalog', unavailable: false }, event.currentTarget)}><Plus />添加 Agent</Button>}>
      {error && <p role="alert" className="fox-model-error">{error}<Button variant="ghost" size="sm" onClick={detect} disabled={detecting}>重试</Button></p>}
      {!overview && !error && <p className="fox-model-section" role="status">正在读取 Agent 配置…</p>}
      {agents.map(agent => <AgentEntry key={agent.agent} status={agent} description={statusText(agent)}>
        {agent.state === 'read_failed' ? <Button variant="ghost" size="sm" onClick={detect} disabled={detecting}>重试</Button>
          : agent.state !== 'not_installed' && <Button variant="outline" size="sm" aria-haspopup="dialog" aria-label={`配置 ${AGENT_NAMES[agent.agent]}`} onClick={event => openPanel({ kind: 'configure', agent: agent.agent }, event.currentTarget)}>{running === agent.agent ? '检查中…' : '配置'}</Button>}
      </AgentEntry>)}
      {agents.some(agent => agent.state === 'not_installed') && <SettingsRow title="安装或修复运行时">
        <Button variant="ghost" size="sm" aria-haspopup="dialog" onClick={event => openPanel({ kind: 'catalog', unavailable: true }, event.currentTarget)}>查看安装指引<ChevronRight /></Button>
      </SettingsRow>}
    </SettingsGroup>
    <Sheet open={panel !== null} onOpenChange={open => { if (!open) closePanel() }}>
      <SheetContent side="right" className="fox-settings-sheet fox-agent-sheet" onCloseAutoFocus={event => {
        if (!panelTrigger.current?.isConnected) return
        event.preventDefault()
        panelTrigger.current.focus()
      }}>
        {panel?.kind === 'catalog' && <>
          <SheetHeader className="fox-settings-sheet-head">
            <span className="fox-settings-provider-mark is-large"><Bot aria-hidden="true" /></span>
            <div className="fox-settings-sheet-title"><SheetTitle>添加 Agent</SheetTitle><SheetDescription>查找和配置 Fox 支持的外部 Agent。</SheetDescription></div>
          </SheetHeader>
          <div className="fox-settings-sheet-body">
            <AgentCatalog agents={agents} unavailable={panel.unavailable} loading={detecting || (!overview && !error)} error={error} onDetect={detect} onConfigure={agent => setPanel({ kind: 'configure', agent: agent.agent })} />
          </div>
          <footer className="fox-settings-sheet-foot fox-agent-sheet-foot"><Button variant="outline" onClick={closePanel}>关闭</Button></footer>
        </>}
        {configured && <>
          <SheetHeader className="fox-settings-sheet-head">
            <AgentMark agent={configured.agent} large />
            <div className="fox-settings-sheet-title"><SheetTitle>配置 {AGENT_NAMES[configured.agent]}</SheetTitle><SheetDescription>设置模型来源并检查连接。</SheetDescription></div>
          </SheetHeader>
          <AgentBindingForm key={configured.agent} status={configured} providers={providers.items} developer={developer} checking={running !== null}
            operationError={operationError}
            failure={failedLayer && running !== configured.agent ? {
              message: failedLayer.message,
              retryConnection: failedLayer.layer === 'config_saved' || failedLayer.layer === 'model_request',
            } : undefined}
            environment={<section className="fox-settings-sheet-section">
              <div className="fox-settings-sheet-section-head"><h3>本机运行环境</h3></div>
              <dl className="fox-settings-facts">
                <div><dt>运行时</dt><dd>{configured.install.launchable ? 'Fox 自带运行时' : '不可用'}</dd></div>
                <div><dt>本机配置</dt><dd>{configured.native.changed ? '有更新，可重新导入' : configured.native.available ? '可导入' : '未检测到'}</dd></div>
                {developer && configured.diagnostic && <div><dt>诊断</dt><dd><code>{configured.diagnostic.code}{configured.diagnostic.detail && ` · ${configured.diagnostic.detail}`}</code></dd></div>}
              </dl>
              <div className="fox-settings-switch-row fox-agent-enable"><span><b>启用 Agent</b><small>连接验证通过后可用于新对话。</small></span>
                <Switch aria-label={`启用 ${AGENT_NAMES[configured.agent]}`} checked={configured.enabled} disabled={!configured.install.launchable || enableBusy === configured.agent} onCheckedChange={value => {
                  setEnableBusy(configured.agent); setAgentError(null)
                  void setExternalAgentEnabled(configured.agent, value).then(refresh).catch(cause => setAgentError({ agent: configured.agent, message: configErrorText(cause) })).finally(() => setEnableBusy(null))
                }} />
              </div>
            </section>}
            verification={(report || running === configured.agent) && <section className="fox-settings-sheet-section">
              <VerificationLadder report={report} running={running === configured.agent} developer={developer} onStop={() => {
                void desktopClient.agentConfigVerifyCancel(configured.agent).catch(cause => setAgentError({ agent: configured.agent, message: configErrorText(cause) }))
              }} />
              {running === configured.agent && approval && <RuntimeApprovalPrompt key={approval.id} approval={approval} onResolve={async (id, decision) => {
                const answered = await desktopClient.agentConfigVerifyAnswer(configured.agent, id, decision === 'allow_once')
                if (answered) setApproval(current => current?.id === id ? null : current)
                return answered
              }} />}
              <FullVerificationControl backend={configured.agent} busy={running !== null} onStart={(permissionMode, confirmFullAccess) => verify(configured.agent, 'tool', { permissionMode, confirmFullAccess })} />
            </section>}
            onVerify={() => void verify(configured.agent)} onDone={shouldVerify => {
              setReports(previous => ({ ...previous, [configured.agent]: undefined }))
              setAgentError(null)
              if (shouldVerify) void verify(configured.agent)
              else void refresh()
            }} onClose={closePanel} onOpenModelSettings={() => { closePanel(); onOpenModelSettings() }} />
        </>}
      </SheetContent>
    </Sheet>
  </>
}
