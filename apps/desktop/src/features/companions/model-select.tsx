import { useEffect, useState } from 'react'
import { CircleAlert, Cpu, Globe, Image, Type } from 'lucide-react'
import { Select, SelectContent, SelectGroup, SelectItem, SelectLabel, SelectTrigger, SelectValue } from '@/components/ui/select'
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

export function ContactModelSelect({ contact, inSideCard = false }: { contact: Companion; inSideCard?: boolean }) {
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
  const globalModelName = ctx?.model || '未配置模型'
  const selectedModel = providers?.find(provider => provider.id === contact.modelProviderId)
    ?.models.find(model => model.id === contact.modelRef)
  const modelName = unavailable ? '独立模型不可用' : value ? selectedModel?.displayName ?? '正在读取模型…' : globalModelName
  return <div className={inSideCard ? 'fox-contact-model-select is-side-card' : 'fox-contact-model-select'}>
    <Select value={unavailable ? '__unavailable__' : value || '__global__'} disabled={busy || !providers} onValueChange={next => void change(next)}>
      <SelectTrigger size="sm" className={inSideCard ? 'fox-contact-model-trigger fox-contact-side-item' : 'fox-contact-model-trigger'} aria-label="联系人模型" title={modelName}>
        {inSideCard && <Cpu size={16} aria-hidden="true" />}
        <SelectValue>
          <span className="fox-contact-model-value">
            <span className="fox-contact-model-name">{modelName}</span>
            {!value && <small className="fox-contact-model-note">跟随全局</small>}
          </span>
        </SelectValue>
      </SelectTrigger>
      <SelectContent align="end" collisionPadding={8} className="fox-contact-side-select-menu">
        <SelectItem value="__global__" textValue={globalModelName}>
          <Globe aria-hidden="true" /><span className="fox-contact-model-name">{globalModelName}</span>
          <small className="fox-contact-model-note">跟随全局</small>
        </SelectItem>
        {unavailable && <SelectItem value="__unavailable__" disabled><CircleAlert aria-hidden="true" />独立模型不可用</SelectItem>}
        {(providers ?? []).map(provider => <SelectGroup key={provider.id}>
          <SelectLabel>{provider.enabled ? provider.name : `${provider.name}（已停用）`}</SelectLabel>
          {provider.models.map(model => <SelectItem key={model.id} value={`${provider.id}:${model.id}`} textValue={model.displayName} disabled={!provider.enabled}>
            {model.supportsImageInput ? <Image aria-hidden="true" /> : <Type aria-hidden="true" />}
            <span className="fox-contact-model-name">{model.displayName}</span>
            <small className="fox-contact-model-note">{model.supportsImageInput ? '能看图' : '不能看图'}</small>
          </SelectItem>)}
        </SelectGroup>)}
      </SelectContent>
    </Select>
    {unavailable && <small className="fox-contact-model-error" role="alert">{unavailable}</small>}
  </div>
}
