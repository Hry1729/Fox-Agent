import { useCallback, useEffect, useRef, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { MoreHorizontal, Plus, Server } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import { WorkspacePage } from '@/features/workspace/page-shell'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { ModelProviderRecord } from '@/features/conversations/model/types'
import { applyAgentEnablement, setExternalAgentEnabled } from '@/features/conversations/model/external-agent-enablement'
import { useDeveloperUi } from '@/features/chat/developer-ui'
import { notify as toast } from '@/features/notifications'
import { useModelProviders } from '../use-model-providers'
import { SettingsGroup, SettingsPageLayout, SettingsRow } from '../settings-layout'
import { AGENT_NAMES, API_NAMES, STATE_LABELS, configErrorText } from './agent-config-copy'
import type { AgentConfigOverview, AgentConfigStatus, AgentId, VerificationReport, VerifyDepth } from './agent-config-types'
import { ServiceForm } from './service-form'
import { serviceDraft, providerIcon, type ServiceDraft } from './service-presets'
import { AgentBindingForm } from './agent-binding-form'
import { RuntimeApprovalPrompt } from '@/features/chat/components/RuntimeApprovalPrompt'
import type { ApprovalRecord } from '@/features/conversations/model/types'
import { FullVerificationControl } from './full-verification-control'
import { VerificationLadder } from './verification-ladder'
import './model-agent.css'

export function ModelAgentPage({ sidebarCollapsed, onSidebar }: { sidebarCollapsed: boolean; onSidebar: () => void }) {
  const providers = useModelProviders()
  const developer = useDeveloperUi()
  const [overview, setOverview] = useState<AgentConfigOverview | null>(null)
  const [error, setError] = useState('')
  const [draft, setDraft] = useState<ServiceDraft | null>(null)
  const [targets, setTargets] = useState<AgentId[]>(['fox'])
  const [binding, setBinding] = useState<{ agent: AgentId; native: boolean } | null>(null)
  const [running, setRunning] = useState<AgentId | null>(null)
  const [reports, setReports] = useState<Partial<Record<AgentId, VerificationReport>>>({})
  const [approval, setApproval] = useState<ApprovalRecord | null>(null)
  const active = useRef<AgentId | null>(null)
  useEffect(() => () => {
    if (active.current) void desktopClient.agentConfigVerifyCancel(active.current).catch(() => {})
  }, [])
  const [showReport, setShowReport] = useState<AgentId | null>(null)
  const [offers, setOffers] = useState<AgentConfigStatus[]>([])
  const [offerProvider, setOfferProvider] = useState('')
  const [enableBusy, setEnableBusy] = useState<AgentId | null>(null)
  const refresh = useCallback(async () => {
    try { const value = await desktopClient.agentConfigOverview(developer); setOverview(value); applyAgentEnablement(value.agents); setError('') }
    catch (cause) { setError(configErrorText(cause)) }
  }, [developer])
  useEffect(() => {
    void refresh()
    const update = () => { void refresh() }
    window.addEventListener('focus', update)
    window.addEventListener('fox:model-service-changed', update)
    return () => { window.removeEventListener('focus', update); window.removeEventListener('fox:model-service-changed', update) }
  }, [refresh])
  useEffect(() => {
    if (!running) return
    let disposed = false
    let unlisten: (() => void) | undefined
    void listen<{ backend: AgentId; report: VerificationReport }>('agent-config-verification', event => {
      if (event.payload.backend === running) setReports(previous => ({ ...previous, [running]: event.payload.report }))
    }).then(stop => { if (disposed) stop(); else unlisten = stop }).catch(() => {})
    return () => { disposed = true; unlisten?.() }
  }, [running])
  const providerError = providers.error === '这个服务地址已经添加过了' ? providers.error : providers.error ? '操作没有完成，请重试。' : null
  const edit = (provider?: ModelProviderRecord) => {
    const next = serviceDraft(provider)
    if (!provider) next.isDefault = providers.items.length === 0
    setDraft(next)
    setTargets(provider ? [...new Set(overview?.services.find(s => s.providerId === provider.id)?.matches.map(m => m.agent) ?? ['fox'] as AgentId[])] : ['fox'])
  }
  const verify = async (agent: AgentId, depth: VerifyDepth = 'model', selection?: { permissionMode: string; confirmFullAccess: boolean }) => {
    if (active.current) return
    active.current = agent
    setApproval(null)
    setRunning(agent); setShowReport(agent); setReports(previous => { const next = { ...previous }; delete next[agent]; return next }); setError('')
    try { const report = await desktopClient.agentConfigVerify(agent, depth, developer, selection && { ...selection, onApproval: setApproval }); setReports(previous => ({ ...previous, [agent]: report })) }
    catch (cause) { setError(configErrorText(cause)) }
    finally { active.current = null; setApproval(null); setRunning(null); await refresh() }
  }
  const save = async () => {
    if (!draft) return
    const saved = await providers.save({ ...draft, apiKey: draft.apiKey || undefined })
    if (!saved) return
    setDraft(null); await refresh(); toast.success('模型服务已保存')
    const latest = await desktopClient.agentConfigOverview(developer).catch(() => null)
    setOfferProvider(saved.id)
    setOffers(latest?.agents.filter(agent => agent.agent !== 'fox' && targets.includes(agent.agent) && !agent.binding && agent.candidates.some(c => c.providerId === saved.id)) ?? [])
  }
  const remove = async (provider: ModelProviderRecord) => {
    if (!await confirmDialog({ title: '删除服务', description: `删除“${provider.name}”？绑定它的 Agent 需要重新配置。`, confirmLabel: '删除', destructive: true })) return
    if (await providers.remove(provider.id)) { setDraft(null); await refresh(); toast.success('模型服务已删除') }
  }
  const setDefault = async (provider: ModelProviderRecord, modelId?: string) => {
    await providers.save({ ...serviceDraft(provider), apiKey: undefined, isDefault: true, models: provider.models.map(m => ({ ...m, isDefault: modelId ? m.modelId === modelId : m.isDefault })) })
    await refresh()
  }
  const defaultProvider = providers.items.find(p => p.isDefault)
  const defaultModel = defaultProvider?.models.find(m => m.isDefault)
  const enabled = overview?.agents.filter(a => a.enabled).map(a => a.agent) ?? []
  const statusText = (agent: AgentConfigStatus) => {
    if (agent.state === 'read_failed') return '没能读取配置，请重试。'
    if (agent.state === 'not_installed') return agent.install.blocker === 'system_only' ? '电脑上装了这个 Agent，但 Fox 需要自带运行时。请修复 Fox。' : agent.install.detected ? '这个 Agent 的运行时不完整，请修复 Fox 外部运行时。' : 'Fox 运行时里没有这个 Agent。'
    if (agent.binding?.source === 'native') return agent.native.changed ? '本机配置已变化，请重新导入。' : '使用从这台电脑导入的配置'
    const bound = agent.binding
    if (bound?.source === 'fox_service') return `${providers.items.find(p => p.id === bound.providerId)?.name ?? '服务已删除'} · ${bound.modelId}`
    if (agent.agent === 'fox') return defaultProvider ? `${defaultProvider.name} · ${defaultModel?.modelId ?? '请选择模型'}` : '请先添加模型服务'
    if (agent.state === 'native_importable') return `在这台电脑上找到了 ${AGENT_NAMES[agent.agent]} 的登录配置。`
    return agent.candidates.length ? '选择一个模型服务' : `请添加支持 ${agent.agent === 'codex' ? 'Responses' : agent.agent === 'deepseek' ? 'Messages' : '匹配协议'} 的服务`
  }
  return <WorkspacePage title="模型与 Agent" subtitle="设置" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout className="fox-model-agent-page" title="模型与 Agent" description="管理模型服务，并为每个 Agent 选择使用哪个服务。" actions={draft ? <Button variant="outline" onClick={() => setDraft(null)}>取消</Button> : <Button onClick={() => edit()}><Plus />添加服务</Button>}>
      {error && <p role="alert" className="fox-model-error">{error}<Button variant="ghost" size="sm" onClick={() => void refresh()}>重试</Button></p>}
      {offers.map(agent => <div className="fox-agent-inline fox-model-actions" key={agent.agent}><span>要让 {AGENT_NAMES[agent.agent]} 使用这个服务吗？</span><Button size="sm" onClick={() => { const candidate = agent.candidates.find(c => c.providerId === offerProvider); if (!candidate || agent.agent === 'fox') return; void desktopClient.agentBindingSave(agent.agent, { source: 'fox_service', providerId: candidate.providerId, endpointId: candidate.endpointId, modelId: candidate.modelId }, developer).then(() => { setOffers(current => current.filter(a => a.agent !== agent.agent)); void refresh() }).catch(cause => setError(configErrorText(cause))) }}>使用</Button><Button variant="ghost" size="sm" onClick={() => setOffers(current => current.filter(a => a.agent !== agent.agent))}>暂不使用</Button></div>)}
      {draft ? <ServiceForm form={draft} onChange={setDraft} targets={targets} onTargets={setTargets} enabled={enabled} existingServices={providers.items} credentialConfigured={providers.items.find(p => p.id === draft.id)?.credentialConfigured ?? false} busy={providers.busyId !== null} error={providerError} onCancel={() => setDraft(null)} onSave={() => void save()} onDelete={draft.id ? () => { const provider = providers.items.find(p => p.id === draft.id); if (provider) void remove(provider) } : undefined} /> : <>
        <SettingsGroup title="默认"><SettingsRow title="默认模型" description="新对话和 Fox 未指定模型时使用"><select aria-label="默认模型" disabled={providers.busyId !== null || !providers.items.length} value={defaultProvider && defaultModel ? `${defaultProvider.id}::${defaultModel.modelId}` : ''} onChange={e => { const [id, model] = e.target.value.split('::'); const provider = providers.items.find(p => p.id === id); if (provider) void setDefault(provider, model) }}><option value="" disabled>添加服务后可选择</option>{providers.items.flatMap(p => p.models.map(m => <option key={`${p.id}:${m.modelId}`} value={`${p.id}::${m.modelId}`}>{p.name} · {m.displayName || m.modelId}</option>))}</select></SettingsRow></SettingsGroup>
        <SettingsGroup title="模型服务" aside={`${providers.items.length} 个服务`}>
          {providers.loading && !providers.items.length && <p className="fox-model-section" role="status">正在读取服务…</p>}
          {!providers.loading && !providers.items.length && <p className="fox-model-section">还没有模型服务。点“添加服务”开始，先选你想让哪些 Agent 使用它。</p>}
          {providerError && <p role="alert" className="fox-model-error">{providerError}</p>}
          {providers.items.map(provider => {
            const service = overview?.services.find(s => s.providerId === provider.id)
            const icon = providerIcon(provider.name, provider.baseUrl, provider.icon)
            return <div className="fox-model-service-row" key={provider.id}>
              <button className="fox-model-service-main" type="button" aria-label={`编辑 ${provider.name}`} onClick={() => edit(provider)}><span className="fox-settings-provider-mark">{icon ? <img src={icon} alt="" /> : <Server />}</span><span><b>{provider.name}</b>{provider.isDefault && <span className="fox-agent-chip">默认</span>}<small>{provider.credentialConfigured ? `${provider.models.length} 个模型 · ${safeHost(provider.baseUrl)}` : '未填写 API Key'}</small></span><span className="fox-model-service-status">{provider.lastStatus === 'connected' ? '已连接' : provider.lastStatus === 'unavailable' ? '不可用' : '未检测'}{provider.lastLatencyMs != null ? ` · ${provider.lastLatencyMs} ms` : ''}</span></button>
              <DropdownMenu modal={false}><DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" aria-label={`管理 ${provider.name}`}><MoreHorizontal /></Button></DropdownMenuTrigger><DropdownMenuContent align="end"><DropdownMenuItem onSelect={() => edit(provider)}>编辑</DropdownMenuItem><DropdownMenuItem disabled={provider.isDefault} onSelect={() => void setDefault(provider)}>设为默认</DropdownMenuItem><DropdownMenuItem onSelect={() => { void desktopClient.testModelService(provider.baseUrl, undefined, provider.apiType, provider.models.find(m => m.isDefault)?.modelId, provider.id).then(() => { void providers.refresh(); void refresh(); toast.success('连接正常') }).catch(cause => toast.error(configErrorText(cause))) }}>测试连接</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem variant="destructive" onSelect={() => runAfterMenuClose(() => void remove(provider))}>删除</DropdownMenuItem></DropdownMenuContent></DropdownMenu>
              <div className="fox-service-matches">{(Object.keys(AGENT_NAMES) as AgentId[]).map(agent => { const matches = service?.matches.filter(m => m.agent === agent) ?? []; if (!matches.length) return null; const verified = matches.some(m => m.verified); return <span className={`fox-agent-chip ${verified ? 'is-verified' : ''}`} key={agent} title={developer ? matches.map(m => `${m.apiType} · ${m.endpointId} · ${m.verifiedAt ?? ''}`).join(' / ') : undefined}>{AGENT_NAMES[agent]} {verified ? '✓ 已实测' : '可用 · 未实测'}</span> })}{service?.missing.filter(m => enabled.includes(m.agent)).map(m => <small key={`${m.agent}:${m.apiType}`}>{AGENT_NAMES[m.agent]} 缺少 {API_NAMES[m.apiType]} 入口</small>)}</div>
            </div>
          })}
        </SettingsGroup>
        <SettingsGroup title="Agent 使用配置" aside="外部 Agent 需要先启用">
          {!overview && !error && <p className="fox-model-section" role="status">正在读取 Agent 配置…</p>}
          {overview?.agents.map(agent => <div className="fox-agent-config-row" key={agent.agent}>
            <div className="fox-agent-config-main"><div><b>{AGENT_NAMES[agent.agent]}</b><small data-ready={agent.state === 'ready'}>{STATE_LABELS[agent.state]}</small></div><div className="fox-agent-description"><span>{statusText(agent)}</span>{agent.enabled && !['ready', 'not_installed', 'read_failed'].includes(agent.state) && <small>启用后需要先配置并验证，才能在对话里使用。</small>}{developer && agent.diagnostic && <code>{agent.diagnostic.code}{agent.diagnostic.detail && ` · ${agent.diagnostic.detail}`}</code>}</div><div className="fox-model-actions">
              {agent.state === 'read_failed' ? <Button variant="outline" size="sm" onClick={() => void refresh()}>重试</Button> : agent.state !== 'not_installed' && <>
                {agent.agent === 'fox' ? <Button variant="ghost" size="sm" onClick={() => { if (defaultProvider) edit(defaultProvider); else edit() }}>查看</Button> : <>
                  {agent.binding && <Button variant="outline" size="sm" disabled={running !== null} onClick={() => void verify(agent.agent)}>验证</Button>}
                  {agent.state === 'native_importable' && <Button variant="outline" size="sm" onClick={() => setBinding({ agent: agent.agent, native: true })}>导入…</Button>}
                  <Button variant="ghost" size="sm" disabled={running === agent.agent} onClick={() => { if (!agent.candidates.length && !agent.native.available) { edit(); setTargets([agent.agent]) } else setBinding({ agent: agent.agent, native: false }) }}>{agent.binding ? '更改' : agent.candidates.length || agent.native.available ? '配置' : '添加服务'}</Button>
                </>}
              </>}
              {agent.agent !== 'fox' && <Switch aria-label={`启用 ${AGENT_NAMES[agent.agent]}`} checked={agent.enabled} disabled={agent.state === 'not_installed' || agent.state === 'read_failed' || enableBusy === agent.agent} onCheckedChange={value => { setEnableBusy(agent.agent); void setExternalAgentEnabled(agent.agent, value).then(refresh).catch(cause => setError(configErrorText(cause))).finally(() => setEnableBusy(null)) }} />}
            </div></div>
            {binding?.agent === agent.agent && <AgentBindingForm status={agent} providers={providers.items} developer={developer} initialNative={binding.native} onCancel={() => setBinding(null)} onDone={shouldVerify => { setBinding(null); void refresh(); if (shouldVerify) void verify(agent.agent) }} />}
            {(showReport === agent.agent || agent.verification) && <>
              <VerificationLadder report={running === agent.agent ? reports[agent.agent] ?? null : reports[agent.agent] ?? agent.verification} running={running === agent.agent} developer={developer} onStop={() => { void desktopClient.agentConfigVerifyCancel(agent.agent).catch(cause => setError(configErrorText(cause))) }} />
              {running === agent.agent && approval && <RuntimeApprovalPrompt key={approval.id} approval={approval} onResolve={async (id, decision) => {
                const answered = await desktopClient.agentConfigVerifyAnswer(agent.agent, id, decision === 'allow_once')
                if (answered) setApproval(current => current?.id === id ? null : current)
                return answered
              }} />}
              {agent.agent !== 'fox' && <FullVerificationControl backend={agent.agent} busy={running !== null} onStart={(permissionMode, confirmFullAccess) => verify(agent.agent, 'tool', { permissionMode, confirmFullAccess })} />}
            </>}
          </div>)}
        </SettingsGroup>
      </>}
    </SettingsPageLayout>
  </WorkspacePage>
}
function safeHost(address: string) { try { return new URL(address).host } catch { return '地址待检查' } }
