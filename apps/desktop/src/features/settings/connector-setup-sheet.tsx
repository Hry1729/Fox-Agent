import { useState } from 'react'
import { Cable, LoaderCircle, Zap } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Switch } from '@/components/ui/switch'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import type { McpServerRecord } from '@/features/conversations/model/types'
import { useMcpServers, type SaveMcpServerInput } from './use-mcp-servers'

interface McpFormState {
  id?: string
  name: string
  command: string
  argsText: string
  transport: McpServerRecord['transport']
  endpointUrl: string
  definition: string
  environmentText: string
  clearEnvironment: boolean
}

const emptyMcpForm: McpFormState = { name: '', command: '', argsText: '', transport: 'stdio', endpointUrl: '', definition: '', environmentText: '', clearEnvironment: false }

export const BUILTIN_CONNECTOR_ID = 'fox-office'

export function extensionTransportLabel(transport: McpServerRecord['transport']) {
  if (transport === 'streamable_http') return 'HTTP MCP'
  if (transport === 'openapi') return 'OpenAPI'
  return 'stdio MCP'
}

export function mcpStatus(server: McpServerRecord) {
  if (!server.enabled) return { tone: 'is-empty', label: '已停用' }
  if (server.status === 'connected') return { tone: 'is-ok', label: '可用' }
  if (server.status === 'unavailable') return { tone: 'is-error', label: '不可用' }
  return { tone: '', label: '未测试' }
}

export function parseEnvironment(value: string) {
  const environment: Record<string, string> = {}
  for (const rawLine of value.split(/\r?\n/)) {
    const line = rawLine.trim()
    if (!line) continue
    const separator = line.indexOf('=')
    if (separator < 1) throw new Error(`环境变量格式无效：${line}`)
    environment[line.slice(0, separator).trim()] = line.slice(separator + 1)
  }
  return environment
}

function formFor(server: McpServerRecord): McpFormState {
  return { id: server.id, name: server.name, command: server.command, argsText: server.args.join('\n'), transport: server.transport, endpointUrl: server.endpointUrl ?? '', definition: server.definition ?? '', environmentText: '', clearEnvironment: false }
}

