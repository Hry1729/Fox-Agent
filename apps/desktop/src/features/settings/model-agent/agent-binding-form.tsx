import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { ModelProviderRecord } from '@/features/conversations/model/types'
import { AgentImportPreview } from './agent-import-preview'
import { API_NAMES, configErrorText } from './agent-config-copy'
import type { AgentConfigStatus, ExternalAgentId } from './agent-config-types'

export function AgentBindingForm({ status, providers, developer, initialNative = false, onDone, onCancel }: { status: AgentConfigStatus; providers: ModelProviderRecord[]; developer: boolean; initialNative?: boolean; onDone: (verify: boolean) => void; onCancel: () => void }) {
  const backend = status.agent as ExternalAgentId
  const binding = status.binding?.source === 'fox_service' ? status.binding : null
  const first = status.candidates.find(c => c.providerId === binding?.providerId && c.endpointId === binding?.endpointId) ?? status.candidates[0]
  const [source, setSource] = useState(initialNative ? 'native' : 'fox_service')
  const [providerId, setProviderId] = useState(first?.providerId ?? '')
  const [endpointId, setEndpointId] = useState(first?.endpointId ?? '')
  const [modelId, setModelId] = useState(binding?.modelId ?? first?.modelId ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const endpoints = status.candidates.filter(c => c.providerId === providerId)
  const provider = providers.find(p => p.id === providerId)
  const save = async (verify: boolean) => {
    setBusy(true); setError('')
    try { await desktopClient.agentBindingSave(backend, { source: 'fox_service', providerId, endpointId, modelId }, developer); onDone(verify) }
    catch (cause) { setError(configErrorText(cause)) }
    finally { setBusy(false) }
  }
  return <div className="fox-agent-inline">
    <div className="fox-model-actions" aria-label="配置来源"><Button type="button" variant={source === 'fox_service' ? 'default' : 'outline'} onClick={() => setSource('fox_service')}>Fox 模型服务</Button><Button type="button" variant={source === 'native' ? 'default' : 'outline'} disabled={!status.native.available} title={status.native.available ? undefined : '这台电脑上没有找到该 Agent 的配置'} onClick={() => setSource('native')}>本机配置</Button></div>
    {source === 'native' ? <AgentImportPreview backend={backend} developer={developer} onDone={() => onDone(false)} onCancel={onCancel} /> : <div className="fox-model-fields">
      <label>服务<select value={providerId} onChange={e => { const next = status.candidates.find(c => c.providerId === e.target.value); setProviderId(e.target.value); setEndpointId(next?.endpointId ?? ''); setModelId(next?.modelId ?? '') }}><option value="" disabled>选择模型服务</option>{providers.filter(p => status.candidates.some(c => c.providerId === p.id)).map(p => <option key={p.id} value={p.id}>{p.name} · {status.candidates.some(c => c.providerId === p.id && c.verified) ? '已有实测入口' : '未实测'}</option>)}</select></label>
      {endpoints.length > 1 && <label>入口<select value={endpointId} onChange={e => { setEndpointId(e.target.value); setModelId(endpoints.find(c => c.endpointId === e.target.value)?.modelId ?? '') }}>{endpoints.map(c => <option key={c.endpointId} value={c.endpointId}>{API_NAMES[c.apiType]} · {c.verified ? '已实测' : '未实测'}</option>)}</select></label>}
      <label>模型<select value={modelId} onChange={e => setModelId(e.target.value)}><option value="" disabled>选择模型</option>{Array.from(new Map([...(provider?.models.map(m => [m.modelId, m.displayName || m.modelId] as const) ?? []), ...endpoints.filter(c => c.modelId).map(c => [c.modelId, c.modelId] as const)].map(pair => pair)).entries()).map(([id, name]) => <option key={id} value={id}>{name}</option>)}</select></label>
      {error && <p role="alert" className="fox-model-error">{error}</p>}
      <div className="fox-model-actions">{status.binding && <Button type="button" variant="ghost" disabled={busy} onClick={() => { setBusy(true); void desktopClient.agentBindingClear(backend, developer).then(() => onDone(false)).catch(cause => setError(configErrorText(cause))).finally(() => setBusy(false)) }}>清除绑定</Button>}<span className="is-grow" /><Button type="button" variant="outline" onClick={onCancel}>取消</Button><Button type="button" variant="outline" disabled={busy || !providerId || !modelId} onClick={() => void save(false)}>保存</Button><Button type="button" disabled={busy || !providerId || !modelId} onClick={() => void save(true)}>保存并验证</Button></div>
    </div>}
  </div>
}
