import { useCallback, useEffect, useState } from 'react'
import { SettingsGroup, SettingsRow } from '@/features/settings/settings-layout'
import { Button } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'
import { desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { contacts, WEB_SERVER_ID, type Companion, type CompanionCapabilities, type ConnectorCapability } from './api'
import { showContactError } from './context'
import './contacts.css'

const WEB_LABELS: Record<string, { label: string; description: string }> = {
  web_search: { label: '网页搜索', description: '用已配置的搜索服务查公开网页' },
  web_read: { label: '读取网页', description: '打开链接，读取正文' },
}

export const CAPABILITY_FOOTER = '从下一轮对话开始生效。已经在等你确认的操作，也会按新的设置重新检查。'
export const STDIO_BOUNDARY = '它在这台电脑上运行。Fox 只能决定它能用哪些工具，管不住它自己能读写哪些文件。'

export function connectorStatus(connector: ConnectorCapability): { label: string; tone: string } {
  if (!connector.enabled) return { label: '未启用', tone: 'is-empty' }
  if (connector.lastError) return { label: '还没连接', tone: 'is-error' }
  return { label: '已连接', tone: 'is-ok' }
}

export function ContactCapabilities({ contact, onBack }: { contact: Companion; onBack?: () => void }) {
  const [data, setData] = useState<CompanionCapabilities | null>(null)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const reload = useCallback(async () => {
    try { setData(await contacts.capabilities(contact.id)); setError('') } catch (e) { setError(desktopErrorDetails(e).message) }
  }, [contact.id])
  useEffect(() => { void reload() }, [reload])
  const change = async (serverId: string, tool: string, granted: boolean) => {
    setBusy(true)
    try { await contacts.setCapability(contact.id, serverId, tool, granted); await reload() } catch (e) { showContactError(e) } finally { setBusy(false) }
  }
  return <div className="fox-contact-capabilities" aria-busy={busy}>
    <div className="fox-contact-capabilities-head"><h3>可用能力</h3>
      {onBack && <Button size="sm" variant="ghost" onClick={onBack}>返回资料</Button>}</div>
    {error && <p role="alert">{error} <button type="button" onClick={() => void reload()}>重试</button></p>}
    {!data ? <p>正在读取可用能力…</p> : <>
      <SettingsGroup title="网页">
        {data.web.map(item => {
          const meta = WEB_LABELS[item.tool] ?? { label: item.tool, description: '' }
          return <SettingsRow key={item.tool} title={meta.label} description={meta.description || undefined}>
            <Switch aria-label={meta.label} checked={item.granted} disabled={busy} onCheckedChange={next => void change(WEB_SERVER_ID, item.tool, next)} />
          </SettingsRow>
        })}
      </SettingsGroup>
      <SettingsGroup title="连接器">
        {!data.connectors.length && <p className="fox-contact-capability-hint">还没有配置连接器。</p>}
        {data.connectors.map(connector => {
          const status = connectorStatus(connector)
          const used = connector.tools.filter(tool => tool.granted).length
          return <div key={connector.serverId} className="fox-contact-capability">
            <div className="fox-contact-capability-head"><b>{connector.name}</b>
              <span className="fox-contact-capability-status"><i className={cn('fox-settings-dot', status.tone)} aria-hidden="true" />{status.label}
                {connector.enabled && !connector.lastError && connector.tools.length > 0 && <span> · 用 {used} 个，共 {connector.tools.length} 个</span>}</span></div>
            {connector.enabled && connector.lastError && <p className="fox-contact-capability-reason">{connector.lastError}</p>}
            {!connector.enabled ? <p className="fox-contact-capability-hint">启用后可选择工具</p>
              : connector.tools.length ? connector.tools.map(tool =>
                <label key={tool.name} className="fox-contact-capability-tool">
                  <input type="checkbox" checked={tool.granted} disabled={busy} aria-label={`允许使用 ${connector.name} 的工具 ${tool.name}`} onChange={e => void change(connector.serverId, tool.name, e.target.checked)} />
                  <span>{tool.name}</span>
                </label>)
              : <p className="fox-contact-capability-hint">没有发现可用工具。</p>}
            {connector.transport.toLowerCase() === 'stdio' && <p className="fox-contact-capability-boundary">{STDIO_BOUNDARY}</p>}
          </div>
        })}
      </SettingsGroup>
      <p className="fox-contact-capability-foot">{CAPABILITY_FOOTER}</p>
    </>}
  </div>
}