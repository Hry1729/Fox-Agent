import { useCallback, useEffect, useRef, useState } from 'react'
import { MoreHorizontal, Plus, Server, X } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Sheet, SheetClose, SheetContent, SheetDescription, SheetHeader, SheetOverlay, SheetPortal, SheetTitle } from '@/components/animate-ui/primitives/radix/sheet'
import { keepOpenOnWindowChrome } from '@/components/ui/window-chrome'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import { WorkspacePage } from '@/features/workspace/page-shell'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { ModelProviderRecord } from '@/features/conversations/model/types'
import { applyAgentEnablement } from '@/features/conversations/model/external-agent-enablement'
import { useDeveloperUi } from '@/features/chat/developer-ui'
import { notify as toast } from '@/features/notifications'
import { useModelProviders } from '../use-model-providers'
import { SettingsGroup, SettingsPageLayout, SettingsRow } from '../settings-layout'
import { AGENT_NAMES, API_NAMES, configErrorText } from './agent-config-copy'
import type { AgentConfigOverview, AgentConfigStatus, AgentId } from './agent-config-types'
import { ServiceForm } from './service-form'
import { ProviderCatalog } from './provider-catalog'
import { presetDraft, serviceDraft, providerIcon, type ServiceDraft, type ServicePreset } from './service-presets'
import './model-agent.css'

