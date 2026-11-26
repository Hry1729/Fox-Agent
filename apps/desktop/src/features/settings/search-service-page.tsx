import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { ChevronRight, Globe2, LoaderCircle, RotateCcw } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { notify as toast } from '@/features/notifications'
import { cn } from '@/lib/utils'

import { searchProviders as services } from './search-providers'

export type SearchProvider = (typeof services)[number]['id']
export type SearchView = { provider: SearchProvider; searxngUrl: string; configuredKeys: SearchProvider[] }

type SearchSheetProps = {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSaved: (value: SearchView) => void
  initialProvider?: SearchProvider
}

export function searchProviderForPlugin(pluginId: string): SearchProvider | undefined {
  return services.find(service => `search:${service.id}` === pluginId)?.id
}

export function SearchServiceSheet({ open, onOpenChange, onSaved, initialProvider }: SearchSheetProps) {
  const [saved, setSaved] = useState<SearchView | null>(null)
  const [provider, setProvider] = useState<SearchProvider>('auto')
  const [url, setUrl] = useState('')
  const [keyDrafts, setKeyDrafts] = useState<Partial<Record<SearchProvider, string>>>({})
  const [busy, setBusy] = useState(false)
  const [loading, setLoading] = useState(false)
  const [message, setMessage] = useState('')
  const [loadAttempt, setLoadAttempt] = useState(0)

  useEffect(() => {
    if (!open) return
    let active = true
    setLoading(true)
    setSaved(null)
    setMessage('')
    setKeyDrafts({})
    invoke<SearchView>('search_service_get').then(value => {
      if (!active) return
      setSaved(value)
      setProvider(initialProvider ?? value.provider)
      setUrl(value.searxngUrl)
    }).catch(error => {
      if (active) setMessage(`无法读取搜索服务配置：${String(error)}`)
    }).finally(() => {
      if (active) setLoading(false)
    })
    return () => { active = false }
  }, [open, loadAttempt, initialProvider])

  async function save() {
    if (!saved || busy) return
    setBusy(true)
    setMessage('')
    try {
      const value = await invoke<SearchView>('search_service_save', {
        request: { provider, apiKey: keyDrafts[provider]?.trim() || null, searxngUrl: provider === 'searxng' ? url : null },
      })
      setSaved(value)
      setKeyDrafts(drafts => ({ ...drafts, [provider]: '' }))
      onSaved(value)
      toast.success('搜索服务已保存，下次 Fox 搜索时生效。')
      onOpenChange(false)
    } catch (error) {
      setMessage(String(error))
    } finally {
      setBusy(false)
    }
  }

  const currentKey = keyDrafts[provider] ?? ''
  const hasSavedKey = saved?.configuredKeys.includes(provider) ?? false
  return <Sheet open={open} onOpenChange={(nextOpen) => { if (!busy) onOpenChange(nextOpen) }}>
    <SheetContent side="right" className="fox-settings-sheet fox-settings-search-sheet" aria-label="网页搜索设置">
      <SheetHeader className="fox-settings-sheet-head">
        <span className="fox-settings-provider-mark is-large"><Globe2 /></span>
        <div className="fox-settings-sheet-title">
          <SheetTitle><span>网页搜索</span></SheetTitle>
          <SheetDescription>用于 Fox 通用助手和专家。外部 Agent 使用各自的搜索能力。</SheetDescription>
        </div>
      </SheetHeader>
      <form id="fox-search-service-form" className="fox-settings-sheet-body" autoComplete="off" onSubmit={event => { event.preventDefault(); void save() }}>
        <section className="fox-settings-sheet-section fox-settings-search-section">
          <fieldset className="fox-settings-search-services" disabled={!saved || busy || loading}>
            <legend>搜索服务</legend>
            <div className="fox-settings-search-options">
              {services.map(service => <label className="fox-settings-search-option" key={service.id}>
                <span className="fox-settings-provider-mark fox-settings-search-mark" aria-hidden="true">{service.mark === 'globe' ? <Globe2 /> : service.mark}</span>
                <span className="fox-settings-search-copy"><b>{service.name}</b><small>{service.description}</small></span>
                <input type="radio" name="fox-search-provider" value={service.id} checked={provider === service.id} onChange={() => { setProvider(service.id); setMessage('') }} />
              </label>)}
            </div>
          </fieldset>

          <div className="fox-settings-search-fields">
            {provider === 'auto' && <p className="fox-settings-search-direct">无需 API Key，可以直接保存。</p>}
            {provider === 'searxng' && <label className="fox-settings-field" htmlFor="fox-search-url">
              <span>SearXNG 实例地址</span>
              <Input id="fox-search-url" name="searxng-url" type="url" inputMode="url" value={url} placeholder="https://search.example.org" disabled={!saved || busy || loading} onChange={event => setUrl(event.target.value)} />
              <small className="fox-settings-field-help">请在实例中启用 JSON 格式的搜索结果。</small>
            </label>}
            {provider !== 'auto' && provider !== 'searxng' && <label className="fox-settings-field" htmlFor={`fox-search-key-${provider}`}>
              <span>{services.find(service => service.id === provider)?.name} API Key</span>
              <Input id={`fox-search-key-${provider}`} name={`search-key-${provider}`} type="password" autoComplete="off" value={currentKey} placeholder={hasSavedKey ? '已保存；留空保留原值' : '输入 API Key'} disabled={!saved || busy || loading} onChange={event => setKeyDrafts(drafts => ({ ...drafts, [provider]: event.target.value }))} />
              <small className="fox-settings-field-help">{hasSavedKey ? '已配置。留空可继续使用现有 Key。' : 'Key 保存在本机凭据库中。'}</small>
            </label>}
          </div>
          {loading && <p className="fox-settings-sheet-hint" role="status"><LoaderCircle className="animate-spin" />正在读取搜索服务配置…</p>}
          {message && <div className="fox-settings-sheet-error fox-settings-search-message" role="alert">
            <span>{message}</span>
            {!saved && !loading && <Button type="button" variant="outline" size="sm" onClick={() => setLoadAttempt(attempt => attempt + 1)}><RotateCcw />重试</Button>}
          </div>}
        </section>
      </form>
      <div className="fox-settings-sheet-foot">
        <span className="is-grow" />
        <Button type="button" variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>取消</Button>
        <Button type="submit" form="fox-search-service-form" disabled={!saved || busy || loading}>{busy && <LoaderCircle className="animate-spin" />}{busy ? '正在保存…' : '保存搜索'}</Button>
      </div>
    </SheetContent>
  </Sheet>
}

