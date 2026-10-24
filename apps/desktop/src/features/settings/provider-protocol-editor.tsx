import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Switch } from '@/components/ui/switch'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { ProtocolProfile } from '@/features/conversations/model/types'
import { isAgentBackendHidden } from '@/features/conversations/model/agent-backend'
import type { SaveModelProviderInput } from './use-model-providers'

// Which Agents can use each protocol; hidden backends (`HIDDEN_AGENT_BACKENDS`) are left out.
const allProtocolAgents: Record<ProtocolProfile['apiType'], string[]> = {
  'openai-completions': ['pi'],
  'anthropic-messages': ['pi', 'deepseek'],
  'openai-responses': ['pi', 'codex'],
}
export const protocolAgents = Object.fromEntries(
  Object.entries(allProtocolAgents).map(([apiType, agents]) => [apiType, agents.filter(agent => !isAgentBackendHidden(agent))]),
) as Record<ProtocolProfile['apiType'], string[]>
const names: Record<string, string> = { pi: 'pi', deepseek: 'DeepSeek', codex: 'Codex' }
const labels = { 'openai-completions': 'OpenAI Chat Completions', 'anthropic-messages': 'Anthropic Messages', 'openai-responses': 'OpenAI Responses' }

export function ProviderProtocolEditor({ form, disabled, onChange }: {
  form: SaveModelProviderInput
  disabled: boolean
  onChange: (form: SaveModelProviderInput) => void
}) {
  const [testing, setTesting] = useState<string | null>(null)
  const [results, setResults] = useState<Record<string, { identity: string; text: string }>>({})
  const profiles = form.protocolProfiles ?? []
  const identity = (profile: ProtocolProfile) => JSON.stringify([profile, form.apiKey, form.clearApiKey, form.baseUrl, form.protocolKeys?.[profile.id]])
  const update = (id: string, patch: Partial<ProtocolProfile>) => onChange({ ...form, protocolProfiles: profiles.map(p => p.id === id ? { ...p, ...patch } : p) })
  const test = async (profile: ProtocolProfile) => {
    const captured = identity(profile)
    setTesting(profile.id)
    try {
      const key = profile.separateCredential ? form.protocolKeys?.[profile.id] : form.apiKey
      const result = await desktopClient.testModelService(profile.baseUrl, key || undefined, profile.apiType, profile.modelId || form.models.find(m => m.isDefault)?.modelId || form.models[0]?.modelId || '', form.id, profile.separateCredential ? profile.id : undefined)
      setResults(previous => ({ ...previous, [profile.id]: { identity: captured, text: `连接验证通过 · ${result.latencyMs} ms。尚未验证 Agent 的流式输出和工具调用。` } }))
    } catch (error) {
      setResults(previous => ({ ...previous, [profile.id]: { identity: captured, text: `连接验证失败：${error instanceof Error ? error.message : String(error)}` } }))
    } finally { setTesting(null) }
  }
  return <section className="fox-protocol-editor" aria-label="外部 Agent 协议配置">
    <div className="fox-provider-models-head">
      <div><h3>外部 Agent 协议</h3><p>按需添加协议。默认复用供应商 Key，地址和模型分别配置。</p></div>
      <Button type="button" variant="outline" size="sm" disabled={disabled} onClick={() => onChange({ ...form, protocolProfiles: [...profiles, { id: crypto.randomUUID(), apiType: 'openai-responses', baseUrl: '', modelId: '', separateCredential: false, backends: [] }] })}>添加协议</Button>
    </div>
    <p>Fox 使用上方连接。各 Agent 自动使用当前供应商中匹配的入口；同一协议有多个地址时，首次使用会提示选择。</p>
    {profiles.map((profile, index) => <fieldset key={profile.id} disabled={disabled || testing !== null} className="fox-protocol-fields">
      <legend>协议配置 {index + 1}</legend>
      <div className="fox-provider-form-grid">
        <label>API 协议<Select value={profile.apiType} onValueChange={(value) => {
          const apiType = value as ProtocolProfile['apiType']
          update(profile.id, { apiType, backends: profile.backends.filter(b => allProtocolAgents[apiType].includes(b)) })
        }}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent>{Object.entries(labels).map(([value, label]) => <SelectItem key={value} value={value}>{label} · {protocolAgents[value as ProtocolProfile['apiType']].map(b => names[b]).join(' / ')}</SelectItem>)}</SelectContent></Select></label>
        <label>模型 ID<Input value={profile.modelId} placeholder="留空使用供应商默认模型" onChange={event => update(profile.id, { modelId: event.target.value })} /></label>
        <label className="is-wide">API 地址<Input value={profile.baseUrl} placeholder="填写供应商提供的此协议地址" onChange={event => update(profile.id, { baseUrl: event.target.value })} /></label>
      </div>
      <p>协议适用于：{protocolAgents[profile.apiType].map(b => names[b]).join('、')}。协议匹配不代表连接已验证。</p>
      <div className="fox-protocol-assignments">{protocolAgents[profile.apiType].map(backend => <label key={backend}>
        <Switch checked={profile.backends.includes(backend)} onCheckedChange={checked => onChange({ ...form, protocolProfiles: profiles.map(p => ({ ...p, backends: p.id === profile.id && checked ? [...p.backends.filter(b => b !== backend), backend] : p.backends.filter(b => b !== backend) })) })} />用作 {names[backend]} 默认协议
      </label>)}</div>
      <label className="fox-protocol-key"><Switch checked={profile.separateCredential} onCheckedChange={separateCredential => {
        const protocolKeys = { ...form.protocolKeys }; delete protocolKeys[profile.id]
        onChange({ ...form, protocolKeys, protocolProfiles: profiles.map(p => p.id === profile.id ? { ...p, separateCredential, credentialConfigured: false } : p) })
      }} />使用独立 Key</label>
      {profile.separateCredential && <label>此协议的 API Key<Input type="password" autoComplete="new-password" value={form.protocolKeys?.[profile.id] ?? ''} placeholder={profile.credentialConfigured ? '已保存，留空保留' : '输入此协议的 Key'} onChange={event => {
        const protocolKeys = { ...form.protocolKeys }
        if (event.target.value) protocolKeys[profile.id] = event.target.value; else delete protocolKeys[profile.id]
        onChange({ ...form, protocolKeys })
      }} /></label>}
      <div className="fox-protocol-actions">
        <Button type="button" variant="outline" size="sm" disabled={!profile.baseUrl.trim() || !(profile.modelId.trim() || form.models.some(m => m.modelId.trim())) || (profile.separateCredential && !profile.credentialConfigured && !form.protocolKeys?.[profile.id])} onClick={() => void test(profile)}>{testing === profile.id ? '正在验证…' : '验证此协议'}</Button>
        <Button type="button" variant="ghost" size="sm" onClick={() => {
          const protocolKeys = { ...form.protocolKeys }; delete protocolKeys[profile.id]
          onChange({ ...form, protocolKeys, protocolProfiles: profiles.filter(p => p.id !== profile.id) })
        }}>删除协议</Button>
      </div>
      <p role="status">{results[profile.id]?.identity === identity(profile) ? results[profile.id].text : '此配置尚未验证'}</p>
    </fieldset>)}
  </section>
}
