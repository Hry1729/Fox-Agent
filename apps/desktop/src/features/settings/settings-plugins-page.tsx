import { useEffect, useState } from 'react'
import { ArrowLeft, Cable, ChevronRight, LoaderCircle, MoreHorizontal, Pencil, Plus, Puzzle, RotateCcw, ShieldCheck, Trash2, Wrench, Zap } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { cn } from '@/lib/utils'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Switch } from '@/components/ui/switch'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import type { SettingsSectionAnchor } from '@/features/workspace/management-navigation'
import type { McpServerRecord, SkillRecord } from '@/features/conversations/model/types'
import { useSkills } from './use-skills'
import { useMcpServers, type SaveMcpServerInput } from './use-mcp-servers'
import { useLifecycleHooks } from './use-lifecycle-hooks'
import { LifecycleHooksSection } from './lifecycle-hooks-section'
import { SettingsGroup, SettingsPageLayout, SettingsRow, settingsSectionId, useScrollToSettingsSection } from './settings-layout'
import { clearPluginReturn, readPluginReturn, type PluginReturnTarget } from './settings-shared'

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

const BUILTIN_CONNECTOR_ID = 'fox-office'

export function extensionTransportLabel(transport: McpServerRecord['transport']) {
  if (transport === 'streamable_http') return 'HTTP MCP'
  if (transport === 'openapi') return 'OpenAPI'
  return 'stdio MCP'
}