/** Search service row and its Sheet. The summary is refreshed only while the Sheet is closed. */
export function SearchServiceSettings({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const [summary, setSummary] = useState<SearchView | null>(null)
  const [summaryError, setSummaryError] = useState('')

  useEffect(() => {
    if (open) return
    let active = true
    invoke<SearchView>('search_service_get').then(value => {
      if (!active) return
      setSummary(value)
      setSummaryError('')
    }).catch(error => {
      if (active) setSummaryError(String(error))
    })
    return () => { active = false }
  }, [open])

  const providerName = summary ? services.find(service => service.id === summary.provider)?.name ?? summary.provider : null
  return <>
    <div className="fox-settings-providers">
      <div className={cn('fox-settings-provider', open && 'is-open')}>
        <button type="button" className="fox-settings-provider-main" onClick={() => onOpenChange(true)} aria-haspopup="dialog" aria-label="配置网页搜索">
          <span className="fox-settings-provider-mark"><Globe2 /></span>
          <span className="fox-settings-provider-text">
            <span className="fox-settings-provider-name"><b>网页搜索</b></span>
            <small title={summaryError || '用于 Fox 通用助手和专家；外部 Agent 使用各自的搜索能力。'}>{summaryError ? `配置读取失败：${summaryError}` : '用于 Fox 通用助手和专家；外部 Agent 使用各自的搜索能力。'}</small>
          </span>
          <span className="fox-settings-provider-status"><i className={cn('fox-settings-dot', summaryError ? 'is-error' : summary ? 'is-ok' : 'is-empty')} aria-hidden="true" /><b>{summaryError ? '读取失败' : providerName ?? '正在读取'}</b></span>
          <ChevronRight className="fox-settings-provider-chevron" />
        </button>
      </div>
    </div>
    <SearchServiceSheet open={open} onOpenChange={onOpenChange} onSaved={setSummary} />
  </>
}
