import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { useMcpServers } from '@/features/settings/use-mcp-servers'
import { connectorServerId, presetInput, type ConnectorPreset } from './connector-presets'

const fieldLabels: Record<string, string> = {
  FOX_EMAIL: '邮箱账号', FOX_PASSWORD: '邮箱客户端授权码', FOX_API_KEY: '平台 API Key',
  FOX_BASE_URL: '服务地址', FOX_CLIENT_ID: '客户端 ID', FOX_TOKEN: '访问令牌',
  FOX_ACCOUNT_SID: 'Twilio Account SID', FOX_PHONE: 'WhatsApp 企业号码（含国际区号）',
  FOX_SUBDOMAIN: 'Zendesk 子域名', FOX_FLYAI_ENTRY: '飞猪 CLI 文件路径', YUQUE_TOKEN: '语雀个人访问令牌',
  AWS_PROFILE: '本机 AWS 配置名称', AWS_REGION: 'AWS 区域', DATABASE_URI: '数据库连接字符串',
  REDIS_URL: 'Redis 连接字符串', MDB_MCP_CONNECTION_STRING: 'MongoDB 连接字符串',
}

export function CatalogConnectSheet({ item, mcp, onClose, onChange }: {
  item: ConnectorPreset; mcp: ReturnType<typeof useMcpServers>; onClose: () => void; onChange: () => void
}) {
  const { setup } = item
  const [values, setValues] = useState<Record<string, string>>({})
  const [manual, setManual] = useState(false)
  const [ready, setReady] = useState<boolean | null>(setup.registration ? null : true)
  const [message, setMessage] = useState('')
  const [busy, setBusy] = useState(false)
  const oauth = setup.oauth && !manual
  useEffect(() => {
    if (!setup.registration || !desktopRuntimeAvailable) return
    let active = true
    void desktopClient.catalogAuthorizationReady(connectorServerId(item)).then(available => {
      if (!active) return
      setReady(available)
      if (!available && setup.secretHeader) setManual(true)
    }).catch(error => { if (active) { setReady(false); setMessage(String(error)) } })
    return () => { active = false }
  }, [item, setup])
  async function connect() {
    setBusy(true)
    setMessage('')
    try {
      const id = connectorServerId(item)
      if (oauth) await desktopClient.authorizeCatalogConnector(id)
      else {
        const saved = await mcp.save(presetInput(item, values, navigator.platform.startsWith('Win')))
        if (!saved) return
      }
      await mcp.refresh()
      onChange()
      const result = await mcp.test(id)
      onChange()
      if (!result) { setMessage('已保存配置，暂时未能读取平台工具。'); return }
      setMessage('已读取 ' + result.toolCount + ' 个工具。可在对话中试用并核对账号权限。')
    } catch (e) { setMessage(e instanceof Error ? e.message : String(e)) }
    finally { setBusy(false) }
  }
  const field = (key: string, label: string, secret = false) => <label key={key} className="fox-settings-field"><span>{label}</span>
    <Input type={secret ? 'password' : 'text'} autoComplete="off" value={values[key] ?? ''} onChange={e => setValues({ ...values, [key]: e.target.value })} /></label>
  return <Sheet open onOpenChange={open => { if (!open && !busy) onClose() }}>
    <SheetContent className="fox-settings-sheet" side="right">
      <SheetHeader className="fox-settings-sheet-head"><div><SheetTitle>连接{item.name}</SheetTitle>
        <SheetDescription>{setup.pending ?? (oauth ? '在平台授权窗口确认后，返回 Fox 检查可用工具。' : item.description)}</SheetDescription></div></SheetHeader>
      <div className="fox-settings-sheet-body"><section className="fox-settings-sheet-section">
        {!desktopRuntimeAvailable && <p>请在 Fox 桌面端打开此连接器以完成连接。</p>}
        {setup.guide && <p>{setup.guide}</p>}
        {ready === false && <p role="status">Fox 的平台应用配置尚未就绪。{setup.secretHeader && '当前可使用平台访问令牌。'}</p>}
        {setup.program && setup.program !== '{fox-sse-bridge}' && <p>本机需要 {setup.program}。{['npx','uvx','uv'].includes(setup.program) && '首次连接可能需要下载工具包。'}</p>}
        {desktopRuntimeAvailable && !oauth && !setup.pending && <>
          {setup.customEndpoint && field('endpoint', '服务提供的 MCP 端点 URL')}
          {(setup.secretHeader || setup.queryKey) && field('token', setup.optionalToken ? '访问令牌（可选）' : '访问令牌', true)}
          {item.id === 'lexiang' && field('company_from', '企业标识 company_from')}
          {setup.args?.includes('{organization}') && field('organization', 'Azure DevOps 组织名称')}
          {setup.args?.includes('{directory}') && field('directory', '允许访问的文件夹绝对路径')}
          {setup.env?.map(key => field(key, fieldLabels[key] ?? key, /KEY|TOKEN|PASSWORD|SECRET|URI|REDIS_URL|CONNECTION_STRING/i.test(key)))}
        </>}
        {desktopRuntimeAvailable && ready && setup.oauth && setup.secretHeader && <Button variant="link" disabled={busy} onClick={() => { setManual(!manual); setMessage('') }}>{manual ? '改用账号授权' : '使用个人令牌'}</Button>}
        {desktopRuntimeAvailable ? <Button variant="link" onClick={() => { void desktopClient.openExternalUrl(item.source).catch(e => setMessage(String(e))) }}>查看平台接入说明</Button>
          : <a href={item.source} target="_blank" rel="noreferrer">查看平台接入说明</a>}
        {(message || mcp.error) && <p role="status">{[message, mcp.error].filter(Boolean).join(' ')}</p>}
      </section></div>
      <div className="fox-settings-sheet-foot"><Button variant="outline" disabled={busy} onClick={onClose}>关闭</Button>
        {desktopRuntimeAvailable && !setup.pending && <Button disabled={busy || Boolean(oauth && !ready)} onClick={() => void connect()}>{busy ? (oauth ? '等待授权与连接…' : '正在连接…') : oauth ? '打开授权窗口' : '保存并测试连接'}</Button>}</div>
    </SheetContent></Sheet>
}
