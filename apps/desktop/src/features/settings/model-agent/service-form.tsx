import { useEffect, useRef, useState, type Ref } from 'react'
import { ArrowLeft, ChevronRight, Eye, EyeOff, Info, LoaderCircle, Plus, Zap } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { useDeveloperUi } from '@/features/chat/developer-ui'
import { ProviderProtocolEditor } from '../provider-protocol-editor'
import { DUPLICATE_SERVICE_URL_TEXT, findDuplicateServiceUrl } from '../provider-errors'
import { servicePresets, providerIconOptions, type ServiceDraft } from './service-presets'
import { AGENT_NAMES, API_NAMES, configErrorText } from './agent-config-copy'
import type { AgentId, ApiType } from './agent-config-types'

const primaryProtocols = ['openai-completions', 'anthropic-messages'] as const

export function ServiceForm({ form, onChange, targets, onTargets, enabled, busy, credentialConfigured, existingServices, error, onSave, onCancel, onDelete, onBack, bodyRef }: {
  form: ServiceDraft; onChange: (value: ServiceDraft) => void; targets: AgentId[]; onTargets: (value: AgentId[]) => void
  enabled: AgentId[]; busy: boolean; credentialConfigured: boolean; existingServices: { id: string; baseUrl: string }[]
  error: string | null; onSave: () => void; onCancel: () => void; onDelete?: () => void
  onBack?: () => void; bodyRef?: Ref<HTMLDivElement>
}) {
  const developer = useDeveloperUi()
  const [preset] = useState(() => servicePresets.find(item => item.baseUrl === form.baseUrl))
  const fullForm = Boolean(form.id) || !preset
  const [showKey, setShowKey] = useState(false)
  const [modelText, setModelText] = useState(() => form.models.map(item => item.modelId).filter(Boolean).join(', '))
  const [testing, setTesting] = useState(false)
  const [result, setResult] = useState<{ identity: string; text: string; ok: boolean } | null>(null)
  const mounted = useRef(false)
  useEffect(() => {
    mounted.current = true
    return () => { mounted.current = false }
  }, [])
  const identity = JSON.stringify(form)
  const model = form.models.find(item => item.isDefault) ?? form.models[0]
  const patch = (values: Partial<ServiceDraft>) => onChange({ ...form, ...values })
  const duplicate = findDuplicateServiceUrl(existingServices, form.id, form.baseUrl) !== null
  const hasAddress = Boolean(form.baseUrl.trim())
  const hasModel = Boolean(model.modelId.trim())
  const hasMissingModel = form.models.some(item => !item.modelId.trim())
  const missing = (api: ApiType) => api !== form.apiType && !(form.protocolProfiles ?? []).some(profile => profile.apiType === api && profile.baseUrl.trim())
  const missingTarget = targets.includes('codex') && missing('openai-responses') ? 'Codex 需要 OpenAI Responses 地址。'
    : targets.includes('deepseek') && missing('anthropic-messages') ? 'DeepSeek（dsh）需要 Anthropic Messages 地址。' : ''
  const canTest = hasAddress && !duplicate && (form.apiType === 'openai-completions' || hasModel)
  const valid = Boolean(form.name.trim() && hasAddress && !hasMissingModel && !duplicate && !missingTarget)
  const status = result?.identity === identity ? result : null
  const canSave = valid && Boolean(form.id || status?.ok)
  const updateModels = (models: ServiceDraft['models']) => {
    setModelText(models.map(item => item.modelId).filter(Boolean).join(', '))
    patch({ models })
  }
  const setModelIds = (value: string) => {
    setModelText(value)
    const ids = [...new Set(value.split(/[,，\n]/).map(item => item.trim()).filter(Boolean))]
    const defaultId = ids.includes(model.modelId) ? model.modelId : ids[0] ?? ''
    patch({ models: (ids.length ? ids : ['']).map(modelId => {
      const prior = form.models.find(item => item.modelId === modelId)
      return prior ? { ...prior, isDefault: modelId === defaultId }
        : { ...model, id: undefined, modelId, displayName: '', isDefault: modelId === defaultId }
    }) })
  }
  const test = async () => {
    setTesting(true)
    setResult(null)
    try {
      const response = await desktopClient.testModelService(form.baseUrl, form.apiKey || undefined, form.apiType, model.modelId || undefined, form.id)
      if (!mounted.current) return
      let checkedIdentity = identity
      let selected = model.modelId
      if (!hasModel && response.models.length) {
        selected = response.models.find(id => id === preset?.modelPlaceholder) ?? response.models[0]
        const next = { ...form, models: [{ ...model, modelId: selected, isDefault: true }] }
        onChange(next)
        setModelText(selected)
        checkedIdentity = JSON.stringify(next)
      }
      setResult({ identity: checkedIdentity, ok: true, text: `连接正常 · ${response.latencyMs} ms${selected ? ` · 默认模型 ${selected}` : ''}` })
      window.dispatchEvent(new Event('fox:model-service-changed'))
    } catch (cause) {
      if (mounted.current) setResult({ identity, ok: false, text: developer ? configErrorText(cause) : '连接失败，请检查地址、模型和 Key。' })
    } finally {
      if (mounted.current) setTesting(false)
    }
  }

  return <form className="fox-service-form fox-provider-sheet-form" onSubmit={event => { event.preventDefault(); if (canSave && !busy && !testing) onSave() }}>
    <div ref={bodyRef} className="fox-settings-sheet-body">
      <fieldset className="fox-provider-edit-fields" disabled={busy || testing}>
        <section className="fox-settings-sheet-section">
          {form.id && <div className="fox-settings-sheet-section-head"><h3>连接信息</h3></div>}
          <div className="fox-model-fields">
            {preset?.catalog?.note && <p className="fox-settings-sheet-hint" data-provider-note><Info aria-hidden="true" />{preset.catalog.note}</p>}
            {fullForm && <>
              <label>服务名称<Input data-provider-initial-focus value={form.name} onChange={event => patch({ name: event.target.value })} /></label>
              <label>API 协议<select value={form.apiType} onChange={event => patch({ apiType: event.target.value as ServiceDraft['apiType'] })}>{primaryProtocols.map(api => <option key={api} value={api}>{API_NAMES[api]}</option>)}</select></label>
              <label>API 地址<Input type="url" value={form.baseUrl} aria-invalid={duplicate || undefined} placeholder="https://api.example.com/v1" onChange={event => patch({ baseUrl: event.target.value })} /></label>
            </>}
            <div className="fox-provider-key">
              <label>API Key<Input type={showKey ? 'text' : 'password'} autoComplete="new-password" value={form.apiKey} placeholder={credentialConfigured ? '已安全保存 · 留空保留' : '填写服务商提供的 Key'} onChange={event => patch({ apiKey: event.target.value, clearApiKey: false })} /></label>
              <Button type="button" variant="outline" size="icon" aria-label={showKey ? '隐藏 API Key' : '显示 API Key'} onClick={() => setShowKey(value => !value)}>{showKey ? <EyeOff /> : <Eye />}</Button>
            </div>
            {!fullForm && <div className="fox-provider-endpoint"><small>默认端点 · {API_NAMES[form.apiType]}</small><code>{form.baseUrl}</code></div>}
            {fullForm ? <label>模型 ID<Input value={modelText} placeholder="多个 ID 用逗号分隔" onChange={event => setModelIds(event.target.value)} /></label>
              : form.apiType !== 'openai-completions' && <label>默认模型<Input value={model.modelId} placeholder={preset?.modelPlaceholder ?? '填写模型 ID'} onChange={event => updateModels(form.models.map(item => item === model ? { ...item, modelId: event.target.value } : item))} /></label>}
            {duplicate && <p role="alert" className="fox-model-error">{DUPLICATE_SERVICE_URL_TEXT}</p>}
            {missingTarget && <p role="status" className="fox-settings-sheet-hint">{missingTarget}可在「更多端点」中配置。</p>}
          </div>
        </section>

        <div className="fox-provider-disclosures">
          <Collapsible className="fox-provider-disclosure">
            <CollapsibleTrigger className="fox-settings-disclosure">
              <span><b>更多端点</b></span>
              {Boolean(form.protocolProfiles?.length) && <small>{form.protocolProfiles?.length} 个端点</small>}
              <ChevronRight aria-hidden="true" />
            </CollapsibleTrigger>
            <CollapsibleContent forceMount className="fox-provider-disclosure-body">
              <ProviderProtocolEditor form={form} disabled={busy || testing} onChange={next => onChange({ ...form, ...next })} />
            </CollapsibleContent>
          </Collapsible>

          <Collapsible className="fox-provider-disclosure">
            <CollapsibleTrigger className="fox-settings-disclosure">
              <span><b>高级设置</b></span>
              <ChevronRight aria-hidden="true" />
            </CollapsibleTrigger>
            <CollapsibleContent className="fox-provider-disclosure-body">
              {!fullForm && <section className="fox-provider-detail-group fox-model-fields" aria-label="Fox 连接">
                <label>Fox 连接协议<select value={form.apiType} onChange={event => patch({ apiType: event.target.value as ServiceDraft['apiType'] })}>{primaryProtocols.map(api => <option key={api} value={api}>{API_NAMES[api]}</option>)}</select></label>
                <label>服务地址<Input type="url" value={form.baseUrl} onChange={event => patch({ baseUrl: event.target.value })} /></label>
              </section>}
              <section className="fox-provider-detail-group">
                <h3>使用此服务的 Agent</h3>
                <div className="fox-provider-agent-options">{(Object.keys(AGENT_NAMES) as AgentId[]).map(agent => <Button key={agent} type="button" variant="outline" aria-pressed={targets.includes(agent)} title={agent !== 'fox' && !enabled.includes(agent) ? '未启用' : undefined}
                  onClick={() => onTargets(targets.includes(agent) ? targets.filter(item => item !== agent) : [...targets, agent])}>{AGENT_NAMES[agent]}</Button>)}</div>
              </section>
              <section className="fox-provider-detail-group">
                <div className="fox-provider-detail-head">
                  <h3>模型参数</h3>
                  <Button type="button" variant="outline" size="sm" onClick={() => updateModels([...form.models, { ...model, id: undefined, modelId: '', displayName: '', isDefault: false }])}><Plus />添加模型</Button>
                </div>
                {form.models.map((m, index) => <div className="fox-provider-detail-card" role="group" aria-label={`模型 ${index + 1}`} key={index}>
                  <div className="fox-provider-detail-head">
                    <h4>模型 {index + 1}</h4>
                    {m.isDefault ? <small>默认模型</small>
                      : <Button type="button" variant="ghost" size="sm" onClick={() => patch({ models: form.models.map((item, at) => ({ ...item, isDefault: at === index })) })}>设为默认</Button>}
                  </div>
                  <div className="fox-provider-model-grid">
                    {(['modelId', 'displayName', 'contextWindow', 'maxOutputTokens'] as const).map((key, i) => <label key={key}>{['模型 ID', '显示名称', '上下文长度', '最大输出'][i]}<Input value={m[key]} type={i > 1 ? 'number' : 'text'} min={i > 1 ? 1 : undefined} onChange={e => updateModels(form.models.map((item, at) => at === index ? { ...item, [key]: i > 1 ? Number(e.target.value) : e.target.value } : item))} /></label>)}
                  </div>
                  <div className="fox-provider-detail-head fox-provider-model-actions">
                    <label className="fox-provider-model-switch"><Switch checked={m.supportsImageInput} onCheckedChange={value => patch({ models: form.models.map((item, at) => at === index ? { ...item, supportsImageInput: value } : item) })} />图片输入</label>
                    {form.models.length > 1 && <Button type="button" variant="ghost" size="sm" className="fox-provider-remove" onClick={() => { const models = form.models.filter((_, at) => at !== index); if (m.isDefault) models[0] = { ...models[0], isDefault: true }; updateModels(models) }}>删除模型</Button>}
                  </div>
                </div>)}
              </section>
              <section className="fox-provider-detail-group fox-model-fields">
                <h3>服务偏好</h3>
                <label>供应商图标<select value={form.icon ?? ''} onChange={e => patch({ icon: e.target.value || null })}><option value="">自动识别</option>{providerIconOptions.map(p => <option key={p.icon} value={p.icon}>{p.name}</option>)}</select></label>
                <label className="fox-settings-switch-row"><span>设为默认服务</span><Switch checked={form.isDefault} onCheckedChange={isDefault => patch({ isDefault })} /></label>
                {form.id && credentialConfigured && <label className="fox-settings-switch-row"><span>保存时清除已存 API Key</span><Switch checked={form.clearApiKey} onCheckedChange={clearApiKey => patch({ clearApiKey, apiKey: clearApiKey ? '' : form.apiKey })} /></label>}
              </section>
            </CollapsibleContent>
          </Collapsible>
        </div>
      </fieldset>
    </div>
    <div className="fox-settings-sheet-foot fox-provider-form-foot">
      {error && <p role="alert" className="fox-model-error">{error}</p>}
      {status && <p role="status" className={status.ok ? 'fox-provider-test-ok' : 'fox-model-error'}>{status.text}</p>}
      <div className="fox-model-actions">
        {onBack && <Button type="button" variant="ghost" disabled={busy || testing} onClick={onBack}><ArrowLeft />供应商列表</Button>}
        {onDelete && <Button type="button" variant="destructive" disabled={busy || testing} onClick={onDelete}>删除服务</Button>}
        <span className="is-grow" />
        <Button type="button" variant="outline" disabled={busy || testing} onClick={onCancel}>取消</Button>
        <Button type="button" variant="outline" disabled={!canTest || busy || testing} onClick={() => void test()}>{testing ? <LoaderCircle className="animate-spin" /> : <Zap />}{testing ? '检查中…' : '检查连接'}</Button>
        <Button type="submit" disabled={!canSave || busy || testing}>{busy ? '保存中…' : form.id ? '保存' : '添加'}</Button>
      </div>
    </div>
  </form>
}