function mcpStatus(server: McpServerRecord) {
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

function connectorTarget(server: McpServerRecord) {
  return server.transport === 'stdio' ? `${server.command}${server.args.length ? ` ${server.args.join(' ')}` : ''}` : server.endpointUrl ?? ''
}

function formFor(server: McpServerRecord): McpFormState {
  return { id: server.id, name: server.name, command: server.command, argsText: server.args.join('\n'), transport: server.transport, endpointUrl: server.endpointUrl ?? '', definition: server.definition ?? '', environmentText: '', clearEnvironment: false }
}

function SkillSheet({ skill, busy, onClose, onToggle }: { skill: SkillRecord | null; busy: boolean; onClose: () => void; onToggle: (skill: SkillRecord, enabled: boolean) => void }) {
  return <Sheet open={skill !== null} onOpenChange={(open) => { if (!open) onClose() }}>
    <SheetContent side="right" className="fox-settings-sheet">
      {skill && <>
        <SheetHeader className="fox-settings-sheet-head">
          <span className="fox-settings-provider-mark is-large"><Puzzle /></span>
          <div className="fox-settings-sheet-title"><SheetTitle><span>{skill.name || skill.id}</span>{skill.valid ? <Badge variant="secondary" className="fox-settings-pill">v{skill.version}</Badge> : <Badge variant="destructive" className="fox-settings-pill">校验失败</Badge>}</SheetTitle><SheetDescription>技能只注入指令，不会获得新的工具权限。</SheetDescription></div>
        </SheetHeader>
        <div className="fox-settings-sheet-body">
          <section className="fox-settings-sheet-section">
            <div className="fox-settings-switch-row"><span><b>启用</b><small>{skill.valid ? '启用后 Fox 通用助手会加载这条技能' : '校验失败的技能不能启用'}</small></span><Switch aria-label={`启用 ${skill.name || skill.id}`} checked={skill.enabled} disabled={!skill.valid || busy} onCheckedChange={(enabled) => onToggle(skill, enabled)} /></div>
            {skill.validationError && <p className="fox-settings-sheet-error" role="alert">{skill.validationError}</p>}
          </section>
          <section className="fox-settings-sheet-section">
            <div className="fox-settings-sheet-section-head"><h3>说明</h3></div>
            <dl className="fox-settings-facts">
              <div><dt>简介</dt><dd>{skill.description || '未填写'}</dd></div>
              <div><dt>需要的工具</dt><dd>{skill.requiredTools.length ? skill.requiredTools.join(' · ') : '不要求额外工具'}</dd></div>
              <div><dt>来源</dt><dd title={skill.sourcePath}><code>{skill.sourcePath}</code></dd></div>
            </dl>
          </section>
          {skill.instructions && <section className="fox-settings-sheet-section">
            <div className="fox-settings-sheet-section-head"><h3>指令</h3></div>
            <pre className="fox-settings-code">{skill.instructions}</pre>
          </section>}
        </div>
        <div className="fox-settings-sheet-foot"><span className="is-grow" /><Button type="button" variant="outline" onClick={onClose}>关闭</Button></div>
      </>}
    </SheetContent>
  </Sheet>
}

export function ExtensionsSettingsPage({ sidebarCollapsed, onSidebar, navigate, section }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; section?: SettingsSectionAnchor }) {
  useScrollToSettingsSection(section)
  const skills = useSkills()
  const mcp = useMcpServers()
  const hooks = useLifecycleHooks()
  const [returnTarget] = useState<PluginReturnTarget | null>(() => readPluginReturn())
  useEffect(() => { clearPluginReturn() }, [])
  const [skillId, setSkillId] = useState<string | null>(null)
  const [skillBusy, setSkillBusy] = useState<string | null>(null)
  const [form, setForm] = useState<McpFormState | null>(null)
  const [formError, setFormError] = useState<string | null>(null)
  const [hooksOpen, setHooksOpen] = useState(false)
  const openSkill = skillId ? skills.items.find((item) => item.id === skillId) ?? null : null
  const editingServer = form?.id ? mcp.items.find((item) => item.id === form.id) ?? null : null
  const isBuiltin = form?.id === BUILTIN_CONNECTOR_ID
  useEffect(() => { if (!hooksOpen) void hooks.refresh() }, [hooksOpen]) // eslint-disable-line react-hooks/exhaustive-deps

  const toggleSkill = async (skill: SkillRecord, enabled: boolean) => {
    setSkillBusy(skill.id)
    const ok = await skills.setEnabled(skill.id, enabled)
    setSkillBusy(null)
    if (ok) toast.success(enabled ? `已启用 ${skill.name}` : `已停用 ${skill.name}`)
  }
  const openConnector = (server?: McpServerRecord) => { setFormError(null); setForm(server ? formFor(server) : { ...emptyMcpForm }) }
  const closeConnector = () => { setForm(null); setFormError(null) }
  const saveConnector = async () => {
    if (!form) return
    try {
      const environment = parseEnvironment(form.environmentText)
      const input: SaveMcpServerInput = { id: form.id, name: form.name.trim(), command: form.command.trim(), args: form.argsText.split(/\r?\n/).map((value) => value.trim()).filter(Boolean), transport: form.transport, endpointUrl: form.endpointUrl.trim() || undefined, definition: form.definition.trim() || undefined, clearEnvironment: form.clearEnvironment }
      if (Object.keys(environment).length) input.environment = environment
      const saved = await mcp.save(input)
      if (!saved) return
      closeConnector()
      toast.success(form.id ? '连接器已更新' : '连接器已添加')
    } catch (cause) { setFormError(cause instanceof Error ? cause.message : String(cause)) }
  }
  const testConnector = (server: McpServerRecord) => void mcp.test(server.id).then((result) => { if (result) toast.success(`健康检查通过，发现 ${result.toolCount} 个工具`) })
  const removeConnector = async (server: McpServerRecord) => {
    const confirmed = await confirmDialog({ title: '删除连接器', description: `删除连接器“${server.name}”？已保存的凭证也会被清除。`, confirmLabel: '删除', destructive: true })
    if (!confirmed) return
    if (await mcp.remove(server.id)) { if (form?.id === server.id) closeConnector(); toast.success('连接器已删除') }
  }
  const goBack = () => {
    if (!returnTarget) return
    navigate(returnTarget.view, returnTarget.entityId, returnTarget.documentId ? { documentId: returnTarget.documentId } : undefined)
  }
  const enabledSkills = skills.items.filter((item) => item.enabled).length
  const enabledConnectors = mcp.items.filter((item) => item.enabled).length
  const enabledHooks = hooks.items.filter((item) => item.enabled).length
  const formInvalid = !form?.name.trim() || (form.transport === 'stdio' && !form.command.trim()) || (form.transport === 'streamable_http' && !form.endpointUrl.trim()) || (form.transport === 'openapi' && !form.definition.trim())
  const test = form?.id ? mcp.tests[form.id] : undefined

  return <WorkspacePage title="插件" subtitle="技能、连接器与策略" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout title="插件" description="管理技能、连接器和工具调用策略；所有调用仍经过 Fox 审批和审计。" actions={<>
      {returnTarget && <Button type="button" variant="outline" onClick={goBack}><ArrowLeft />返回 {returnTarget.label}</Button>}
      <Button type="button" onClick={() => openConnector()}><Plus />添加连接器</Button>
    </>}>
      <SettingsGroup id={settingsSectionId('skills')} title="技能" aside={<>{skills.items.length > 0 && <span>{skills.items.length} 个，{enabledSkills} 个已启用</span>}<Button type="button" variant="ghost" size="sm" disabled={skills.loading} onClick={() => void skills.refresh()}>{skills.loading ? <LoaderCircle className="animate-spin" /> : <RotateCcw />}重新扫描</Button></>}>
        {skills.error && <p className="fox-settings-error" role="alert">暂时无法读取技能列表：{skills.error}</p>}
        {!skills.loading && !skills.error && skills.items.length === 0 && <p className="fox-settings-empty">还没有技能。在 Fox 数据目录的 skills 子目录中放入包含 SKILL.md 的文件夹后点「重新扫描」。</p>}
        <div className="fox-settings-providers">
          {skills.items.map((skill) => <div className={cn('fox-settings-provider', skillId === skill.id && 'is-open')} key={skill.id}>
            <button type="button" className="fox-settings-provider-main" onClick={() => setSkillId(skill.id)} aria-haspopup="dialog" aria-label={`查看 ${skill.name || skill.id}`}>
              <span className="fox-settings-provider-mark"><Puzzle /></span>
              <span className="fox-settings-provider-text"><span className="fox-settings-provider-name"><b>{skill.name || skill.id}</b>{skill.valid ? <Badge variant="secondary" className="fox-settings-pill">v{skill.version}</Badge> : <Badge variant="destructive" className="fox-settings-pill">校验失败</Badge>}</span><small title={skill.validationError ?? skill.description}>{skill.validationError || skill.description || skill.id}</small></span>
            </button>
            <Switch className="fox-settings-provider-switch" aria-label={`启用 ${skill.name || skill.id}`} checked={skill.enabled} disabled={!skill.valid || skillBusy === skill.id} onCheckedChange={(enabled) => void toggleSkill(skill, enabled)} />
          </div>)}
        </div>
      </SettingsGroup>

      <SettingsGroup id={settingsSectionId('mcp')} title="连接器" aside={mcp.items.length ? `${mcp.items.length} 个，${enabledConnectors} 个已启用` : undefined}>
        {mcp.error && !form && <p className="fox-settings-error" role="alert">{mcp.error}</p>}
        {!mcp.loading && mcp.items.length === 0 && <p className="fox-settings-empty">还没有连接器。点右上角「添加连接器」接入本机 MCP、HTTP MCP 或 OpenAPI。</p>}
        <div className="fox-settings-providers">
          {mcp.items.map((server) => {
            const status = mcpStatus(server)
            const result = mcp.tests[server.id]
            const toolCount = result ? result.toolCount : server.toolCount
            const builtin = server.id === BUILTIN_CONNECTOR_ID
            return <div className={cn('fox-settings-provider', form?.id === server.id && 'is-open')} key={server.id}>
              <button type="button" className="fox-settings-provider-main" onClick={() => openConnector(server)} aria-haspopup="dialog" aria-label={`编辑 ${server.name}`}>
                <span className="fox-settings-provider-mark"><Cable /></span>
                <span className="fox-settings-provider-text"><span className="fox-settings-provider-name"><b>{server.name}</b>{builtin && <Badge variant="secondary" className="fox-settings-pill">内置</Badge>}</span><small title={server.lastError ?? connectorTarget(server)}>{server.lastError || `${builtin ? '内置 CLI' : extensionTransportLabel(server.transport)} · ${connectorTarget(server)}`}</small></span>
                <span className="fox-settings-provider-status"><i className={cn('fox-settings-dot', status.tone)} aria-hidden="true" /><b>{status.label}</b>{toolCount != null && <><span className="is-separator">·</span><span>{toolCount} 个工具</span></>}</span>
              </button>
              <Switch className="fox-settings-provider-switch" aria-label={`启用 ${server.name}`} checked={server.enabled} disabled={mcp.busyId === server.id} onCheckedChange={(enabled) => void mcp.setEnabled(server.id, enabled)} />
              <DropdownMenu modal={false}>
                <DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" className="fox-settings-provider-more" aria-label={`管理 ${server.name}`}><MoreHorizontal /></Button></DropdownMenuTrigger>
                <DropdownMenuContent align="end">
                  <DropdownMenuItem disabled={!server.enabled || mcp.busyId === server.id} onSelect={() => testConnector(server)}><Zap />健康检查</DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => runAfterMenuClose(() => openConnector(server))}><Pencil />{builtin ? '查看' : '编辑'}</DropdownMenuItem>
                  {!builtin && <><DropdownMenuSeparator /><DropdownMenuItem variant="destructive" onSelect={() => runAfterMenuClose(() => void removeConnector(server))}><Trash2 />删除</DropdownMenuItem></>}
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          })}
        </div>
      </SettingsGroup>

      <SettingsGroup title="策略">
        <SettingsRow title="生命周期 Hook" description={hooks.items.length ? `${hooks.items.length} 条，${enabledHooks} 条启用。按工具名匹配并阻止、要求审批或附加说明，不执行脚本` : '按工具名匹配并阻止、要求审批或附加说明；Hook 不执行脚本，每次命中都写入审计'}>
          <Button type="button" variant="outline" size="sm" onClick={() => setHooksOpen(true)}><ShieldCheck />管理</Button>
        </SettingsRow>
        <SettingsRow title="工具插件" description="内置工具和已安装工具插件的状态与兼容性">
          <Button type="button" variant="ghost" size="sm" onClick={() => navigate('plugins', 'tool')}><Wrench />插件中心<ChevronRight /></Button>
        </SettingsRow>
      </SettingsGroup>
    </SettingsPageLayout>

    <SkillSheet skill={openSkill} busy={skillBusy !== null} onClose={() => setSkillId(null)} onToggle={(skill, enabled) => void toggleSkill(skill, enabled)} />

    <Sheet open={form !== null} onOpenChange={(open) => { if (!open) closeConnector() }}>
      <SheetContent side="right" className="fox-settings-sheet">
        {form && <>
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
            {(formError || (mcp.error && form)) && <p className="fox-settings-sheet-error" role="alert">{formError ?? mcp.error}</p>}
          </div>
          <div className="fox-settings-sheet-foot">
            {editingServer && !isBuiltin && <Button type="button" variant="ghost" className="fox-settings-danger" disabled={mcp.busyId === editingServer.id} onClick={() => void removeConnector(editingServer)}><Trash2 />删除连接器</Button>}
            <span className="is-grow" />
            <Button type="button" variant="outline" onClick={closeConnector}>{isBuiltin ? '关闭' : '取消'}</Button>
            {!isBuiltin && <Button type="button" disabled={formInvalid || mcp.busyId != null} onClick={() => void saveConnector()}>{mcp.busyId === (form.id ?? 'new') && <LoaderCircle className="animate-spin" />}{form.id ? '保存' : '添加'}</Button>}
          </div>
        </>}
      </SheetContent>
    </Sheet>

    <Sheet open={hooksOpen} onOpenChange={setHooksOpen}>
      <SheetContent side="right" className="fox-settings-sheet fox-settings-hooks-sheet">
        <SheetHeader className="fox-settings-sheet-head">
          <span className="fox-settings-provider-mark is-large"><ShieldCheck /></span>
          <div className="fox-settings-sheet-title"><SheetTitle><span>生命周期 Hook</span></SheetTitle><SheetDescription>匹配器支持精确名称、逗号分组和前后缀通配符，例如 call_*。</SheetDescription></div>
        </SheetHeader>
        <div className="fox-settings-sheet-body"><LifecycleHooksSection showHeader={false} /></div>
      </SheetContent>
    </Sheet>
  </WorkspacePage>
}