export function ConnectorSetupSheet({ mcp, server, suggestedName, onClose, onSaved }: {
  mcp: ReturnType<typeof useMcpServers>
  server?: McpServerRecord
  suggestedName?: string
  onClose: () => void
  onSaved?: () => void
}) {
  const [form, setForm] = useState(() => server ? formFor(server) : { ...emptyMcpForm, name: suggestedName ?? '' })
  const [formError, setFormError] = useState<string | null>(null)
  const editingServer = form.id ? mcp.items.find(item => item.id === form.id) ?? server : undefined
  const isBuiltin = form.id === BUILTIN_CONNECTOR_ID
  const saveConnector = async () => {
    try {
      const environment = parseEnvironment(form.environmentText)
      const input: SaveMcpServerInput = { id: form.id, name: form.name.trim(), command: form.command.trim(), args: form.argsText.split(/\r?\n/).map((value) => value.trim()).filter(Boolean), transport: form.transport, endpointUrl: form.endpointUrl.trim() || undefined, definition: form.definition.trim() || undefined, clearEnvironment: form.clearEnvironment }
      if (Object.keys(environment).length) input.environment = environment
      const saved = await mcp.save(input)
      if (!saved) return
      onSaved?.()
      onClose()
      toast.success(form.id ? '连接器已更新' : '连接器已添加')
    } catch (cause) { setFormError(cause instanceof Error ? cause.message : String(cause)) }
  }
  const testConnector = (server: McpServerRecord) => void mcp.test(server.id).then((result) => { if (result) toast.success(`健康检查通过，发现 ${result.toolCount} 个工具`) })
  const formInvalid = !form.name.trim() || (form.transport === 'stdio' && !form.command.trim()) || (form.transport === 'streamable_http' && !form.endpointUrl.trim()) || (form.transport === 'openapi' && !form.definition.trim())
  const test = form.id ? mcp.tests[form.id] : undefined

  return <Sheet open onOpenChange={open => { if (!open && mcp.busyId == null) onClose() }}>
      <SheetContent side="right" className="fox-settings-sheet">
          <SheetHeader className="fox-settings-sheet-head">
            <span className="fox-settings-provider-mark is-large"><Cable /></span>
            <div className="fox-settings-sheet-title">
              <SheetTitle><span>{form.id ? editingServer?.name ?? form.name : '添加连接器'}</span>{editingServer && <Badge variant="secondary" className="fox-settings-pill">{mcpStatus(editingServer).label}</Badge>}</SheetTitle>
              <SheetDescription>凭证只存入系统凭证库；HTTP MCP 和 OpenAPI 禁止自动重定向，超时 15 秒，响应上限 10 MB。</SheetDescription>
            </div>
          </SheetHeader>
          <div className="fox-settings-sheet-body">
            {isBuiltin
              ? <section className="fox-settings-sheet-section"><p className="fox-settings-sheet-hint">Fox 内置 CLI 连接器不能编辑或删除，可以启停和做健康检查。</p></section>
              : <section className="fox-settings-sheet-section">
                <label className="fox-settings-field"><span>名称</span><Input value={form.name} onChange={(event) => setForm({ ...form, name: event.target.value })} placeholder="例如 Filesystem 或 Pet API" /></label>
                <div className="fox-settings-field"><span>扩展类型</span><Select value={form.transport} onValueChange={(transport: McpServerRecord['transport']) => setForm({ ...form, transport })}><SelectTrigger aria-label="扩展类型"><SelectValue /></SelectTrigger><SelectContent><SelectItem value="stdio">stdio MCP</SelectItem><SelectItem value="streamable_http">Streamable HTTP MCP</SelectItem><SelectItem value="openapi">OpenAPI 3.x Connector</SelectItem></SelectContent></Select></div>
                {form.transport === 'stdio'
                  ? <>
                    <label className="fox-settings-field"><span>命令</span><Input value={form.command} onChange={(event) => setForm({ ...form, command: event.target.value })} placeholder="例如 npx.cmd 或 MCP Server 可执行文件路径" /></label>
                    <label className="fox-settings-field"><span>参数（每行一项）</span><Textarea className="fox-settings-textarea" value={form.argsText} onChange={(event) => setForm({ ...form, argsText: event.target.value })} placeholder={'-y\n@modelcontextprotocol/server-filesystem\nD:\\projects'} /></label>
                  </>
                  : <label className="fox-settings-field"><span>端点 URL{form.transport === 'openapi' ? '（可选，覆盖 servers[0].url）' : ''}</span><Input value={form.endpointUrl} onChange={(event) => setForm({ ...form, endpointUrl: event.target.value })} placeholder={form.transport === 'openapi' ? '可留空使用定义中的 servers[0].url' : 'https://mcp.example.com/mcp'} /></label>}
                {form.transport === 'openapi' && <label className="fox-settings-field"><span>OpenAPI JSON / YAML</span><Textarea className="fox-settings-textarea is-tall" value={form.definition} onChange={(event) => setForm({ ...form, definition: event.target.value })} placeholder={'openapi: 3.0.3\ninfo: ...\npaths: ...'} /></label>}
                <label className="fox-settings-field"><span>{form.transport === 'stdio' ? '环境变量' : '请求 Header'}（每行 KEY=VALUE）</span><Textarea className="fox-settings-textarea" value={form.environmentText} onChange={(event) => setForm({ ...form, environmentText: event.target.value, clearEnvironment: false })} placeholder={form.id ? '已保存的值不会回显；留空保持不变' : form.transport === 'stdio' ? 'API_KEY=...' : 'Authorization=Bearer ...'} /></label>
                {form.id && <div className="fox-settings-switch-row fox-settings-field"><span><b>清除登录信息</b><small>{editingServer?.credentialConfigured ? '保存时同时从系统凭证库删除' : '当前没有保存凭证'}</small></span><Switch aria-label="清除登录信息" checked={form.clearEnvironment} onCheckedChange={(clearEnvironment) => setForm({ ...form, clearEnvironment, environmentText: clearEnvironment ? '' : form.environmentText })} /></div>}
              </section>}
            {editingServer && <section className="fox-settings-sheet-section">
              <div className="fox-settings-sheet-section-head"><h3>工具</h3><Button type="button" variant="ghost" size="sm" disabled={!editingServer.enabled || mcp.busyId === editingServer.id} onClick={() => testConnector(editingServer)}>{mcp.busyId === editingServer.id ? <LoaderCircle className="animate-spin" /> : <Zap />}健康检查</Button></div>
              {editingServer.lastError && <p className="fox-settings-sheet-error" role="alert">{editingServer.lastError}</p>}
              {test?.tools.length
                ? <div className="fox-settings-tool-list">{test.tools.map((tool) => <div key={tool.name}><b>{tool.name}</b><small>{tool.description || '此工具没有提供说明。'}</small></div>)}</div>
                : <p className="fox-settings-sheet-hint">{editingServer.toolCount != null ? `上次检查发现 ${editingServer.toolCount} 个工具${editingServer.lastLatencyMs != null ? ` · ${editingServer.lastLatencyMs} ms` : ''}。` : '还没有做过健康检查。'}点「健康检查」读取并校验工具目录，每个连接器最多 200 个工具。</p>}
            </section>}
            {(formError || mcp.error) && <p className="fox-settings-sheet-error" role="alert">{formError ?? mcp.error}</p>}
          </div>
          <div className="fox-settings-sheet-foot">
            <span className="is-grow" />
            <Button type="button" variant="outline" disabled={mcp.busyId != null} onClick={onClose}>{isBuiltin ? '关闭' : '取消'}</Button>
            {!isBuiltin && <Button type="button" disabled={formInvalid || mcp.busyId != null} onClick={() => void saveConnector()}>{mcp.busyId === (form.id ?? 'new') && <LoaderCircle className="animate-spin" />}{form.id ? '保存' : '添加'}</Button>}
          </div>
      </SheetContent>
    </Sheet>
}
