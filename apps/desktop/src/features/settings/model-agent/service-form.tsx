import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import { useDeveloperUi } from '@/features/chat/developer-ui'
import { SettingsGroup } from '../settings-layout'
import { ProviderProtocolEditor } from '../provider-protocol-editor'
import { DUPLICATE_SERVICE_URL_TEXT, findDuplicateServiceUrl } from '../provider-errors'
import { servicePresets, presetDraft, providerIconOptions, type ServiceDraft } from './service-presets'
import { AGENT_NAMES, API_NAMES, configErrorText } from './agent-config-copy'
import type { AgentId, ApiType } from './agent-config-types'

export function ServiceForm({ form, onChange, targets, onTargets, enabled, busy, credentialConfigured, existingServices, error, onSave, onCancel, onDelete }: {
  form: ServiceDraft; onChange: (value: ServiceDraft) => void; targets: AgentId[]; onTargets: (value: AgentId[]) => void
  enabled: AgentId[]; busy: boolean; credentialConfigured: boolean; existingServices: { id: string; baseUrl: string }[]
  error: string | null; onSave: () => void; onCancel: () => void; onDelete?: () => void
}) {
  const developer = useDeveloperUi()
  const [preset, setPreset] = useState(() => servicePresets.find(p => p.baseUrl === form.baseUrl)?.id ?? 'custom')
  const [step, setStep] = useState(form.id ? 2 : 1)
  const [testing, setTesting] = useState(false)
  const [result, setResult] = useState<{ identity: string; rows: string[] } | null>(null)
  const identity = JSON.stringify(form)
  const model = form.models.find(m => m.isDefault) ?? form.models[0]
  const patch = (values: Partial<ServiceDraft>) => onChange({ ...form, ...values })
  const duplicate = findDuplicateServiceUrl(existingServices, form.id, form.baseUrl) !== null
  const missing = (api: ApiType) => api !== form.apiType && !(form.protocolProfiles ?? []).some(p => p.apiType === api && p.baseUrl.trim())
  const requiresResponses = targets.includes('codex') && missing('openai-responses')
  const requiresMessages = targets.includes('deepseek') && missing('anthropic-messages')
  const valid = Boolean(form.name.trim() && form.baseUrl.trim() && model?.modelId.trim() && !duplicate && !requiresResponses && !requiresMessages)
  const choosePreset = (id: string) => {
    setPreset(id)
    const found = servicePresets.find(p => p.id === id)
    if (found) onChange({ ...presetDraft(found, form.isDefault), id: form.id, apiKey: form.apiKey, models: form.models.some(m => m.modelId.trim()) ? form.models : presetDraft(found, form.isDefault).models })
  }
  const extraAddress = (api: ApiType, value: string) => {
    const profiles = form.protocolProfiles ?? []
    const found = profiles.find(p => p.apiType === api)
    patch({ protocolProfiles: found ? profiles.map(p => p.id === found.id ? { ...p, baseUrl: value } : p) : [...profiles, { id: crypto.randomUUID(), apiType: api, baseUrl: value, modelId: '', separateCredential: false }] })
  }
  const test = async () => {
    setTesting(true)
    const rows: string[] = []
    const captured = identity
    const endpoints = [{ id: 'primary', apiType: form.apiType as ApiType, baseUrl: form.baseUrl, modelId: model.modelId, separateCredential: false }, ...(form.protocolProfiles ?? []).filter(p => p.id !== 'primary')]
    for (const endpoint of endpoints) {
      try {
        const key = endpoint.separateCredential ? form.protocolKeys?.[endpoint.id] : form.apiKey
        const value = await desktopClient.testModelService(endpoint.baseUrl, key || undefined, endpoint.apiType, endpoint.modelId || model.modelId, form.id, endpoint.separateCredential ? endpoint.id : undefined)
        rows.push(`${API_NAMES[endpoint.apiType]}：连接正常 · ${value.latencyMs} ms`)
      } catch (cause) {
        rows.push(`${API_NAMES[endpoint.apiType]}：${developer ? configErrorText(cause) : '连接失败，请检查地址、模型和 Key。'}`)
      }
      setResult({ identity: captured, rows: [...rows] })
    }
    setTesting(false)
    window.dispatchEvent(new Event('fox:model-service-changed'))
  }
  const presets = [...servicePresets].sort((a, z) => {
    const coverage = (id: string) => targets.filter(t => t !== 'codex' && t !== 'deepseek' || id === 'deepseek' || t === 'codex' && id === 'openai').length
    return coverage(z.id) - coverage(a.id)
  })
  return <form className="fox-service-form" onSubmit={event => { event.preventDefault(); if (step === 2 && valid && !busy && !testing) onSave() }}>
    <p className="fox-model-breadcrumb">模型服务 / {form.id ? '编辑服务' : '添加服务'}</p>
    <SettingsGroup title="1. 选择目标和预设" aside={step === 2 && <Button type="button" variant="ghost" size="sm" onClick={() => setStep(1)}>更改</Button>}>
      {step === 1 ? <div className="fox-model-section">
        <p>这个服务给谁用</p>
        <div className="fox-target-grid">{(Object.keys(AGENT_NAMES) as AgentId[]).map(agent => <button type="button" className="fox-target-option" aria-pressed={targets.includes(agent)} key={agent} onClick={() => onTargets(targets.includes(agent) ? targets.filter(t => t !== agent) : [...targets, agent])}><b>{AGENT_NAMES[agent]}</b><small>{agent !== 'fox' && !enabled.includes(agent) ? '未启用' : '模型服务'}</small></button>)}</div>
        <p>从预设开始</p>
        <div className="fox-preset-grid">{presets.map(p => <button key={p.id} type="button" aria-pressed={preset === p.id} onClick={() => choosePreset(p.id)}><img src={`/providers/${p.icon}`} alt="" /><span>{p.name}{targets.includes('deepseek') && p.id !== 'deepseek' && <small>需要补充 Messages 地址</small>}{targets.includes('codex') && !['deepseek', 'openai'].includes(p.id) && <small>需要补充 Responses 地址</small>}</span></button>)}<button type="button" aria-pressed={preset === 'custom'} onClick={() => choosePreset('custom')}>自定义</button></div>
        <div className="fox-model-actions"><Button type="button" variant="outline" onClick={onCancel}>取消</Button><Button type="button" disabled={!targets.length} onClick={() => setStep(2)}>下一步</Button></div>
      </div> : <p className="fox-model-section">目标：{targets.map(a => AGENT_NAMES[a]).join('、')} · 预设：{servicePresets.find(p => p.id === preset)?.name ?? '自定义'}</p>}
    </SettingsGroup>
    {step === 2 && <>
      <SettingsGroup title="2. 连接信息"><div className="fox-model-section fox-model-fields">
        <label>服务名称<Input value={form.name} onChange={e => patch({ name: e.target.value })} /></label>
        <label>API Key<Input type="password" autoComplete="new-password" value={form.apiKey} placeholder={credentialConfigured ? '已安全保存 · 留空保持不变' : '填写服务商提供的 Key'} onChange={e => patch({ apiKey: e.target.value, clearApiKey: false })} /><small>Key 保存在系统凭据库。Agent 使用时由 Fox 写入私有配置，不会改写你自己的登录配置。</small></label>
        <label>默认模型<Input value={model.modelId} placeholder={servicePresets.find(p => p.id === preset)?.modelPlaceholder ?? '填写服务商提供的模型 ID'} onChange={e => patch({ models: form.models.map(m => m === model ? { ...m, modelId: e.target.value } : m) })} /></label>
        {preset === 'custom' && <label>服务地址<Input value={form.baseUrl} aria-invalid={duplicate || undefined} onChange={e => patch({ baseUrl: e.target.value })} /></label>}
        {duplicate && <p role="alert" className="fox-model-error">{DUPLICATE_SERVICE_URL_TEXT}</p>}
        {targets.includes('codex') && !['openai', 'deepseek'].includes(preset) && <label>Responses 地址<Input value={form.protocolProfiles?.find(p => p.apiType === 'openai-responses')?.baseUrl ?? ''} onChange={e => extraAddress('openai-responses', e.target.value)} /><small>Codex 需要 OpenAI Responses 地址。</small></label>}
        {targets.includes('deepseek') && preset !== 'deepseek' && form.apiType !== 'anthropic-messages' && <label>Messages 地址<Input value={form.protocolProfiles?.find(p => p.apiType === 'anthropic-messages')?.baseUrl ?? ''} onChange={e => extraAddress('anthropic-messages', e.target.value)} /><small>DeepSeek（dsh）需要 Anthropic Messages 地址。</small></label>}
        <div><Button type="button" variant="outline" disabled={!valid || testing || busy} onClick={() => void test()}>{testing ? '正在测试…' : '测试连接'}</Button><div role="status">{result?.identity === identity && result.rows.map((row, i) => <p key={i}>{row}</p>)}</div></div>
      </div></SettingsGroup>
      <details className="fox-model-advanced"><summary>高级设置 <span>{API_NAMES[form.apiType]} · 上下文 {Math.round(model.contextWindow / 1000)}K · 最大输出 {Math.round(model.maxOutputTokens / 1000)}K · {(form.protocolProfiles ?? []).filter(p => p.id !== 'primary').length + 1} 个入口</span></summary>
        <div className="fox-model-fields">
          <label>Fox 连接协议<select value={form.apiType} onChange={e => patch({ apiType: e.target.value as ServiceDraft['apiType'] })}><option value="openai-completions">Chat Completions</option><option value="anthropic-messages">Anthropic Messages</option></select></label>
          <label>服务地址<Input value={form.baseUrl} onChange={e => patch({ baseUrl: e.target.value })} /></label>
          <ProviderProtocolEditor form={form} disabled={busy || testing} onChange={next => onChange({ ...form, ...next })} />
          <h3>模型</h3>
          {form.models.map((m, index) => <fieldset className="fox-model-edit-row" key={index}><legend>模型 {index + 1}</legend>
            {(['modelId', 'displayName', 'contextWindow', 'maxOutputTokens'] as const).map((key, i) => <label key={key}>{['模型 ID', '显示名称', '上下文长度', '最大输出'][i]}<Input value={m[key]} type={i > 1 ? 'number' : 'text'} min={i > 1 ? 1 : undefined} onChange={e => patch({ models: form.models.map((item, at) => at === index ? { ...item, [key]: i > 1 ? Number(e.target.value) : e.target.value } : item) })} /></label>)}
            <label><Switch checked={m.supportsImageInput} onCheckedChange={value => patch({ models: form.models.map((item, at) => at === index ? { ...item, supportsImageInput: value } : item) })} />图片输入</label>
            <Button type="button" variant="ghost" disabled={m.isDefault} onClick={() => patch({ models: form.models.map((item, at) => ({ ...item, isDefault: at === index })) })}>{m.isDefault ? '默认模型' : '设为默认'}</Button>
            <Button type="button" variant="ghost" disabled={form.models.length === 1} onClick={() => { const models = form.models.filter((_, at) => at !== index); if (m.isDefault) models[0] = { ...models[0], isDefault: true }; patch({ models }) }}>删除模型</Button>
          </fieldset>)}
          <Button type="button" variant="outline" onClick={() => patch({ models: [...form.models, { modelId: '', displayName: '', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false, isDefault: false }] })}>添加模型</Button>
          <label>供应商图标<select value={form.icon ?? ''} onChange={e => patch({ icon: e.target.value || null })}><option value="">自动识别</option>{providerIconOptions.map(p => <option key={p.icon} value={p.icon}>{p.name}</option>)}</select></label>
          <label><Switch checked={form.isDefault} onCheckedChange={isDefault => patch({ isDefault })} />设为默认服务</label>
          {form.id && credentialConfigured && <label><Switch checked={form.clearApiKey} onCheckedChange={clearApiKey => patch({ clearApiKey, apiKey: clearApiKey ? '' : form.apiKey })} />保存时清除已存 API Key</label>}
        </div>
      </details>
      {error && <p role="alert" className="fox-model-error">{error}</p>}
      <div className="fox-model-actions">{onDelete && <Button type="button" variant="ghost" disabled={busy} onClick={onDelete}>删除服务</Button>}<span className="is-grow" /><Button type="button" variant="outline" onClick={onCancel}>取消</Button><Button type="submit" disabled={!valid || busy || testing}>{busy ? '保存中…' : '保存'}</Button></div>
    </>}
  </form>
}
