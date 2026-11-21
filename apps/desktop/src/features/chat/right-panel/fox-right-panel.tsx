import { useProjectFiles } from './use-project-files'
import { BackgroundTasksPanel } from './background-tasks'
import { ChangesPanel } from './changes-panel'
import { OfficeFilePreview, officePreviewKind } from './office-file-preview'
import { type ActivityDetailPanelProps } from '../components/ActivityDetailPanel'
import { FOX_ASSISTANT_AVATAR } from '../components/FoxAssistantAvatar'
import { toolFailurePresentation } from '../run-status-facts'
import { formatFileSize } from '../runtime-artifacts'
import { childRunIsActive, childRunStatusLabel, isWebToolCall, normalizeBrowserUrl, webActivitySummary } from '../sidebar-model'
import { ProjectFileTree } from './project-file-tree'
import { RightPanelEmpty } from './right-panel-empty'
export { ProjectFileTree } from './project-file-tree'
import { WebPreview, WebPreviewBody, WebPreviewNavigation, WebPreviewNavigationButton } from '@/components/ai-elements/web-preview'
import { SideSection, SideCard } from '@/components/fox-ui/side-section'
import { Item, ItemContent, ItemTitle, ItemDescription } from '@/components/ui/item'
import { InputGroup, InputGroupInput } from '@/components/ui/input-group'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { FoxDisclosure } from '@/components/fox-ui/fox-disclosure'
import './right-panel.css'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { type ArtifactActionResponse, type ArtifactApplication, type ArtifactInspectResponse, type ArtifactRecord, type ChildRunRecord, type ConversationDetail, type ConversationMessage, type DesktopErrorDetails, type ProjectFileActionResponse, type ProjectFileEntry, type ProjectFilePreview, type TaskEvidenceRecord, type ToolCallRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { Activity, AlertTriangle, ArrowLeft, BookOpen, Bot, Check, ChevronDown, ChevronRight, ChevronUp, CircleStop, Copy, ExternalLink, FileCode, FileEdit, FileImage, FileSpreadsheet, FileText, FolderOpen, Folders, Globe2, History, LayoutGrid, Ellipsis, ListTodo, LoaderCircle, type LucideIcon, Maximize2, Minimize2, PanelRight, Plus, RotateCcw, Search, ShieldCheck, Terminal, X } from 'lucide-react'
import { type ReactNode, Suspense, lazy, useEffect, useMemo, useRef, useState } from 'react'
import { isChangeOrCommandTool } from '../composer/fox-composer'
import { useDeveloperUi } from '../developer-ui'
import { type ChildAgentSelection, MarkdownResponse, type OpenFileTab, type RightMode } from '../fox-shared'
import { runtimeProcess } from '../timeline/fox-runtime-process-model'
import { Workbench } from '../workbench'
import { userFacingCause } from '@/features/chat/send-failure-notice'
import { managedFilesFingerprintOf, useManagedFiles, type ManagedFilesController } from '../components/managed-files/use-managed-files'
import { useManagedFileUnseen } from '../components/managed-files/use-managed-file-unseen'
import { toolRecordPresentation } from '../tool-record-presentation'
import { managedFilesRefreshKey, managedFilePathKey, type ManagedFileGroup } from '../components/managed-files/model'

const ActivityDetailPanel = lazy(() => import('../components/ActivityDetailPanel').then((module) => ({ default: module.ActivityDetailPanel })))
const TerminalPanel = lazy(() => import('./terminal-panel').then(module => ({ default: module.TerminalPanel })))
const HtmlFilePreview = lazy(() => import('../html-file-preview').then((module) => ({ default: module.HtmlFilePreview })))


function normalizedProjectPath(value: string) {
  return value.replace(/\\/g, '/').replace(/^\.\//, '').replace(/\/{2,}/g, '/').replace(/\/$/, '')
}

function projectDisplayPath(value: string, projectRoot: string) {
  const normalizedValue = normalizedProjectPath(value)
  const normalizedRoot = normalizedProjectPath(projectRoot)
  const valueKey = normalizedValue.toLocaleLowerCase()
  const rootKey = normalizedRoot.toLocaleLowerCase()
  const relativePath = valueKey === rootKey
    ? ''
    : valueKey.startsWith(`${rootKey}/`)
      ? normalizedValue.slice(normalizedRoot.length + 1)
      : normalizedValue
  return `/${relativePath.replace(/^\/+/, '')}`
}

export function createOpenFileTab(path: string, artifactId: string | null = null): OpenFileTab {
  const normalizedPath = normalizedProjectPath(path)
  return {
    id: normalizedPath.toLocaleLowerCase(),
    path: normalizedPath,
    name: normalizedPath.split('/').at(-1) ?? normalizedPath,
    artifactId,
  }
}

export function artifactProjectPath(artifact: ArtifactRecord, projectRoot: string, entries: ProjectFileEntry[]) {
  const storagePath = normalizedProjectPath(artifact.storagePath)
  const normalizedRoot = normalizedProjectPath(projectRoot)
  const storageKey = storagePath.toLocaleLowerCase()
  const rootKey = normalizedRoot.toLocaleLowerCase()
  const relativePath = storageKey.startsWith(`${rootKey}/`)
    ? storagePath.slice(normalizedRoot.length + 1)
    : storagePath
  const relativeKey = relativePath.toLocaleLowerCase()
  return entries.find((entry) => {
    if (entry.isDirectory) return false
    const entryKey = normalizedProjectPath(entry.path).toLocaleLowerCase()
    return entryKey === relativeKey || storageKey.endsWith(`/${entryKey}`)
  })?.path ?? relativePath
}

function fileKindIcon(name: string): LucideIcon {
  if (/\.(xlsx?|csv|tsv)$/i.test(name)) return FileSpreadsheet
  if (/\.(png|jpe?g|gif|webp|svg|bmp|ico)$/i.test(name)) return FileImage
  if (/\.(html?|md|json|[cm]?[jt]sx?|py|rs|css|ya?ml|toml|sh|ps1|xml|sql)$/i.test(name)) return FileCode
  return FileText
}

function fileFolderLabel(displayPath: string) {
  const parts = displayPath.split('/').filter(Boolean)
  parts.pop()
  return parts.length ? parts.join('/') : '项目根目录'
}

function FileStatusLine({ tone, children }: { tone: 'info' | 'warning' | 'error'; children: ReactNode }) {
  const Icon = tone === 'error' ? AlertTriangle : tone === 'warning' ? History : Check
  return <p className={`fox-file-head-status is-${tone}`} role={tone === 'error' ? 'alert' : 'status'}><Icon size={13} aria-hidden="true" /><span>{children}</span></p>
}

function FileHeaderCard({ name, displayPath, size, status, actions }: { name: string; displayPath: string; size?: string; status?: ReactNode; actions: ReactNode }) {
  return <div className="fox-file-head"><div className="fox-file-head-main"><strong title={[displayPath, size].filter(Boolean).join(' · ')}>{name}</strong><div className="fox-file-head-actions">{actions}</div></div>{status}</div>
}

type FileOpenAction = 'open' | 'open_with' | 'reveal' | 'copy_path'
function FileOpenActions({ busy, canOpen = true, canReveal = true, canCopyPath = true, applications, onAction, managed, group }: { busy: boolean; canOpen?: boolean; canReveal?: boolean; canCopyPath?: boolean; applications: ArtifactApplication[]; onAction: (action: FileOpenAction, applicationId?: string) => void; managed?: ManagedFilesController | null; group?: ManagedFileGroup }) {
  return <>
    <Tooltip><TooltipTrigger asChild><button type="button" className="fox-panel-icon-button is-small" aria-label="打开" disabled={busy || !canOpen} onClick={() => onAction('open')}><ExternalLink /></button></TooltipTrigger><TooltipContent>用默认应用打开</TooltipContent></Tooltip>
    <Tooltip><TooltipTrigger asChild><button type="button" className="fox-panel-icon-button is-small" aria-label="打开所在文件夹" disabled={busy || !canReveal} onClick={() => onAction('reveal')}><FolderOpen /></button></TooltipTrigger><TooltipContent>打开所在文件夹</TooltipContent></Tooltip>
    <DropdownMenu><Tooltip><TooltipTrigger asChild><DropdownMenuTrigger asChild><button type="button" className="fox-panel-icon-button is-small" aria-label="更多文件操作" disabled={busy}><Ellipsis /></button></DropdownMenuTrigger></TooltipTrigger><TooltipContent>更多</TooltipContent></Tooltip><DropdownMenuContent align="end" className="fox-result-open-menu">
      {applications.map(application => <DropdownMenuItem key={application.id} onSelect={() => onAction('open_with', application.id)}><Terminal />{application.label}</DropdownMenuItem>)}
      {applications.length > 0 && <DropdownMenuSeparator />}
      <DropdownMenuItem disabled={!canCopyPath} onSelect={() => onAction('copy_path')}><Copy />复制路径</DropdownMenuItem>
    </DropdownMenuContent></DropdownMenu>
  </>
}

function FilePreviewState({ icon: Icon, spinning = false, title, children, action }: { icon: LucideIcon; spinning?: boolean; title: string; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className="fox-file-preview-state" role={spinning ? 'status' : undefined}>
      <Icon size={20} className={spinning ? 'animate-spin' : undefined} aria-hidden="true" />
      <strong>{title}</strong>
      {children && <span>{children}</span>}
      {action}
    </div>
  )
}

function FilePreviewContent({ previewKey, name, content, html }: { previewKey: string; name: string; content: string; html: boolean }) {
  return (
    <>
      {html
        ? <Suspense fallback={<FilePreviewState icon={LoaderCircle} spinning title="正在加载预览" />}><HtmlFilePreview key={previewKey} name={name} content={content} /></Suspense>
        : <pre className="fox-file-text">{content}</pre>}
    </>
  )
}

function ProjectFilePreviewPane({ conversationId, path, displayPath, managed, group }: { managed?: ManagedFilesController | null; group?: ManagedFileGroup; conversationId: string; path: string; displayPath: string }) {
  const developerUi = useDeveloperUi()
  const office = officePreviewKind(path)
  const [preview, setPreview] = useState<ProjectFilePreview | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [reload, setReload] = useState(0)
  const [applications, setApplications] = useState<ArtifactApplication[]>([])
  const [busyAction, setBusyAction] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    setPreview(null)
    setLoading(!office)
    setError(null)
    if (!office) {
      void desktopClient.readProjectFile(conversationId, path)
        .then((response) => { if (!cancelled) setPreview(response) })
        .catch((cause) => { if (!cancelled) setError(userFacingCause(cause)) })
        .finally(() => { if (!cancelled) setLoading(false) })
    }
    void desktopClient.projectFileAction(conversationId, path, 'inspect')
      .then((response) => { if (!cancelled) setApplications(response.applications) })
      .catch(() => { if (!cancelled) setApplications([]) })
    return () => { cancelled = true }
  }, [conversationId, path, office, reload, group?.versions[0]?.id])

  const runAction = async (action: FileOpenAction, applicationId?: string) => {
    if (busyAction) return
    setBusyAction(action)
    try {
      const response = await desktopClient.projectFileAction(conversationId, path, action, applicationId)
      setApplications(response.applications)
    } catch (cause) {
      toast.error(developerUi ? desktopErrorDetails(cause).message : '文件操作没有完成')
    } finally {
      setBusyAction(null)
    }
  }

  const name = preview?.name || path.split(/[\\/]/).at(-1) || path
  return (
    <div className="fox-file-preview fox-project-file-preview">
      <FileHeaderCard name={name} displayPath={displayPath} size={preview ? formatFileSize(preview.byteSize) : undefined} status={<>{preview?.truncated && <FileStatusLine tone="info">预览已截断，只显示文件开头部分。</FileStatusLine>}</>}
        actions={<FileOpenActions managed={managed} group={group} busy={Boolean(busyAction)} applications={applications} onAction={(action, applicationId) => void runAction(action, applicationId)} />} />
      <SideCard className="fox-file-body">
        {office ? <OfficeFilePreview key={`${conversationId}:${path}:${group?.versions[0]?.id ?? ''}`} conversationId={conversationId} source={{ kind: 'project', path }} filename={name} /> : loading
          ? <FilePreviewState icon={LoaderCircle} spinning title="正在读取文件" />
          : error
            ? <FilePreviewState icon={AlertTriangle} title="预览失败" action={<Button size="sm" variant="outline" onClick={() => setReload((value) => value + 1)}>重试</Button>}>{developerUi ? error : '文件没有预览出来，请重试。'}</FilePreviewState>
            : <FilePreviewContent previewKey={path} name={displayPath} content={preview?.content ?? ''} html={/\.html?$/i.test(path) && Boolean(preview?.content)} />}
      </SideCard>
    </div>
  )
}

