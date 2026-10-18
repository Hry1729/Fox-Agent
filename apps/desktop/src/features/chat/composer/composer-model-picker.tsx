import { useEffect, useRef } from 'react'
import { Models } from '@/components/assistant-ui/elements/composer'
import type { ModelServiceRecord } from '@/features/conversations/model/types'
import { useModelProviders } from '@/features/settings/use-model-providers'
import { useModelService } from '@/features/settings/use-model-service'
import { notify as toast } from '@/features/notifications'

export function ComposerModelPicker({ service, disabled, onOpenSettings }: {
  service?: ModelServiceRecord | null
  disabled: boolean
  onOpenSettings?: () => void
}) {
  const providers = useModelProviders()
  const modelService = useModelService()
  const saving = useRef(false)
  useEffect(() => {
    const refresh = () => { void providers.refresh() }
    window.addEventListener('fox:model-service-changed', refresh)
    return () => window.removeEventListener('fox:model-service-changed', refresh)
  }, [providers.refresh])
  const choices = providers.items.filter(provider => provider.enabled).flatMap(provider => provider.models.map(model => ({
    id: `${provider.id}:${model.id}`, provider, model,
  })))
  const selected = choices.find(choice => choice.provider.baseUrl === service?.baseUrl && choice.provider.apiType === service?.apiType && choice.model.modelId === service?.modelId)
  const options = choices.map(choice => ({ id: choice.id, name: choice.model.modelId, meta: choice.provider.name }))
  if (service?.modelId && !selected) options.unshift({ id: 'current-service', name: service.modelId, meta: service.name })

  const select = async (id: string) => {
    if (disabled || saving.current || id === selected?.id || id === 'current-service') return
    const choice = choices.find(item => item.id === id)
    if (!choice) return
    saving.current = true
    try {
      const saved = await modelService.save({
        name: choice.provider.name,
        baseUrl: choice.provider.baseUrl,
        apiType: choice.provider.apiType,
        modelId: choice.model.modelId,
        contextWindow: choice.model.contextWindow,
        maxOutputTokens: choice.model.maxOutputTokens,
        supportsImageInput: choice.model.supportsImageInput,
      })
      if (!saved) toast.error('模型切换失败，请重试')
    } finally { saving.current = false }
  }
  return <Models value={selected?.id ?? (service?.modelId ? 'current-service' : undefined)} options={options} fallbackLabel={service?.modelId || '选择模型'} disabled={disabled || modelService.saving} onValueChange={id => void select(id)} note={providers.error ? '模型列表读取失败，可前往设置重试。' : '切换默认模型，后续对话使用。'} onManage={onOpenSettings} />
}
