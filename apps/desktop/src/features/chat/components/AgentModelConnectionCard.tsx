import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { AgentBackendTarget } from '@/features/conversations/model/agent-backend-onboarding'

export interface AgentModelConnection {
  status: 'existing_login' | 'ready' | 'missing_provider' | 'missing_address' | 'choose_address' | 'missing_key' | 'verification_failed'
  providerId: string | null
  providerName: string | null
  apiType: string
  candidates: Array<{ id: string; baseUrl: string; modelId: string }>
  message: string | null
}
export interface AgentModelConnectionRequest {
  target: AgentBackendTarget
  providerId?: string
  profileId?: string
  baseUrl?: string
}

export function AgentModelConnectionCard({ backend, connection, onSave }: {
  backend: string
  connection: AgentModelConnection
  onSave: (choice: { baseUrl?: string; profileId?: string }) => Promise<void>
}) {
  const [address, setAddress] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const save = async (choice: { baseUrl?: string; profileId?: string }) => {
    setBusy(true); setError(null)
    try { await onSave(choice) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setBusy(false) }
  }
  const agent = backend === 'deepseek' ? 'dsh' : backend === 'codex' ? 'Codex' : 'pi'
  const api = connection.apiType === 'openai-responses' ? 'Responses' : connection.apiType === 'anthropic-messages' ? 'Anthropic Messages' : 'Chat Completions'
  if (connection.status === 'ready' || connection.status === 'existing_login') return null
  return <section className="fox-agent-model-card" aria-label={`${agent} 模型连接`}>
    {connection.status === 'missing_address' && <form onSubmit={event => { event.preventDefault(); void save({ baseUrl: address.trim() }) }}>
      <strong>{agent} 需要 {api} 地址</strong>
      <p>保存到「{connection.providerName}」，复用供应商的 API Key 和默认模型。</p>
      <label>API 地址<Input type="url" required value={address} disabled={busy} placeholder="https://…" onChange={event => setAddress(event.target.value)} /></label>
      <Button type="submit" size="sm" disabled={busy || !address.trim()}>{busy ? '正在验证…' : '保存并验证'}</Button>
    </form>}
    {connection.status === 'choose_address' && <><strong>为 {agent} 选择地址</strong><p>「{connection.providerName}」有多个匹配地址。选择后会记住。</p>{connection.candidates.map(candidate => <Button key={candidate.id} variant="outline" size="sm" disabled={busy} onClick={() => void save({ profileId: candidate.id })}>{candidate.baseUrl} · {candidate.modelId}</Button>)}</>}
    {connection.status === 'missing_provider' && <p>请先在设置中选择模型供应商。</p>}
    {connection.status === 'missing_key' && <p>请在「{connection.providerName}」中填写 API Key。</p>}
    {connection.status === 'verification_failed' && <><strong>连接尚未通过验证</strong><p>{connection.message}</p>{connection.providerName && <p>可在「{connection.providerName}」的高级设置中修改地址。</p>}<Button size="sm" variant="outline" disabled={busy} onClick={() => void save({})}>{busy ? '正在验证…' : '重试'}</Button></>}
    {error && <p role="alert">{error}</p>}
  </section>
}