export function ModelAgentPage({ sidebarCollapsed, onSidebar }: { sidebarCollapsed: boolean; onSidebar: () => void }) {
  const providers = useModelProviders()
  const developer = useDeveloperUi()
  const [overview, setOverview] = useState<AgentConfigOverview | null>(null)
  const [error, setError] = useState('')
  const [draft, setDraft] = useState<ServiceDraft | null>(null)
  const [sheetOpen, setSheetOpen] = useState(false)
  const [catalogQuery, setCatalogQuery] = useState('')
  const addButton = useRef<HTMLButtonElement>(null)
  const returnFocus = useRef<HTMLElement | null>(null)
  const sheetBody = useRef<HTMLDivElement>(null)
  const [targets, setTargets] = useState<AgentId[]>(['fox'])
  const [offers, setOffers] = useState<AgentConfigStatus[]>([])
  const [offerProvider, setOfferProvider] = useState('')
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
  const providerError = providers.error === '这个服务地址已经添加过了' ? providers.error : providers.error ? '操作没有完成，请重试。' : null
  const openCatalog = () => {
    returnFocus.current = addButton.current
    setCatalogQuery('')
    setDraft(null)
    setTargets(['fox'])
    setSheetOpen(true)
  }
  const closeEditor = () => {
    setSheetOpen(false)
  }
  const pickProvider = (preset?: ServicePreset) => {
    const isDefault = providers.items.length === 0
    setDraft(preset ? presetDraft(preset, isDefault) : { ...serviceDraft(), isDefault })
  }
  const edit = (provider: ModelProviderRecord) => {
    const next = serviceDraft(provider)
    returnFocus.current = document.activeElement instanceof HTMLElement ? document.activeElement : null
    setDraft(next)
    setTargets([...new Set(overview?.services.find(service => service.providerId === provider.id)?.matches.map(match => match.agent) ?? ['fox'] as AgentId[])])
    setSheetOpen(true)
  }
  const stage = draft ? 'form' : 'catalog'
  useEffect(() => {
    if (!sheetOpen || !sheetBody.current) return
    sheetBody.current.scrollTop = 0
    sheetBody.current.querySelector<HTMLElement>('[data-provider-initial-focus], input')?.focus({ preventScroll: true })
  }, [sheetOpen, stage])
  const save = async () => {
    if (!draft) return
    const saved = await providers.save({ ...draft, apiKey: draft.apiKey || undefined })
    if (!saved) return
    closeEditor(); await refresh(); toast.success('模型服务已保存')
    const latest = await desktopClient.agentConfigOverview(developer).catch(() => null)
    setOfferProvider(saved.id)
    setOffers(latest?.agents.filter(agent => agent.agent !== 'fox' && targets.includes(agent.agent) && !agent.binding && agent.candidates.some(candidate => candidate.providerId === saved.id)) ?? [])
  }
  const remove = async (provider: ModelProviderRecord) => {
    if (!await confirmDialog({ title: '删除服务', description: `删除“${provider.name}”？绑定它的 Agent 需要重新配置。`, confirmLabel: '删除', destructive: true })) return
    if (await providers.remove(provider.id)) { closeEditor(); await refresh(); toast.success('模型服务已删除') }
  }
  const setDefault = async (provider: ModelProviderRecord, modelId?: string) => {
    await providers.save({ ...serviceDraft(provider), apiKey: undefined, isDefault: true, models: provider.models.map(model => ({ ...model, isDefault: modelId ? model.modelId === modelId : model.isDefault })) })
    await refresh()
  }
  const defaultProvider = providers.items.find(provider => provider.isDefault)
  const defaultModel = defaultProvider?.models.find(model => model.isDefault)
  const enabled = overview?.agents.filter(agent => agent.enabled).map(agent => agent.agent) ?? []
  const draftIcon = draft ? providerIcon(draft.name, draft.baseUrl, draft.icon) : null

  return <WorkspacePage title="模型" subtitle="默认模型与模型服务" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout className="fox-model-agent-page" title="模型" description="管理模型服务，设置 Fox 的默认模型。外部 Agent 的服务绑定在 Agent 页配置。" actions={<Button ref={addButton} onClick={openCatalog}><Plus />添加模型</Button>}>
      {error && <p role="alert" className="fox-model-error">{error}<Button variant="ghost" size="sm" onClick={() => void refresh()}>重试</Button></p>}
      {offers.map(agent => <div className="fox-agent-inline fox-model-actions" key={agent.agent}><span>要让 {AGENT_NAMES[agent.agent]} 使用这个服务吗？</span><Button size="sm" onClick={() => { const candidate = agent.candidates.find(item => item.providerId === offerProvider); if (!candidate || agent.agent === 'fox') return; void desktopClient.agentBindingSave(agent.agent, { source: 'fox_service', providerId: candidate.providerId, endpointId: candidate.endpointId, modelId: candidate.modelId }, developer).then(() => { setOffers(current => current.filter(item => item.agent !== agent.agent)); void refresh() }).catch(cause => setError(configErrorText(cause))) }}>使用</Button><Button variant="ghost" size="sm" onClick={() => setOffers(current => current.filter(item => item.agent !== agent.agent))}>暂不使用</Button></div>)}
        <SettingsGroup title="默认"><SettingsRow title="默认模型" description="新对话和 Fox 未指定模型时使用"><select aria-label="默认模型" disabled={providers.busyId !== null || !providers.items.length} value={defaultProvider && defaultModel ? `${defaultProvider.id}::${defaultModel.modelId}` : ''} onChange={event => { const [id, model] = event.target.value.split('::'); const provider = providers.items.find(item => item.id === id); if (provider) void setDefault(provider, model) }}><option value="" disabled>添加模型后可选择</option>{providers.items.flatMap(provider => provider.models.map(model => <option key={`${provider.id}:${model.modelId}`} value={`${provider.id}::${model.modelId}`}>{provider.name} · {model.displayName || model.modelId}</option>))}</select></SettingsRow></SettingsGroup>
        <SettingsGroup title="模型服务" aside={`${providers.items.length} 个服务`}>
          {providers.loading && !providers.items.length && <p className="fox-model-section" role="status">正在读取服务…</p>}
          {!providers.loading && !providers.items.length && <p className="fox-model-section">还没有模型服务。添加模型后可以选择默认模型。</p>}
          {providerError && <p role="alert" className="fox-model-error">{providerError}</p>}
          {providers.items.map(provider => {
            const service = overview?.services.find(item => item.providerId === provider.id)
            const icon = providerIcon(provider.name, provider.baseUrl, provider.icon)
            return <div className="fox-model-service-row" key={provider.id}>
              <button className="fox-model-service-main" type="button" aria-label={`编辑 ${provider.name}`} onClick={() => edit(provider)}><span className="fox-settings-provider-mark">{icon ? <img src={icon} alt="" /> : <Server />}</span><span><b>{provider.name}</b>{provider.isDefault && <span className="fox-agent-chip">默认</span>}<small>{provider.credentialConfigured ? `${provider.models.length} 个模型 · ${safeHost(provider.baseUrl)}` : '未填写 API Key'}</small></span><span className="fox-model-service-status">{provider.lastStatus === 'connected' ? '已连接' : provider.lastStatus === 'unavailable' ? '不可用' : '未检测'}{provider.lastLatencyMs != null ? ` · ${provider.lastLatencyMs} ms` : ''}</span></button>
              <DropdownMenu modal={false}><DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" aria-label={`管理 ${provider.name}`}><MoreHorizontal /></Button></DropdownMenuTrigger><DropdownMenuContent align="end"><DropdownMenuItem onSelect={() => runAfterMenuClose(() => edit(provider))}>编辑</DropdownMenuItem><DropdownMenuItem disabled={provider.isDefault} onSelect={() => void setDefault(provider)}>设为默认</DropdownMenuItem><DropdownMenuItem onSelect={() => { void desktopClient.testModelService(provider.baseUrl, undefined, provider.apiType, provider.models.find(model => model.isDefault)?.modelId, provider.id).then(() => { void providers.refresh(); void refresh(); toast.success('连接正常') }).catch(cause => toast.error(configErrorText(cause))) }}>测试连接</DropdownMenuItem><DropdownMenuSeparator /><DropdownMenuItem variant="destructive" onSelect={() => runAfterMenuClose(() => void remove(provider))}>删除</DropdownMenuItem></DropdownMenuContent></DropdownMenu>
              <div className="fox-service-matches">{(Object.keys(AGENT_NAMES) as AgentId[]).map(agent => { const matches = service?.matches.filter(match => match.agent === agent) ?? []; if (!matches.length) return null; const verified = matches.some(match => match.verified); return <span className={`fox-agent-chip ${verified ? 'is-verified' : ''}`} key={agent} title={developer ? matches.map(match => `${match.apiType} · ${match.endpointId} · ${match.verifiedAt ?? ''}`).join(' / ') : undefined}>{AGENT_NAMES[agent]} {verified ? '✓ 已实测' : '可用 · 未实测'}</span> })}{service?.missing.filter(match => enabled.includes(match.agent)).map(match => <small key={`${match.agent}:${match.apiType}`}>{AGENT_NAMES[match.agent]} 缺少 {API_NAMES[match.apiType]} 入口</small>)}</div>
            </div>
          })}
        </SettingsGroup>
    </SettingsPageLayout>
    <Sheet open={sheetOpen} onOpenChange={open => { if (!open) closeEditor() }}>
      <SheetPortal>
        <SheetOverlay className="fixed inset-x-0 bottom-0 top-[var(--fox-titlebar-height,0px)] z-50 bg-black/20 dark:bg-black/50" />
        <SheetContent side="right" className="fox-settings-sheet fox-provider-sheet z-50 flex flex-col border-l bg-popover text-popover-foreground shadow-lg"
          style={{ insetBlockStart: 'var(--fox-titlebar-height, 0px)' }}
          onPointerDownOutside={keepOpenOnWindowChrome()}
          onInteractOutside={keepOpenOnWindowChrome()}
          onFocusOutside={keepOpenOnWindowChrome()}
          onCloseAutoFocus={event => {
            event.preventDefault()
            setDraft(null)
            const target = returnFocus.current?.isConnected ? returnFocus.current : addButton.current
            target?.focus({ preventScroll: true })
          }}>
          <SheetHeader className="fox-settings-sheet-head">
            <span className="fox-settings-provider-mark">{draftIcon ? <img src={draftIcon} alt="" /> : draft ? <Server /> : <Plus />}</span>
            <div className="fox-settings-sheet-title">
              <SheetTitle><span>{draft ? draft.id ? '编辑模型服务' : draft.name || '自定义供应商' : '添加模型'}</span></SheetTitle>
              <SheetDescription>{draft ? draft.id ? draft.name : '填写连接信息和模型。' : '选择供应商，或连接自己的 API 地址。'}</SheetDescription>
            </div>
          </SheetHeader>
          <SheetClose asChild><Button type="button" variant="ghost" size="icon-sm" className="absolute" aria-label="关闭"><X /></Button></SheetClose>
          {draft ? <ServiceForm bodyRef={sheetBody} form={draft} onChange={setDraft} targets={targets} onTargets={setTargets} enabled={enabled} existingServices={providers.items}
            credentialConfigured={providers.items.find(provider => provider.id === draft.id)?.credentialConfigured ?? false}
            busy={providers.busyId !== null} error={providerError}
            onBack={draft.id ? undefined : () => setDraft(null)} onCancel={closeEditor} onSave={() => void save()}
            onDelete={draft.id ? () => { const provider = providers.items.find(item => item.id === draft.id); if (provider) void remove(provider) } : undefined} /> : <>
            <div ref={sheetBody} className="fox-settings-sheet-body"><ProviderCatalog query={catalogQuery} onQuery={setCatalogQuery} onPick={pickProvider} /></div>
            <div className="fox-settings-sheet-foot fox-provider-catalog-foot"><SheetClose asChild><Button type="button" variant="outline">取消</Button></SheetClose></div>
          </>}
        </SheetContent>
      </SheetPortal>
    </Sheet>
  </WorkspacePage>
}
function safeHost(address: string) { try { return new URL(address).host } catch { return '地址待检查' } }
