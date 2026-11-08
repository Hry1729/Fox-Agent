import { useEffect, useState } from 'react'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { ModelProviderRecord } from '@/features/conversations/model/types'
import { contacts, type Companion } from './api'
import { showContactError, useContacts } from './context'

/** Why a saved override cannot be used right now. null = usable or following
 * the global model. The send path refuses with the same reasons; nothing here
 * silently falls back to another model. */
export function overrideUnavailableReason(contact: Companion, providers: ModelProviderRecord[]): string | null {
  if (!contact.modelProviderId || !contact.modelRef) return null
  const provider = providers.find(item => item.id === contact.modelProviderId)
  if (!provider) return '独立模型服务已被删除，请重新选择'
  if (!provider.enabled) return `模型服务「${provider.name}」已停用`
  if (!provider.models.some(model => model.id === contact.modelRef)) return '所选模型已从该服务移除，请重新选择'
  return null
}

export function ContactModelSelect({ contact }: { contact: Companion }) {
  const ctx = useContacts()
  const [providers, setProviders] = useState<ModelProviderRecord[] | null>(null)
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    let cancelled = false
    desktopClient.listModelProviders()
      .then(list => { if (!cancelled) setProviders(list) })
      .catch(error => { if (!cancelled) { setProviders([]); showContactError(error) } })
    return () => { cancelled = true }
  }, [contact.id])
  const unavailable = providers ? overrideUnavailableReason(contact, providers) : null
  const value = contact.modelProviderId && contact.modelRef ? `${contact.modelProviderId}:${contact.modelRef}` : ''
  const change = async (next: string) => {
    const separator = next.indexOf(':')
    const providerId = separator === -1 ? null : next.slice(0, separator)
    const modelRef = separator === -1 ? null : next.slice(separator + 1)
    setBusy(true)
    try {
      await contacts.setModel(contact.id, providerId, modelRef)
      await ctx?.refresh()
    } catch (error) { showContactError(error) } finally { setBusy(false) }
  }
  const description = unavailable
    ?? (value ? '只影响下一轮对话，正在进行的任务继续用原来的模型' : `现在是 ${ctx?.model ?? '尚未配置'}，全局换了它也跟着换`)
  return <div className="fox-contact-model-select">
    <select
      aria-label="联系人模型"
      value={unavailable ? '__unavailable__' : value}
      disabled={busy || !providers}
      onChange={event => void change(event.target.value)}
    >
      <option value="">跟随全局{ctx?.model ? ` · ${ctx.model}` : ''}</option>
      {unavailable && <option value="__unavailable__" disabled>独立模型不可用</option>}
      {(providers ?? []).map(provider =>
        <optgroup key={provider.id} label={provider.enabled ? provider.name : `${provider.name}（已停用）`} disabled={!provider.enabled}>
          {provider.models.map(model => <option key={`${provider.id}:${model.id}`} value={`${provider.id}:${model.id}`}>
            {model.displayName} · {model.supportsImageInput ? '能看图' : '不能看图'}
          </option>)}
        </optgroup>)}
    </select>
    <small className={unavailable ? 'fox-contact-model-error' : undefined} role={unavailable ? 'alert' : undefined}>{description}</small>
  </div>
}