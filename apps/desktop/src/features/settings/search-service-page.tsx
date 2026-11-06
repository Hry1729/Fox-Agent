import { useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Input } from '@/components/ui/input'
import { WorkspacePage } from '@/features/workspace/page-shell'
import { SettingsGroup, SettingsPageLayout, SettingsRow } from './settings-layout'

const services = {
  auto: '免 Key 搜索',
  tavily: 'Tavily',
  brave: 'Brave Search',
  exa: 'Exa',
  searxng: 'SearXNG',
}
type Provider = keyof typeof services
type SearchView = { provider: Provider; searxngUrl: string; configuredKeys: Provider[] }

export default function SearchServicePage({ sidebarCollapsed, onSidebar }: { sidebarCollapsed: boolean; onSidebar: () => void }) {
  const [saved, setSaved] = useState<SearchView | null>(null)
  const [provider, setProvider] = useState<Provider>('auto')
  const [url, setUrl] = useState('')
  const [key, setKey] = useState('')
  const [busy, setBusy] = useState(false)
  const [message, setMessage] = useState('')

  useEffect(() => {
    let active = true
    invoke<SearchView>('search_service_get').then(value => {
      if (active) {
        setSaved(value)
        setProvider(value.provider)
        setUrl(value.searxngUrl)
      }
    }).catch(error => { if (active) setMessage(String(error)) })
    return () => { active = false }
  }, [])

  async function save() {
    setBusy(true)
    setMessage('')
    try {
      const value = await invoke<SearchView>('search_service_save', {
        request: { provider, apiKey: key || null, searxngUrl: provider === 'searxng' ? url : null },
      })
      setSaved(value)
      setKey('')
      setMessage('已保存，下次 Fox 搜索时生效。')
    } catch (error) {
      setMessage(String(error))
    } finally {
      setBusy(false)
    }
  }

  const needsKey = provider !== 'auto' && provider !== 'searxng'
  return <WorkspacePage title="搜索服务" subtitle="设置" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout title="搜索服务" description="选择 Fox 搜索网页时使用的服务。外部 Agent 优先使用自己的搜索。">
      <SettingsGroup title="网页搜索">
        <SettingsRow title={<label htmlFor="fox-search-provider">搜索服务</label>} description={provider === 'auto' ? '默认使用 DuckDuckGo，遇到拦截或没有结果时尝试 Yahoo。' : undefined}>
          <Select value={provider} disabled={!saved || busy} onValueChange={value => {
            setProvider(value as Provider)
            setKey('')
            setMessage('')
          }}>
            <SelectTrigger id="fox-search-provider" className="fox-settings-select"><SelectValue /></SelectTrigger>
            <SelectContent>
              {Object.entries(services).map(([id, name]) => <SelectItem key={id} value={id}>{name}</SelectItem>)}
            </SelectContent>
          </Select>
        </SettingsRow>
        {needsKey && <SettingsRow title={<label htmlFor="fox-search-key">API Key</label>} description={saved?.configuredKeys.includes(provider) ? '已配置。留空可继续使用现有 Key。' : 'Key 保存在本机凭据库中。'}>
          <Input id="fox-search-key" type="password" autoComplete="off" value={key} disabled={busy} onChange={event => setKey(event.target.value)} />
        </SettingsRow>}
        {provider === 'searxng' && <SettingsRow title={<label htmlFor="fox-search-url">实例地址</label>} description="实例需启用 JSON 搜索结果。">
          <Input id="fox-search-url" type="url" value={url} placeholder="https://search.example.org" disabled={busy} onChange={event => setUrl(event.target.value)} />
        </SettingsRow>}
      </SettingsGroup>
      <Button disabled={!saved || busy} onClick={save}>{busy ? '正在保存…' : '保存'}</Button>
      {message && <p role="status">{message}</p>}
    </SettingsPageLayout>
  </WorkspacePage>
}