function artifactLoadErrorCopy(error: DesktopErrorDetails, developerUi: boolean) {
  if (developerUi) return { title: '文件没有打开', text: `${error.code}：${error.message}` }
  switch (error.code) {
    case 'artifact.integrity_mismatch': return { title: '文件已经变了', text: '回答之后这个文件又被修改过，这一版不能再预览。' }
    case 'artifact.file_missing':
    case 'artifact.file_unavailable':
    case 'artifact.not_regular_file': return { title: '找不到这个文件', text: '它可能被移动、改名或删除了。' }
    case 'artifact.root_unavailable':
    case 'artifact.path_denied': return { title: '没有权限读取', text: '这个文件不在当前对话可以访问的文件夹里。' }
    case 'artifact.file_too_large': return { title: '文件太大', text: '可以用上面的「打开」在其他软件里查看。' }
    default: return { title: '文件没有打开', text: error.retryable ? '可以稍后再试。' : '这个文件现在不能预览。' }
  }
}


function ArtifactFilePreview({ detail, artifactId, displayPath, projectPath, managed, group }: { managed?: ManagedFilesController | null; group?: ManagedFileGroup; detail: ConversationDetail | null; artifactId?: string | null; displayPath: string; projectPath?: string }) {
  const developerUi = useDeveloperUi()
  const conversationId = detail?.conversation.id ?? null
  const artifact = detail?.artifacts.find((item) => item.id === artifactId) ?? null
  const [inspection, setInspection] = useState<ArtifactInspectResponse | null>(null)
  const [loading, setLoading] = useState(false)
  const [reload, setReload] = useState(0)
  const [busyAction, setBusyAction] = useState<ArtifactActionResponse['action'] | null>(null)
  const [loadError, setLoadError] = useState<DesktopErrorDetails | null>(null)
  const [actionError, setActionError] = useState<DesktopErrorDetails | null>(null)

  useEffect(() => {
    setActionError(null)
    if (!conversationId || !artifact) {
      setInspection(null)
      setLoadError(null)
      return
    }
    let cancelled = false
    setInspection(null)
    setLoading(true)
    setLoadError(null)
    void desktopClient.inspectArtifact(conversationId, artifact.id)
      .then((response) => { if (!cancelled) setInspection(response) })
      .catch((cause) => { if (!cancelled) setLoadError(desktopErrorDetails(cause)) })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [artifact?.id, conversationId, reload, group?.versions[0]?.id])

  const runAction = async (action: ArtifactActionResponse['action'], applicationId?: string) => {
    if (!conversationId || !artifact || busyAction) return
    setBusyAction(action)
    setActionError(null)
    try {
      const response = await desktopClient.artifactAction(conversationId, artifact.id, action, applicationId)
      setInspection(response)
    } catch (cause) {
      setActionError(desktopErrorDetails(cause))
    } finally {
      setBusyAction(null)
    }
  }

  if (!artifact) {
    return <RightPanelEmpty icon={FileText}>这段对话还没有可预览的文件。</RightPanelEmpty>
  }

  // A stale artifact falls back to the current project file.
  if (loadError?.code === 'artifact.integrity_mismatch' && conversationId && projectPath) {
    return <ProjectFilePreviewPane managed={managed} group={group} conversationId={conversationId} path={projectPath} displayPath={displayPath} />
  }

  const status = actionError
    ? <FileStatusLine tone="error">{developerUi ? actionError.message : actionError.retryable ? '这次没有完成，可以稍后再试。' : '这个文件现在不能这样打开。'}</FileStatusLine>
    : null
  const loadCopy = loadError ? artifactLoadErrorCopy(loadError, developerUi) : null
  const preview = inspection?.preview
  return (
    <div className="fox-file-preview fox-artifact-file-preview">
      <FileHeaderCard name={artifact.displayName || displayPath.split('/').at(-1) || displayPath} displayPath={displayPath}
        size={formatFileSize(artifact.byteSize)} status={<>{status}{preview?.truncated && <FileStatusLine tone="info">预览已截断，只显示前 512 KB。</FileStatusLine>}</>}
        actions={<FileOpenActions managed={managed} group={group} busy={loading || Boolean(busyAction)} canOpen={Boolean(inspection?.capabilities.open)} canReveal={Boolean(inspection?.capabilities.reveal)} canCopyPath={Boolean(inspection?.capabilities.copyPath)} applications={inspection?.applications ?? []} onAction={(action, applicationId) => void runAction(action, applicationId)} />} />
      <SideCard className="fox-file-body">
        {loading
          ? <FilePreviewState icon={LoaderCircle} spinning title="正在读取文件" />
          : loadCopy
            ? <FilePreviewState icon={AlertTriangle} title={loadCopy.title} action={loadError?.retryable ? <Button size="sm" variant="outline" onClick={() => setReload((value) => value + 1)}>重试</Button> : undefined}>{loadCopy.text}</FilePreviewState>
            : conversationId && inspection && officePreviewKind(artifact.displayName)
              ? <OfficeFilePreview key={artifact.id} conversationId={conversationId} source={{ kind: 'artifact', artifactId: artifact.id }} filename={artifact.displayName} />
            : preview?.available && preview.content !== null
              ? <FilePreviewContent previewKey={artifact.id} name={artifact.displayName} content={preview.content} html={preview.kind === 'html'} />
              : <FilePreviewState icon={fileKindIcon(artifact.displayName)} title={preview?.reason ?? '暂时没有可用预览'}>可以用上面的「打开」查看原文件。</FilePreviewState>}
      </SideCard>
    </div>
  )
}
function toolResultPreview(tool: ToolCallRecord) {
  // A failed or denied record reads as the same whitelist sentence the process
  // row shows: the Host's raw diagnostic (`errorMessage`, codes, frozen-scope
  // wording) stays in the durable record. The category, never the prose, decides
  // the sentence — and a missing category stays deliberately limited.
  if (tool.status === 'failed' || tool.status === 'denied') {
    const failure = toolFailurePresentation(tool.toolName, tool.result)
    return [failure.summary, failure.hint].filter(Boolean).join(' · ')
  }
  if (typeof tool.result === 'string') return tool.result.trim().slice(0, 420)
  if (tool.result === null || tool.result === undefined) return '尚未返回结果'
  try {
    return JSON.stringify(tool.result, null, 2).slice(0, 420)
  } catch {
    return String(tool.result).slice(0, 420)
  }
}

function BrowserPanel({ detail }: { detail: ConversationDetail | null }) {
  const [activeTab, setActiveTab] = useState<'page' | 'activity'>('page')
  const [address, setAddress] = useState('')
  const [browserUrl, setBrowserUrl] = useState('')
  const [frameVersion, setFrameVersion] = useState(0)
  const [selectedCall, setSelectedCall] = useState<string | null>(null)
  const webCalls = useMemo(
    () => (detail?.toolCalls ?? []).filter(isWebToolCall).sort((left, right) => right.startedAt - left.startedAt),
    [detail?.toolCalls],
  )

  const navigateTo = (value: string) => {
    const normalized = normalizeBrowserUrl(value)
    if (!normalized) {
      toast.error('请输入有效的 http 或 https 网页地址')
      return
    }
    setAddress(normalized)
    setBrowserUrl(normalized)
    setFrameVersion((version) => version + 1)
    setActiveTab('page')
  }
  const openExternal = async () => {
    if (!browserUrl) return
    try {
      await desktopClient.openExternalUrl(browserUrl)
    } catch (cause) {
      toast.error(cause instanceof Error ? cause.message : String(cause))
    }
  }

  return (
    <WebPreview className="fox-browser-panel">
      <Tabs value={activeTab} onValueChange={value => setActiveTab(value as 'page' | 'activity')}><TabsList className="fox-browser-tabs" aria-label="网页记录">
        <TabsTrigger value="page"><Globe2 size={13} />网页预览</TabsTrigger>
        <TabsTrigger value="activity"><Activity size={13} />网络记录<small>{webCalls.length}</small></TabsTrigger>
      </TabsList></Tabs>
      {activeTab === 'page' && <WebPreviewNavigation className="fox-browser-address">
        <WebPreviewNavigationButton disabled={!browserUrl} tooltip="刷新" onClick={() => setFrameVersion((version) => version + 1)}><RotateCcw size={14} /></WebPreviewNavigationButton>
        <form onSubmit={(event) => { event.preventDefault(); navigateTo(address) }}>
          <InputGroup><InputGroupInput aria-label="网页地址" value={address} onChange={(event) => setAddress(event.target.value)} placeholder="输入网址，例如 example.com" spellCheck={false} /></InputGroup>
        </form>
        <WebPreviewNavigationButton disabled={!browserUrl} tooltip="在系统浏览器中打开" onClick={() => void openExternal()}><ExternalLink size={14} /></WebPreviewNavigationButton>
      </WebPreviewNavigation>}
      {activeTab === 'page'
        ? browserUrl
          ? <div className="fox-browser-frame-wrap"><WebPreviewBody key={`${browserUrl}:${frameVersion}`} className="fox-browser-frame" src={browserUrl} /></div>
          : <RightPanelEmpty icon={Globe2}>输入网址即可预览网页。</RightPanelEmpty>
        : webCalls.length > 0
          ? <ScrollArea className="fox-browser-activity"><div>{webCalls.map((tool) => {
              const summary = webActivitySummary(tool)
              const status = tool.status === 'completed' ? '已完成' : tool.status === 'failed' ? '失败' : tool.status === 'denied' ? '已拒绝' : tool.status === 'cancelled' ? '已取消' : '执行中'
              return <div key={tool.id} className="fox-browser-network-row">
                <button type="button" className="fox-record-row" aria-expanded={selectedCall === tool.id} onClick={() => setSelectedCall(current => current === tool.id ? null : tool.id)}>
                  <Globe2 /><span className="fox-record-copy"><strong>{summary.label}</strong><small title={summary.target}>{summary.target}</small></span><small className={`fox-side-status is-${tool.status}`}>{status}</small>
                </button>
                {selectedCall === tool.id && <div className="fox-record-result"><pre>{toolResultPreview(tool)}</pre>{summary.navigableUrl && <button type="button" className="fox-side-text-button" onClick={() => navigateTo(summary.navigableUrl!)}>预览网页</button>}</div>}
              </div>
            })}</div></ScrollArea>
          : <RightPanelEmpty icon={Activity}>这段对话还没有联网记录。</RightPanelEmpty>}
    </WebPreview>
  )
}

function ChildRunStatusIcon({ run }: { run: ChildRunRecord }) {
  if (childRunIsActive(run)) return <LoaderCircle className="animate-spin" size={13} />
  if (run.status === 'completed') return <Check size={13} />
  if (run.status === 'cancelled' || run.status === 'interrupted') return <CircleStop size={13} />
  return <AlertTriangle size={13} />
}

type ChildTranscriptItem =
  | { kind: 'message'; id: string; createdAt: number; role: 'user' | 'assistant'; content: string }
  | { kind: 'tool'; id: string; createdAt: number; toolName: string; status: string; input?: unknown }

function childTranscriptItems(detail: ConversationDetail | null): ChildTranscriptItem[] {
  if (!detail) return []
  const messages: ChildTranscriptItem[] = detail.messages
    .filter((message): message is ConversationMessage & { role: 'user' | 'assistant' } => (
      (message.role === 'user' || message.role === 'assistant') && Boolean(message.content.trim())
    ))
    .map((message) => ({
      kind: 'message',
      id: message.id,
      createdAt: message.createdAt,
      role: message.role,
      content: message.content.trim(),
    }))
  const tools: ChildTranscriptItem[] = detail.toolCalls.map((tool) => ({
    kind: 'tool',
    id: tool.id,
    createdAt: tool.startedAt,
    toolName: tool.toolName,
    input: tool.input,
    status: tool.status,
  }))
  return [...messages, ...tools].sort((left, right) => left.createdAt - right.createdAt)
}

function childToolStatusLabel(status: string) {
  if (status === 'completed') return '已完成'
  if (status === 'failed') return '失败'
  return '执行中'
}

function childRunFailureTitle(run: ChildRunRecord) {
  if (/duration|timeout/i.test(run.errorCode ?? '') || /超时|time limit|timed out/i.test(run.errorMessage ?? '')) return '运行超时'
  if (run.status === 'interrupted') return '运行中断'
  return '运行失败'
}

function childRunFailureMessage(run: ChildRunRecord, developerUi: boolean) {
  // The concrete execution cap is internal accounting: the user reads that the
  // sub-task did not finish in time and that its result was not used.
  if (childRunFailureTitle(run) === '运行超时') return '子任务未在限定时间内完成，结果未采用'
  const human = run.status === 'interrupted' ? '运行意外中断，本次结果未采用' : '协作助手没有完成，结果未采用'
  if (!developerUi) return human
  return run.errorMessage ?? human
}

function childAgentVisualQaEnabled() {
  return import.meta.env.DEV && new URLSearchParams(window.location.search).get('preview') === 'child-agent'
}

export function childAgentVisualQaPanelWidth() {
  if (!childAgentVisualQaEnabled()) return null
  const requested = Number(new URLSearchParams(window.location.search).get('panelWidth'))
  return Number.isFinite(requested) && requested > 0 ? Math.min(760, Math.max(280, requested)) : 540
}

function childAgentVisualQaRuns(): ChildRunRecord[] {
  if (!childAgentVisualQaEnabled()) return []
  const now = Date.now()
  const base: ChildRunRecord = {
    id: 'preview-delegation-1',
    parentRunId: 'preview-parent-run',
    childRunId: 'preview-child-run-1',
    childConversationId: 'preview-conversation-1',
    workerAgentId: 'fox-debugger',
    workerAgentName: 'Fox 调试专家',
    objective: '审查 quicksort.py，找出所有 Bug，简洁输出，每个 Bug 一行标题 + 一行修复建议。',
    context: '请按 critical / high / medium / low 分级，并给出能够直接执行的修改建议。',
    teamRunId: null,
    teamMemberId: null,
    allowedTools: ['read_file', 'search_code', 'run_tests'],
    status: 'failed',
    depth: 1,
    budget: { maxDurationMs: 600_000, maxTotalTokens: 80_000, maxOutputTokens: 6_000, maxToolCalls: 12 },
    resultText: '已读取并审查代码，以下是按严重级别排列的 Bug 清单（节选）：\n\n• Critical：quicksort_inplace 递归深度无界，可能导致递归深度异常。\n• High：_sort 区间判断存在潜在偏差，缺少对 equal 分区的递归处理。\n• Medium：_inplace 缺少对空数组和单元素的提前返回保护。\n• Low：命名、注释和类型注解可以进一步统一。',
    inputTokens: 52_410,
    outputTokens: 12_570,
    totalTokens: 64_980,
    toolCallCount: 3,
    errorCode: 'runtime.duration_budget_exceeded',
    errorMessage: '超过 10 分钟执行上限，结果未采用',
    createdAt: now - 610_000,
    startedAt: now - 608_000,
    finishedAt: now - 8_000,
  }
  return [
    base,
    { ...base, id: 'preview-delegation-2', childRunId: 'preview-child-run-2', childConversationId: 'preview-conversation-2', objective: '审查 quicksort.py 代码，验证修复后的边界条件。', workerAgentName: 'Fox 代码审查专家', status: 'completed', resultText: '边界条件验证通过。', totalTokens: 18_420, toolCallCount: 5, errorCode: null, errorMessage: null, finishedAt: now - 180_000 },
    { ...base, id: 'preview-delegation-3', childRunId: 'preview-child-run-3', childConversationId: 'preview-conversation-3', objective: '对 quicksort.py 进行全面测试并记录失败用例。', workerAgentName: 'Fox 测试专家', status: 'completed', resultText: '已完成测试并返回用例摘要。', totalTokens: 21_760, toolCallCount: 7, errorCode: null, errorMessage: null, finishedAt: now - 420_000 },
    { ...base, id: 'preview-delegation-4', childRunId: 'preview-child-run-4', childConversationId: 'preview-conversation-4', objective: '检查排序实现的性能退化风险。', workerAgentName: 'Fox 性能专家', status: 'running', resultText: null, totalTokens: 8_320, toolCallCount: 2, errorCode: null, errorMessage: null, finishedAt: null },
  ]
}

const childAgentAvatars = [FOX_ASSISTANT_AVATAR, FOX_ASSISTANT_AVATAR, FOX_ASSISTANT_AVATAR, FOX_ASSISTANT_AVATAR]

function ChildAgentPanel({ detail, onOpenChildAgent }: { detail: ConversationDetail | null; onOpenChildAgent: (run: ChildRunRecord, avatarIndex: number) => void }) {
  const previewRuns = useMemo(childAgentVisualQaRuns, [])
  const runs = useMemo(
    () => [...(detail?.childRuns?.length ? detail.childRuns : previewRuns)].sort((left, right) => right.createdAt - left.createdAt),
    [detail?.childRuns, previewRuns],
  )
  return <div className="fox-side-body fox-agent-panel">
    {runs.length === 0 ? <RightPanelEmpty icon={Bot}>这段对话还没有协作助手。</RightPanelEmpty>
      : <nav aria-label="协作助手列表">{[true, false].map(active => {
        const group = runs.filter(run => childRunIsActive(run) === active)
        return group.length > 0 && <SideSection key={String(active)} title={active ? '进行中' : '已结束'} count={group.length}><SideCard>{group.map(run => {
          const index = runs.indexOf(run)
          return <Item asChild className="fox-side-row" key={run.childRunId}><button type="button" className="fox-agent-list-item" aria-label={`打开 ${run.workerAgentName} 消息`} onClick={() => onOpenChildAgent(run, index)}>
            <img className="fox-agent-list-avatar" src={childAgentAvatars[index % childAgentAvatars.length]} alt="" />
            <ItemContent><ItemTitle>{run.workerAgentName}</ItemTitle><ItemDescription>{run.status === 'failed' || run.status === 'interrupted' ? <span className="fox-side-status is-failed">{childRunFailureTitle(run)} · {childRunFailureMessage(run, false)}</span> : run.objective}</ItemDescription></ItemContent>
            <span className={`fox-side-status is-${run.status}`}>{childRunIsActive(run) && <LoaderCircle size={13} className="animate-spin" />}{childRunStatusLabel(run.status)}</span><ChevronRight size={14} />
          </button></Item>
        })}</SideCard></SideSection>
      })}</nav>}
  </div>
}

function ChildAgentConversationPanel({ run, avatarIndex, onBack }: { run: ChildRunRecord; avatarIndex: number; onBack: () => void }) {
  const developerUi = useDeveloperUi()
  const [childDetail, setChildDetail] = useState<ConversationDetail | null>(null)
  const [childDetailLoading, setChildDetailLoading] = useState(false)
  const [childDetailError, setChildDetailError] = useState<string | null>(null)
  const transcript = useMemo(() => {
    const storedItems = childTranscriptItems(childDetail)
    if (storedItems.length || !run.childConversationId.startsWith('preview-conversation-')) return storedItems
    const startedAt = run.startedAt ?? run.createdAt
    const previewItems: ChildTranscriptItem[] = [{
      kind: 'message',
      id: `${run.childRunId}:objective`,
      createdAt: run.createdAt,
      role: 'user',
      content: run.objective,
    }]
    const tools = (run.allowedTools ?? []).slice(0, Math.max(1, Math.min(run.toolCallCount, 3)))
    tools.forEach((toolName, index) => previewItems.push({
      kind: 'tool',
      id: `${run.childRunId}:tool:${index}`,
      createdAt: startedAt + ((index + 1) * 30_000),
      toolName,
      status: childRunIsActive(run) && index === tools.length - 1 ? 'running' : 'completed',
    }))
    if (run.resultText) previewItems.push({
      kind: 'message',
      id: `${run.childRunId}:result`,
      createdAt: run.finishedAt ?? Date.now(),
      role: 'assistant',
      content: run.resultText,
    })
    return previewItems
  }, [childDetail, run])

  useEffect(() => {
    if (run.childConversationId.startsWith('preview-conversation-')) {
      setChildDetail(null)
      setChildDetailLoading(false)
      setChildDetailError(null)
      return
    }
    let cancelled = false
    let refreshTimer: number | undefined
    setChildDetail(null)
    setChildDetailLoading(true)
    setChildDetailError(null)
    const load = async (showLoading: boolean) => {
      if (showLoading) setChildDetailLoading(true)
      try {
        const value = await desktopClient.loadConversation(run.childConversationId)
        if (!cancelled) {
          setChildDetail(value)
          setChildDetailError(null)
        }
      } catch (cause) {
        if (!cancelled) setChildDetailError(desktopErrorDetails(cause).message)
      } finally {
        if (!cancelled && showLoading) setChildDetailLoading(false)
      }
    }
    void load(true)
    if (childRunIsActive(run)) refreshTimer = window.setInterval(() => { void load(false) }, 1_500)
    return () => {
      cancelled = true
      if (refreshTimer !== undefined) window.clearInterval(refreshTimer)
    }
  }, [run.childConversationId, run.status])

  return (
    <section className="fox-agent-conversation is-tab" aria-label={`${run.workerAgentName}只读消息记录`}>
      <header className="fox-agent-conversation-head">
        <button type="button" className="fox-agent-conversation-back" aria-label="返回协作助手列表" onClick={onBack}><ArrowLeft /></button>
        <img src={childAgentAvatars[avatarIndex % childAgentAvatars.length]} alt="" /><div><strong title={run.objective}>{run.workerAgentName}</strong><small>只读 · 消息会自动更新</small></div>
        <span className={`is-${run.status}`}><ChildRunStatusIcon run={run} />{childRunStatusLabel(run.status)}</span>
      </header>
      <div className="fox-agent-conversation-scroll">
        {childDetailLoading && !childDetail
          ? <div className="fox-agent-inline-state"><LoaderCircle className="animate-spin" />正在读取协作助手消息</div>
          : childDetailError && !childDetail
            ? <div className="fox-agent-inline-state is-error"><AlertTriangle />{childDetailError}</div>
            : (['user', 'tool', 'assistant'] as const).map(kind => {
              const items = transcript.filter(item => kind === 'tool' ? item.kind === 'tool' : item.kind === 'message' && item.role === kind)
              if (!items.length) return null
              return <SideSection key={kind} title={kind === 'user' ? 'Fox 交给它的任务' : kind === 'tool' ? '过程' : '回复'}><SideCard>{items.map(item => {
                if (item.kind === 'tool') {
                  const { action, target, Icon } = toolRecordPresentation({ ...item, input: item.input ?? {} })
                  return <div className="fox-record-row" key={item.id}><Icon /><span className="fox-record-copy"><strong>{action}</strong>{target && <small>{target}</small>}{developerUi && <small>{item.toolName}</small>}</span><small className={`fox-side-status is-${item.status}`}>{childToolStatusLabel(item.status)}</small></div>
                }
                return <div className="fox-agent-section-text" key={item.id}>{item.role === 'user' ? <p>{item.content}</p> : <MarkdownResponse>{item.content}</MarkdownResponse>}</div>
              })}</SideCard></SideSection>
            })}
        {!childDetailLoading && !childDetailError && transcript.length === 0 && <p className="fox-agent-transcript-empty">这个协作助手还没有可见消息。</p>}
        {(run.status === 'failed' || run.status === 'interrupted') && <div className="fox-agent-run-error" role="alert">
          <AlertTriangle />
          <div><strong>{childRunFailureTitle(run)}</strong><p>{childRunFailureMessage(run, developerUi)}</p></div>
        </div>}
      </div>
    </section>
  )
}

function ContextContent({ onUnbindKnowledge, knowledgeBusy, mode, detail, fileTabs, activeFileTabId, activity, managed, onOpenFile, onOpenEvidence, onOpenChildAgent }: { onUnbindKnowledge?: (binding: ConversationDetail["knowledgeBindings"][number]) => Promise<boolean>; knowledgeBusy?: boolean; mode: RightMode; detail: ConversationDetail | null; fileTabs: OpenFileTab[]; activeFileTabId: string | null; activity: Omit<ActivityDetailPanelProps, "detail">; managed: ManagedFilesController | null; onOpenFile: (path: string, artifactId: string | null) => void; onOpenEvidence?: (evidence: TaskEvidenceRecord) => void; onOpenChildAgent: (run: ChildRunRecord, avatarIndex: number) => void }) {
  const conversationId = detail?.conversation.id ?? null
  const projectRoot = detail?.conversation.projectRoot
  const [fileRefresh, setFileRefresh] = useState(0)
  const [fileQuery, setFileQuery] = useState('')
  useEffect(() => { setFileQuery('') }, [conversationId, projectRoot])
  const completedWrites = (detail?.toolCalls ?? []).filter(tool => tool.status === 'completed' && isChangeOrCommandTool(tool.toolName)).map(tool => tool.id).join('|')
  const files = useProjectFiles(conversationId, mode === 'files' && Boolean(projectRoot), `${fileRefresh}:${completedWrites}`, fileQuery, projectRoot || '')
  const { entries: projectFiles, loading: fileLoading, error: fileError } = files

  const activeFileTab = fileTabs.find((tab) => tab.id === activeFileTabId) ?? null
  const selectedArtifact = detail?.artifacts.find((item) => item.id === activeFileTab?.artifactId) ?? null

  if (mode === 'files') {
    if (!conversationId || !detail || !projectRoot) return <RightPanelEmpty icon={Folders}>尚未选择项目。</RightPanelEmpty>
    const selectFile = (path: string) => {
      const matchingArtifact = detail.artifacts.find((artifact) => artifactProjectPath(artifact, projectRoot, projectFiles) === path)
      onOpenFile(path, matchingArtifact?.id ?? null)
    }
    const openedFilePath = activeFileTab?.path
    const openedFileDisplayPath = openedFilePath ? projectDisplayPath(openedFilePath, projectRoot) : ''
    const versionGroups = managed?.state.status === 'ready' ? managed.state.groups : []
    const projectKey = managedFilePathKey(projectRoot || '')
    const groupFor = (path: string) => versionGroups.find(group => managedFilePathKey(group.storagePath) === managedFilePathKey(path) || managedFilePathKey(group.storagePath) === managedFilePathKey(`${projectKey}/${path}`))
    const activeGroup = openedFilePath ? groupFor(openedFilePath) : undefined
    const fileMarks = Object.fromEntries(projectFiles.filter(file => !file.isDirectory).flatMap(file => {
      const group = groupFor(file.path)
      return group ? [[managedFilePathKey(file.path), group.versions.at(-1)?.changeKind === 'created' ? '新建' : '已修改']] : []
    }))
    const searching = Boolean(fileQuery.trim())
    const emptyFiles = !fileLoading && !fileError && !(searching ? files.results.length : projectFiles.length)
    const fileActions = <>
      <button type="button" aria-label="刷新项目文件" onClick={() => setFileRefresh(value => value + 1)}><RotateCcw size={14} /></button>
      <button type="button" aria-label="打开项目文件夹" onClick={() => { void desktopClient.openProjectFolder(conversationId).catch(cause => toast.error(userFacingCause(cause))) }}><FolderOpen size={14} /></button>
    </>
    return (
      <div className={`fox-file-panel ${openedFilePath ? 'is-file-open' : 'is-tree-only'}`}>
        {openedFilePath
          ? selectedArtifact
            ? <ArtifactFilePreview managed={managed} group={activeGroup} detail={detail} artifactId={selectedArtifact.id} displayPath={openedFileDisplayPath} projectPath={openedFilePath} />
            : <ProjectFilePreviewPane managed={managed} group={activeGroup} conversationId={conversationId} path={openedFilePath} displayPath={openedFileDisplayPath} />
          : <div className="fox-file-tree-pane">
              <div className="fox-panel-search"><Search size={14} /><input value={fileQuery} onChange={event => setFileQuery(event.target.value)} placeholder="搜索文件" />{fileQuery && <button type="button" aria-label="清除搜索" onClick={() => setFileQuery('')}><X size={12} /></button>}</div>
              {emptyFiles ? <SideSection title="项目文件" action={fileActions} className="fox-file-tree-empty">
                  <RightPanelEmpty icon={searching ? Search : Folders}>{searching ? '没有匹配的文件。' : '这个项目还没有文件。'}</RightPanelEmpty>
                </SideSection>
                : <ScrollArea type="always" className="fox-file-tree">
                <SideSection title="项目文件" action={fileActions}><SideCard>
                  {fileLoading ? <div className="fox-panel-loading"><LoaderCircle className="animate-spin" />正在读取项目文件</div>
                    : fileError ? <p className="fox-file-tree-error" role="alert"><AlertTriangle size={14} />{fileError}<button type="button" onClick={() => setFileRefresh(value => value + 1)}>重试</button></p>
                    : searching ? <div className="fox-file-search-results">
                      {files.results.map(item => <button type="button" key={item.path} onClick={() => selectFile(item.path)}><FileText size={14} /><span><strong>{item.name}</strong><small>{item.parent || projectRoot}</small></span></button>)}
                      {files.truncated && <p>仅显示前 200 个匹配文件，请缩小搜索范围</p>}
                    </div>
                    : <ProjectFileTree key={conversationId} onDirectoriesChange={files.setDirectories} entries={projectFiles} workspaceName={projectRoot.split(/[\\/]/).at(-1) || projectRoot} selectedFile="" marks={fileMarks} onSelect={selectFile} />}
                </SideCard></SideSection>
              </ScrollArea>}
            </div>}
      </div>
    )
  }
  if (mode === 'knowledge') {
    const sources = runtimeProcess(detail?.runtimeEvents ?? []).sources
    const bindings = (detail?.knowledgeBindings ?? []).filter(binding => binding.enabled !== false)
    if (!bindings.length && !sources.length) return <RightPanelEmpty icon={BookOpen}>这段对话还没有引用资料。</RightPanelEmpty>
    return <div className="fox-side-body fox-source-list">
      {bindings.length > 0 && <SideSection title="已绑定" count={bindings.length}><SideCard>{bindings.map(binding => <Item className="fox-side-row" key={binding.knowledgeBaseId}><span className="fox-side-tile"><BookOpen size={15} /></span><ItemContent><ItemTitle>{binding.knowledgeBaseName || binding.knowledgeBaseId}</ItemTitle></ItemContent>{onUnbindKnowledge && <DropdownMenu><DropdownMenuTrigger asChild><button type="button" className="fox-panel-icon-button is-small" aria-label={`管理 ${binding.knowledgeBaseName || binding.knowledgeBaseId}`} disabled={knowledgeBusy}><Ellipsis /></button></DropdownMenuTrigger><DropdownMenuContent align="end"><DropdownMenuItem onSelect={() => { void onUnbindKnowledge(binding).then(saved => { if (!saved) toast.error('没有解绑资料库，请重试。') }) }}>解绑资料库</DropdownMenuItem></DropdownMenuContent></DropdownMenu>}</Item>)}</SideCard></SideSection>}
      {sources.length > 0 && <SideSection title="本次引用" count={sources.length}><SideCard>{sources.map(source => <Item className="fox-side-row" key={source.id}><span className="fox-side-tile"><FileText size={15} /></span><ItemContent><ItemTitle title={source.title}>{source.title}</ItemTitle><ItemDescription>{source.excerpt || '本次回答引用的知识来源'}</ItemDescription></ItemContent></Item>)}</SideCard></SideSection>}
    </div>
  }
  if (mode === 'changes') return <ChangesPanel key={conversationId} detail={detail} focusedEvidence={activity.focusedEvidence} onOpenEvidence={onOpenEvidence} />
  if (mode === 'evidence') return <Suspense fallback={<div className="fox-context-empty">正在加载相关依据…</div>}><ActivityDetailPanel detail={detail} {...activity} /></Suspense>
  if (mode === 'background') return <BackgroundTasksPanel key={conversationId} detail={detail} />
  if (mode === 'browser') {
    return <BrowserPanel detail={detail} />
  }
  if (mode === 'agents') {
    return <ChildAgentPanel detail={detail} onOpenChildAgent={onOpenChildAgent} />
  }
  return null
}

const baseRightItems: Array<[RightMode, string, ReactNode]> = [
  ['knowledge', '资料库', <BookOpen size={16} />],
  ['files', '文件', <Folders size={16} />],
  ['changes', '更改', <FileEdit size={16} />],
  ['terminal', '终端', <Terminal size={16} />],
  ['background', '后台任务', <ListTodo size={16} />],
  ['browser', '网页与网络', <Globe2 size={16} />],
  ['agents', '协作助手', <Bot size={16} />]
]

function SidebarHome({ detail, managed, onMode, onOpenFile }: { detail: ConversationDetail | null; managed: ManagedFilesController | null; onMode: (mode: RightMode) => void; onOpenFile: (path: string, artifactId: string | null) => void }) {
  const tools = detail?.toolCalls ?? []
  const changes = tools.filter(tool => isChangeOrCommandTool(tool.toolName))
  const failed = changes.filter(tool => tool.status === 'failed').length
  const agents = detail?.childRuns ?? []
  const pending = new Set((detail?.approvals ?? []).filter(item => item.status === 'pending').map(item => item.conversationId))
  const needsApproval = agents.some(agent => pending.has(agent.childConversationId))
  const activeAgents = agents.filter(childRunIsActive).length
  const project = detail?.conversation.projectRoot?.split(/[\\/]/).filter(Boolean).at(-1) || '未绑定项目'
  const groups = managed?.state.status === 'ready' ? managed.state.groups : []
  const recent = new Map((detail?.artifacts ?? []).map(file => [managedFilePathKey(file.storagePath), { path: file.storagePath, name: file.displayName, time: file.updatedAt, artifactId: file.id as string | null }]))
  for (const file of groups) {
    const key = managedFilePathKey(file.storagePath)
    if (!file.deleted && (!recent.has(key) || (file.versions[0]?.createdAt ?? 0) > recent.get(key)!.time)) recent.set(key, { path: file.storagePath, name: file.displayName, time: file.versions[0]?.createdAt ?? 0, artifactId: recent.get(key)?.artifactId ?? null })
  }
  const rows: Array<{ mode: RightMode; label: string; Icon: LucideIcon; description: string; status?: ReactNode; tone?: string }> = [
    { mode: 'files', label: '文件', Icon: Folders, description: `${project} · 改过 ${groups.length || (detail?.artifacts.length ?? 0)} 个文件` },
    { mode: 'knowledge', label: '资料库', Icon: BookOpen, description: `已绑定 ${(detail?.knowledgeBindings ?? []).filter(item => item.enabled !== false).length} 个 · 本次引用 ${runtimeProcess(detail?.runtimeEvents ?? []).sources.length} 条` },
    { mode: 'changes', label: '更改', Icon: FileEdit, description: `${changes.length} 项`, status: failed ? `${failed} 个失败` : undefined, tone: 'is-error' },
    { mode: 'terminal', label: '终端', Icon: Terminal, description: '在项目目录打开 PowerShell' },
    { mode: 'browser', label: '网页与网络', Icon: Globe2, description: `${tools.filter(isWebToolCall).length} 次网络调用` },
    { mode: 'background', label: '后台任务', Icon: ListTodo, description: '查看命令和输出' },
    { mode: 'agents', label: '协作助手', Icon: Bot, description: `${agents.length} 个 · ${activeAgents} 个进行中`, status: needsApproval ? '需要你批准' : undefined, tone: 'is-warning' },
  ]
  return <div className="fox-sidebar-home">
    <h2>页面</h2>
    <nav aria-label="侧栏页面">{rows.map(({ mode, label, Icon, description, status, tone }) => <button type="button" className="fox-sidebar-home-row" key={mode} onClick={() => onMode(mode)}><span className="fox-sidebar-home-icon"><Icon /></span><span className="fox-sidebar-home-copy"><strong>{label}</strong><small>{description}</small></span>{status ? <span className={`fox-sidebar-home-status ${tone || ''}`}>{status}</span> : <ChevronRight className="fox-sidebar-home-chevron" />}</button>)}</nav>
    <section className="fox-sidebar-recent"><h2>最近文件</h2>{[...recent.values()].sort((a, b) => b.time - a.time).slice(0, 6).map(file => {
      const Icon = /\.(xlsx?|csv)$/i.test(file.name) ? FileSpreadsheet : /\.(html?|tsx?|jsx?|json)$/i.test(file.name) ? FileCode : FileText
      const folder = file.path.replace(/\\/g, '/').split('/').slice(0, -1).at(-1) || project
      return <button type="button" className="fox-sidebar-home-row" key={file.path} title={file.path} onClick={() => onOpenFile(file.path, file.artifactId)}><Icon /><span className="fox-sidebar-home-copy"><strong>{file.name}</strong><small>{folder}</small></span><time>{new Date(file.time).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</time></button>
    })}{recent.size === 0 && <p>还没有最近文件。</p>}</section>
  </div>
}

/** The right-hand sidebar: opened pages (including one tab per open file), the
 *  `＋` page menu, and the page body. Exported so its DOM can be driven directly by
 *  the acceptance tests without mounting the whole Workbench. */
export function RightPanel({ onUnbindKnowledge, knowledgeBusy, mode, tabs, detail, activity, width, maximized = false, hidden = false, fileTabs, activeFileTabId, activeChildAgent, onMode, onCloseMode, onOpenFile, onActivateFile, onCloseFile, onOpenChildAgent, onBackChildAgent, onOpenEvidence, onToggleMaximized, onCollapse }: { onUnbindKnowledge?: (binding: ConversationDetail["knowledgeBindings"][number]) => Promise<boolean>; knowledgeBusy?: boolean; mode: RightMode | null; tabs: RightMode[]; detail: ConversationDetail | null; activity: Omit<ActivityDetailPanelProps, 'detail'>; width: number; maximized?: boolean; hidden?: boolean; fileTabs: OpenFileTab[]; activeFileTabId: string | null; activeChildAgent: ChildAgentSelection | null; onMode: (mode: RightMode) => void; onCloseMode: (mode: RightMode) => void; onOpenFile: (path: string, artifactId: string | null) => void; onActivateFile: (tabId: string) => void; onCloseFile: (tabId: string) => void; onOpenChildAgent: (run: ChildRunRecord, avatarIndex: number) => void; onBackChildAgent: () => void; onOpenEvidence?: (evidence: TaskEvidenceRecord) => void; onToggleMaximized: () => void; onCollapse: () => void }) {
  const activeChildAgentRun = activeChildAgent
    ? detail?.childRuns.find((run) => run.childRunId === activeChildAgent.run.childRunId) ?? activeChildAgent.run
    : null
  const rightItems = baseRightItems
  const conversationId = detail?.conversation.id ?? null
  const projectRoot = detail?.conversation.projectRoot
  const terminalContext = conversationId && projectRoot ? `${conversationId}:${projectRoot}` : null
  const terminalActive = mode === 'terminal' && !hidden
  const hasTerminalTab = tabs.includes('terminal')
  const openingTerminalFromMenu = useRef(false)
  const [terminalKey, setTerminalKey] = useState<string | null>(null)
  useEffect(() => {
    setTerminalKey(current => terminalActive ? terminalContext : hasTerminalTab && current === terminalContext ? current : null)
  }, [hasTerminalTab, terminalContext, terminalActive])

  const managed = useManagedFiles(hidden ? null : conversationId, managedFilesRefreshKey(detail))
  const showingFilePreview = detail?.conversation.projectRoot && fileTabs.some(tab => tab.id === activeFileTabId && tab.path)
  const [versionsUnseen] = useManagedFileUnseen({
    surface: 'files',
    conversationId: detail?.conversation.id ?? null,
    fingerprint: managedFilesFingerprintOf(managed),
    viewing: !hidden && mode === 'files' && !showingFilePreview,
  })
  const hasFilesTab = tabs.includes('files')
  return (
    <aside hidden={hidden} className={`fox-context-panel ${mode === null ? 'is-home' : mode === 'files' ? 'is-files' : ''} ${maximized ? 'is-maximized' : ''}`} style={maximized ? undefined : { width }}>
      {!hidden && (mode === null ? <SidebarHome detail={detail} managed={managed} onMode={onMode} onOpenFile={onOpenFile} />
        : <Tabs className="fox-context-tabbar" value={activeFileTabId && mode === 'files' ? `file:${activeFileTabId}` : mode} onValueChange={value => { if (value.startsWith('file:')) onActivateFile(value.slice(5)); else onMode(value as RightMode) }}>
        <TabsList className="fox-context-tabs" aria-label="已打开的侧边栏页面">
          {tabs.map((tabMode) => {
            const item = tabMode === 'evidence' ? ['evidence', '相关依据', <ShieldCheck size={16} />] as const : rightItems.find(([id]) => id === tabMode)
            if (!item) return null
            // A file tab *is* the files view. Keeping the mode tab beside it left a
            // second, default-looking tab for the same panel — the file tabs own it
            // while any are open, and the list stays reachable from the + menu.
            if (tabMode === 'files' && !hasFilesTab) return null
            const active = tabMode === mode && (tabMode !== 'files' || activeFileTabId === null)
            return <div key={tabMode} className={`fox-context-tab ${active ? 'is-active' : ''}`}>
              <Tooltip><TooltipTrigger asChild><TabsTrigger value={tabMode} aria-label={item[1]} title={item[1]}>{item[2]}{active && <span>{item[1]}</span>}</TabsTrigger></TooltipTrigger><TooltipContent>{item[1]}</TooltipContent></Tooltip>
              {active && <button type="button" className="fox-context-tab-close" aria-label={`关闭${item[1]}页面`} onClick={() => onCloseMode(tabMode)}><X size={12} /></button>}
              {tabMode === 'files' && versionsUnseen && <span className="fox-tab-dot" aria-hidden="true" />}
            </div>
          })}
          {fileTabs.map((tab) => {
            const active = mode === 'files' && activeFileTabId === tab.id
            return <div key={`file:${tab.id}`} className={`fox-context-tab is-file-tab ${active ? 'is-active' : ''}`}>
              <Tooltip><TooltipTrigger asChild><TabsTrigger value={`file:${tab.id}`} aria-label={tab.name} title={tab.path}><FileText size={14} />{active && <span>{tab.name}</span>}</TabsTrigger></TooltipTrigger><TooltipContent>{tab.path}</TooltipContent></Tooltip>
              {active && <button type="button" className="fox-context-tab-close" aria-label={`关闭 ${tab.name}`} onClick={() => onCloseFile(tab.id)}><X size={12} /></button>}
            </div>
          })}
        </TabsList>
        <div className="fox-context-tab-actions">
          <DropdownMenu>
            <DropdownMenuTrigger asChild><button type="button" aria-label="打开页面" title="打开页面"><Plus size={14} />{versionsUnseen && mode && !hasFilesTab && <span className="fox-tab-dot" aria-hidden="true" />}</button></DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="fox-context-add-menu" onCloseAutoFocus={event => {
              if (openingTerminalFromMenu.current) event.preventDefault()
              openingTerminalFromMenu.current = false
            }}>
              {rightItems.map(([id, label, icon]) => <DropdownMenuItem key={id} onSelect={() => {
                openingTerminalFromMenu.current = id === 'terminal' && !terminalActive && Boolean(terminalContext)
                onMode(id)
              }}>{icon}<span>{label}</span>{id === 'files' && versionsUnseen && <span className="fox-tab-dot is-inline" aria-hidden="true" />}</DropdownMenuItem>)}
            </DropdownMenuContent>
          </DropdownMenu>
          <button type="button" aria-label={maximized ? '还原右侧边栏' : '全屏显示右侧边栏'} title={maximized ? '还原' : '全屏'} onClick={onToggleMaximized}>{maximized ? <Minimize2 size={14} /> : <Maximize2 size={14} />}</button>
        </div>
      </Tabs>)}
      {!hidden && mode === 'agents' && activeChildAgent && activeChildAgentRun
        ? <ChildAgentConversationPanel key={activeChildAgentRun.childRunId} run={activeChildAgentRun} avatarIndex={activeChildAgent.avatarIndex} onBack={onBackChildAgent} />
        : !hidden && mode && mode !== 'terminal'
        ? <ContextContent onUnbindKnowledge={onUnbindKnowledge} knowledgeBusy={knowledgeBusy} mode={mode} detail={detail} activity={activity} fileTabs={fileTabs} activeFileTabId={activeFileTabId} managed={managed} onOpenFile={onOpenFile} onOpenEvidence={onOpenEvidence} onOpenChildAgent={onOpenChildAgent} />
        : null}
      {terminalActive && !terminalContext && <RightPanelEmpty icon={Terminal}>选择项目后即可打开 PowerShell。</RightPanelEmpty>}
      {conversationId && projectRoot && hasTerminalTab && (terminalActive || terminalKey === terminalContext) &&
        <div className="fox-terminal-page" hidden={!terminalActive}>
          <Suspense fallback={<div className="fox-context-empty" role="status">正在加载终端…</div>}>
            <TerminalPanel key={terminalContext} active={terminalActive} conversationId={conversationId} projectRoot={projectRoot} />
          </Suspense>
        </div>}
    </aside>
  )
}
