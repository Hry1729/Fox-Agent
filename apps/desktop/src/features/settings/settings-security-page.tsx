import { useEffect, useState } from 'react'
import { Archive, ExternalLink, FolderOpen, LoaderCircle, MoreHorizontal, RotateCcw, ShieldCheck } from 'lucide-react'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger, runAfterMenuClose } from '@/components/ui/dropdown-menu'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { SettingsSectionAnchor } from '@/features/workspace/management-navigation'
import { ProjectContactGrants } from '@/features/companions/profile'
import { desktopClient } from '@/features/conversations/api/desktop-client'
import type { ProjectManagementRecord, ProjectRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { cn } from '@/lib/utils'
import { LifecycleHooksSection } from './lifecycle-hooks-section'
import { ProjectPermissionRules } from './project-permission-rules'
import { SettingsGroup, SettingsPageLayout, SettingsRow, settingsSectionId, useScrollToSettingsSection } from './settings-layout'
import { useLifecycleHooks } from './use-lifecycle-hooks'
import { useStoredPreference } from './settings-shared'

type PermissionMode = ProjectRecord['permissionMode']
const PERMISSION_LABELS: Record<PermissionMode, string> = { read_only: '只读', ask: '询问', allow: '自动' }

function PermissionSelect({ value, disabled, label, onChange, className }: { value: PermissionMode; disabled?: boolean; label: string; onChange: (value: PermissionMode) => void; className?: string }) {
  return <Select value={value} disabled={disabled} onValueChange={next => onChange(next as PermissionMode)}>
    <SelectTrigger className={cn('fox-settings-compact-select', className)} aria-label={label}><SelectValue /></SelectTrigger>
    <SelectContent><SelectItem value="read_only">只读</SelectItem><SelectItem value="ask">询问</SelectItem><SelectItem value="allow">自动</SelectItem></SelectContent>
  </Select>
}

function projectStorageLabel(bytes: number | null) {
  if (bytes === null) return '未统计'
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`
  return `${(bytes / 1024 ** 3).toFixed(1)} GB`
}

function useManagedProjects() {
  const [projects, setProjects] = useState<ProjectManagementRecord[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const load = async () => {
    setLoading(true)
    try { setProjects(await desktopClient.listManagedProjects()); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }
  useEffect(() => { void load() }, [])
  const run = async (project: ProjectManagementRecord, action: () => Promise<void>) => {
    setBusyId(project.id)
    try { await action() } finally { setBusyId(null) }
  }
  const updatePermission = async (project: ProjectManagementRecord, permissionMode: PermissionMode) => {
    await run(project, async () => {
      try {
        const updated = await desktopClient.updateProjectPermission(project.id, permissionMode)
        setProjects(items => items.map(item => item.id === updated.id ? { ...item, permissionMode: updated.permissionMode, updatedAt: updated.updatedAt } : item))
        toast.success(`已更新 ${updated.name} 的执行方式`)
      } catch (cause) { toast.error('权限更新失败', { description: cause instanceof Error ? cause.message : String(cause) }) }
    })
  }
  const choosePath = async (project: ProjectManagementRecord) => {
    await run(project, async () => {
      try {
        const rootPath = await desktopClient.pickProjectFolder()
        if (!rootPath) return
        const updated = await desktopClient.updateProjectPath(project.id, rootPath)
        setProjects(items => items.map(item => item.id === updated.id ? updated : item))
        toast.success(`已迁移 ${updated.name} 的项目路径`)
      } catch (cause) { toast.error('项目路径更新失败', { description: cause instanceof Error ? cause.message : String(cause) }) }
    })
  }
  const setArchived = async (project: ProjectManagementRecord, archived: boolean) => {
    await run(project, async () => {
      try {
        await desktopClient.setProjectArchived(project.id, archived)
        setProjects(items => items.map(item => item.id === project.id ? { ...item, archivedAt: archived ? Date.now() : null } : item))
        toast.success(archived ? `已归档 ${project.name}` : `已恢复 ${project.name}`)
      } catch (cause) { toast.error(archived ? '项目归档失败' : '项目恢复失败', { description: cause instanceof Error ? cause.message : String(cause) }) }
    })
  }
  const openRoot = async (project: ProjectManagementRecord) => {
    try { await desktopClient.openProjectRoot(project.id) }
    catch (cause) { toast.error('无法打开项目目录', { description: cause instanceof Error ? cause.message : String(cause) }) }
  }
  return { projects, loading, error, busyId, load, updatePermission, choosePath, setArchived, openRoot }
}

export function SecuritySettingsPage({ sidebarCollapsed, onSidebar, section }: { sidebarCollapsed: boolean; onSidebar: () => void; section?: SettingsSectionAnchor }) {
  useScrollToSettingsSection(section)
  const resource = useManagedProjects()
  const hooks = useLifecycleHooks()
  const [defaultPermission, setDefaultPermission] = useStoredPreference<PermissionMode>('fox.preferences.defaultPermission', 'ask')
  const [projectId, setProjectId] = useState<string | null>(null)
  const [hooksOpen, setHooksOpen] = useState(false)
  const { projects, busyId } = resource
  const activeProjects = projects.filter(project => project.archivedAt === null)
  const archivedProjects = projects.filter(project => project.archivedAt !== null)
  const openProject = projectId ? projects.find(item => item.id === projectId) ?? null : null
  const enabledHooks = hooks.items.filter(item => item.enabled).length

  return <WorkspacePage title="权限与安全" subtitle="项目权限与全局工具规则" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout title="权限与安全" description="管理项目权限和跨 Agent 的工具规则。高风险操作仍需确认，所有命中都会写入审计。">
      <SettingsGroup id={settingsSectionId('projects')} title="项目权限" aside={<Button type="button" variant="ghost" size="sm" disabled={resource.loading} onClick={() => void resource.load()}>{resource.loading ? <LoaderCircle className="animate-spin" /> : <RotateCcw />}刷新</Button>}>
        <SettingsRow title="新项目权限" description="只影响之后加入的新项目；建议「询问」，写入或运行命令前先确认">
          <PermissionSelect value={defaultPermission} label="新项目权限" onChange={setDefaultPermission} />
        </SettingsRow>
        {resource.error
          ? <div className="fox-settings-inline-alert" role="alert"><span><b>暂时无法读取项目列表</b><small title={resource.error}>{resource.error}</small></span><Button variant="outline" size="sm" onClick={() => void resource.load()}>重试</Button></div>
          : resource.loading && projects.length === 0
            ? <p className="fox-settings-empty"><LoaderCircle className="animate-spin" /> 正在检查目录、Git 状态与占用空间…</p>
            : activeProjects.length === 0 && <p className="fox-settings-empty">还没有项目，可从主界面的「添加项目」选择一个本机文件夹。</p>}
        <div className="fox-settings-providers">
          {activeProjects.map(project => <div className={cn('fox-settings-provider', projectId === project.id && 'is-open')} key={project.id} data-project-id={project.id}>
            <button type="button" className="fox-settings-provider-main" onClick={() => setProjectId(project.id)} aria-haspopup="dialog" aria-label={`查看 ${project.name}`}>
              <span className="fox-settings-provider-mark"><FolderOpen /></span>
              <span className="fox-settings-provider-text"><span className="fox-settings-provider-name"><b>{project.name}</b>{!project.pathExists && <Badge variant="destructive" className="fox-settings-pill">路径失效</Badge>}</span><small title={project.rootPath}>{project.rootPath}</small></span>
            </button>
            <PermissionSelect value={project.permissionMode} disabled={busyId === project.id} label={`${project.name} 执行方式`} onChange={value => void resource.updatePermission(project, value)} />
            <DropdownMenu modal={false}>
              <DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" className="fox-settings-provider-more" aria-label={`管理 ${project.name}`}><MoreHorizontal /></Button></DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItem disabled={!project.pathExists} onSelect={() => void resource.openRoot(project)}><ExternalLink />打开目录</DropdownMenuItem>
                <DropdownMenuItem disabled={busyId === project.id} onSelect={() => runAfterMenuClose(() => void resource.choosePath(project))}><FolderOpen />迁移目录</DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem disabled={busyId === project.id} onSelect={() => void resource.setArchived(project, true)}><Archive />归档</DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>)}
        </div>
      </SettingsGroup>

      {archivedProjects.length > 0 && <SettingsGroup title="已归档项目" aside={`${archivedProjects.length} 个`}>
        {archivedProjects.map(project => <SettingsRow key={project.id} title={project.name} description={<span title={project.rootPath}>{project.rootPath}</span>}>
          <Button variant="outline" size="sm" disabled={busyId === project.id} onClick={() => void resource.setArchived(project, false)}><RotateCcw />取消归档</Button>
        </SettingsRow>)}
      </SettingsGroup>}

      <SettingsGroup title="全局工具规则">
        <SettingsRow title="生命周期 Hook" description={hooks.items.length ? `${hooks.items.length} 条，${enabledHooks} 条启用。按工具名匹配并阻止、要求审批或附加说明，不执行脚本` : '按工具名匹配并阻止、要求审批或附加说明；Hook 不执行脚本，每次命中都写入审计'}>
          <Button type="button" variant="outline" size="sm" onClick={() => setHooksOpen(true)}><ShieldCheck />管理规则</Button>
        </SettingsRow>
        {hooks.error && <p className="fox-settings-error" role="alert">暂时无法读取 Hook 列表：{hooks.error} <Button variant="ghost" size="sm" onClick={() => void hooks.refresh()}>重试</Button></p>}
      </SettingsGroup>
    </SettingsPageLayout>

    <Sheet open={openProject !== null} onOpenChange={open => { if (!open) setProjectId(null) }}>
      <SheetContent side="right" className="fox-settings-sheet">
        {openProject && <>
          <SheetHeader className="fox-settings-sheet-head">
            <span className="fox-settings-provider-mark is-large"><FolderOpen /></span>
            <div className="fox-settings-sheet-title"><SheetTitle><span>{openProject.name}</span>{!openProject.pathExists && <Badge variant="destructive" className="fox-settings-pill">路径失效</Badge>}</SheetTitle><SheetDescription title={openProject.rootPath}>{openProject.rootPath}</SheetDescription></div>
          </SheetHeader>
          <div className="fox-settings-sheet-body">
            <section className="fox-settings-sheet-section">
              <div className="fox-settings-sheet-section-head"><h3>执行方式</h3></div>
              <PermissionSelect value={openProject.permissionMode} disabled={busyId === openProject.id} label={`${openProject.name} 执行方式`} className="is-wide" onChange={value => void resource.updatePermission(openProject, value)} />
              <p className="fox-settings-sheet-hint">{PERMISSION_LABELS[openProject.permissionMode]}：{openProject.permissionMode === 'read_only' ? 'Agent 只能读取项目文件。' : openProject.permissionMode === 'ask' ? '写入文件或运行命令前先询问。' : '项目内写入和低风险命令直接执行；高风险命令、项目外和受保护路径仍会询问。完全访问只能在对话里开启。'}</p>
            </section>
            <ProjectPermissionRules projectId={openProject.id} />
            <section className="fox-settings-sheet-section">
              <div className="fox-settings-sheet-section-head"><h3>目录</h3></div>
              <dl className="fox-settings-facts">
                <div><dt>版本状态</dt><dd>{openProject.isGitRepository ? openProject.gitBranch || 'Git' : '普通目录'}</dd></div>
                <div><dt>目录占用</dt><dd>{projectStorageLabel(openProject.diskBytes)}</dd></div>
                <div><dt>关联对话</dt><dd>{openProject.conversationCount}</dd></div>
                <div><dt>生成文件</dt><dd>{openProject.artifactCount}</dd></div>
                <div><dt>最近对话</dt><dd title={openProject.recentConversationTitle ?? undefined}>{openProject.recentConversationTitle ?? '暂无'}</dd></div>
                <div><dt>最近生成</dt><dd title={openProject.recentArtifactName ?? undefined}>{openProject.recentArtifactName ?? '暂无'}</dd></div>
              </dl>
            </section>
          </div>
          <div className="fox-settings-sheet-foot">
            <ProjectContactGrants projectId={openProject.id} />
            <Button type="button" variant="ghost" className="fox-settings-danger" disabled={busyId === openProject.id} onClick={() => { const project = openProject; setProjectId(null); void resource.setArchived(project, true) }}><Archive />归档项目</Button>
            <span className="is-grow" />
            <Button type="button" variant="outline" disabled={!openProject.pathExists} onClick={() => void resource.openRoot(openProject)}><ExternalLink />打开目录</Button>
            <Button type="button" variant="outline" disabled={busyId === openProject.id} onClick={() => void resource.choosePath(openProject)}>{busyId === openProject.id ? <LoaderCircle className="animate-spin" /> : <FolderOpen />}迁移目录</Button>
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
