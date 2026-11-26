import { useState, type ReactNode } from 'react'
import { CircleAlert, Zap } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { ModelProviderRecord } from '@/features/conversations/model/types'
import { AgentImportPreview } from './agent-import-preview'
import { API_NAMES, configErrorText } from './agent-config-copy'
import type { ExternalAgentStatus } from './agent-catalog'

export function AgentBindingForm({ status, providers, developer, checking, failure, operationError, environment, verification, onVerify, onDone, onClose, onOpenModelSettings }: {
  status: ExternalAgentStatus
  providers: ModelProviderRecord[]
  developer: boolean
  checking: boolean
  failure?: { message: string; retryConnection: boolean }
  operationError?: string
  environment: ReactNode
  verification: ReactNode
  onVerify: () => void
  onDone: (verify: boolean) => void
  onClose: () => void
  onOpenModelSettings: () => void
}) {
  const backend = status.agent
  const binding = status.binding?.source === 'fox_service' ? status.binding : null
  const first = status.candidates.find(c => c.providerId === binding?.providerId && c.endpointId === binding?.endpointId) ?? status.candidates[0]
  const [source, setSource] = useState(status.binding?.source === 'native' || status.state === 'native_importable' ? 'native' : 'fox_service')
  const [providerId, setProviderId] = useState(first?.providerId ?? '')
  const [endpointId, setEndpointId] = useState(first?.endpointId ?? '')
  const [modelId, setModelId] = useState(binding?.modelId ?? first?.modelId ?? '')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const endpoints = status.candidates.filter(c => c.providerId === providerId)
  const provider = providers.find(p => p.id === providerId)
  const models = new Map(provider?.models.map(model => [model.modelId, model.displayName || model.modelId]))
  for (const endpoint of endpoints) if (endpoint.modelId && !models.has(endpoint.modelId)) models.set(endpoint.modelId, endpoint.modelId)
  const saved = source === 'native' ? status.binding?.source === 'native'
    : binding?.providerId === providerId && binding.endpointId === endpointId && binding.modelId === modelId
  const disabled = busy || checking
  const footerError = error || operationError || (saved ? failure?.message : '')
  const save = async (verify: boolean) => {
    setBusy(true); setError('')
    try { await desktopClient.agentBindingSave(backend, { source: 'fox_service', providerId, endpointId, modelId }, developer); onDone(verify) }
    catch (cause) { setError(configErrorText(cause)) }
    finally { setBusy(false) }
  }
  return <>
    <div className="fox-settings-sheet-body">
      {environment}
      <section className="fox-settings-sheet-section">
        <div className="fox-settings-sheet-section-head"><h3>模型来源</h3></div>
        <select aria-label="模型来源" value={source} disabled={disabled} onChange={event => { setSource(event.target.value); setError('') }}>
          <option value="fox_service">Fox 模型服务</option>
          <option value="native" disabled={!status.native.available && status.binding?.source !== 'native'}>本机配置</option>
        </select>
        {source === 'native' ? status.native.available
          ? <AgentImportPreview backend={backend} developer={developer} onDone={() => onDone(false)} onCancel={onClose} />
          : <p className="fox-settings-sheet-hint">使用已导入的本机配置。</p>
          : <div className="fox-agent-binding-fields">
            {status.candidates.length > 0 ? <>
              <label className="fox-settings-field"><span>服务</span><select value={providerId} disabled={disabled} onChange={event => {
                const next = status.candidates.find(candidate => candidate.providerId === event.target.value)
                setProviderId(event.target.value)
                setEndpointId(next?.endpointId ?? '')
                setModelId(next?.modelId ?? '')
                setError('')
              }}><option value="" disabled>选择模型服务</option>{providers.filter(p => status.candidates.some(c => c.providerId === p.id)).map(p => <option key={p.id} value={p.id}>{p.name}</option>)}</select></label>
              {endpoints.length > 1 && <label className="fox-settings-field"><span>入口</span><select value={endpointId} disabled={disabled} onChange={event => {
                setEndpointId(event.target.value)
                setModelId(endpoints.find(candidate => candidate.endpointId === event.target.value)?.modelId ?? '')
                setError('')
              }}>{endpoints.map(candidate => <option key={candidate.endpointId} value={candidate.endpointId}>{API_NAMES[candidate.apiType]} · {candidate.verified ? '已实测' : '未实测'}</option>)}</select></label>}
              <label className="fox-settings-field"><span>模型</span><select value={modelId} disabled={disabled} onChange={event => { setModelId(event.target.value); setError('') }}><option value="" disabled>选择模型</option>{[...models].map(([id, name]) => <option key={id} value={id}>{name}</option>)}</select></label>
            </> : <><p className="fox-settings-sheet-hint">请先添加该 Agent 支持的模型服务。</p><Button variant="outline" onClick={onOpenModelSettings}>前往模型设置</Button></>}
          </div>}
        {status.binding && <Button type="button" className="fox-agent-clear-binding" variant="ghost" disabled={disabled} onClick={() => {
          setBusy(true); setError('')
          void desktopClient.agentBindingClear(backend, developer).then(() => onDone(false)).catch(cause => setError(configErrorText(cause))).finally(() => setBusy(false))
        }}>清除绑定</Button>}
      </section>
      {verification}
    </div>
    <footer className="fox-settings-sheet-foot fox-agent-sheet-foot">
      {footerError && <p role="alert" className="fox-agent-sheet-error"><CircleAlert aria-hidden="true" />{footerError}</p>}
      <Button type="button" variant="outline" onClick={onClose}>关闭</Button>
      {saved ? <Button type="button" disabled={disabled} onClick={onVerify}><Zap />{checking ? '检查中…' : failure?.retryConnection ? '重试检查' : '检查连接'}</Button>
        : source === 'fox_service' && <>
          <Button type="button" variant="outline" disabled={disabled || !providerId || !modelId} onClick={() => void save(false)}>保存</Button>
          <Button type="button" disabled={disabled || !providerId || !modelId} onClick={() => void save(true)}>保存并验证</Button>
        </>}
    </footer>
  </>
}
