import { type ActivityDetailPanelProps } from '../components/ActivityDetailPanel'
import { FOX_ASSISTANT_AVATAR } from '../components/FoxAssistantAvatar'
import { safeDiagnosticText, toolFailurePresentation } from '../run-status-facts'
import { artifactSummaryLine, artifactVerificationLabel, formatFileSize } from '../runtime-artifacts'
import { childRunIsActive, childRunStatusLabel, formatDurationMs, isWebToolCall, normalizeBrowserUrl, webActivitySummary } from '../sidebar-model'
import { Agent as AIAgent, AgentContent as AIAgentContent } from '@/components/ai-elements/agent'
import { Files, SubFiles, FolderItem, FolderTrigger, FolderPanel, FileItem } from '@/components/animate-ui/components/base/files'
import { WebPreview, WebPreviewBody, WebPreviewNavigation, WebPreviewNavigationButton } from '@/components/ai-elements/web-preview'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { ScrollArea } from '@/components/ui/scroll-area'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { desktopClient, desktopErrorDetails } from '@/features/conversations/api/desktop-client'
import { type ApprovalRecord, type ArtifactActionResponse, type ArtifactApplication, type ArtifactInspectResponse, type ArtifactRecord, type ChildRunRecord, type ConversationDetail, type ConversationMessage, type DesktopErrorDetails, type ProjectFileActionResponse, type ProjectFileEntry, type ProjectFilePreview, type TaskEvidenceRecord, type ToolCallRecord } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { Activity, AlertTriangle, ArrowLeft, BookOpen, Bot, Check, ChevronDown, ChevronRight, ChevronUp, CircleStop, Copy, ExternalLink, FileEdit, FileText, FolderOpen, Folders, Globe2, LoaderCircle, Maximize2, Minimize2, PanelRight, Plus, RotateCcw, Search, ShieldCheck, Terminal, X } from 'lucide-react'
import { type ReactNode, Suspense, lazy, useEffect, useMemo, useState } from 'react'
import { commandOutput, isChangeOrCommandTool, isCommandTool, toolExecutionStatus, toolStatusLabel } from '../composer/fox-composer'
import { type ChildAgentSelection, MarkdownResponse, type OpenFileTab, type RightMode } from '../fox-shared'
import { compactValue, runtimeProcess } from '../timeline/fox-runtime-process-model'
import { Workbench } from '../workbench'

const ActivityDetailPanel = lazy(() => import('../components/ActivityDetailPanel').then((module) => ({ default: module.ActivityDetailPanel })))
const HtmlFilePreview = lazy(() => import('../html-file-preview').then((module) => ({ default: module.HtmlFilePreview })))

function toolTarget(tool: ToolCallRecord) {
  const input = tool.input && typeof tool.input === 'object' && !Array.isArray(tool.input)
    ? tool.input as Record<string, unknown>
    : {}
  return compactValue(input.path ?? input.filePath ?? input.target, tool.toolName)
}

function toolDiff(tool: ToolCallRecord, approvals: ApprovalRecord[]) {
  const approval = approvals.find((item) => item.toolCallId === tool.id)
  if (approval?.request.diff) return approval.request.diff
  const input = tool.input && typeof tool.input === 'object' && !Array.isArray(tool.input)
    ? tool.input as Record<string, unknown>
    : {}
  if (typeof input.content === 'string') return input.content.split('\n').map((line) => `+ ${line}`).join('\n')
  if (typeof input.oldText === 'string' || typeof input.newText === 'string') {
    return [
      ...String(input.oldText ?? '').split('\n').map((line) => `- ${line}`),
      ...String(input.newText ?? '').split('\n').map((line) => `+ ${line}`),
    ].join('\n')
  }
  return JSON.stringify(tool.input, null, 2)
}

export function ProjectFileTree({ entries, workspaceName, selectedFile, onSelect }: { entries: ProjectFileEntry[]; workspaceName: string; selectedFile: string; onSelect: (path: string) => void }) {
  const workspaceRootPath = '__fox_workspace_root__'
  const [openFolders, setOpenFolders] = useState<string[]>([workspaceRootPath])
  const byParent = new Map<string, ProjectFileEntry[]>()
  entries.forEach((entry) => byParent.set(entry.parent, [...(byParent.get(entry.parent) ?? []), entry]))
  const selectedKey = normalizedProjectPath(selectedFile).toLocaleLowerCase()
  // Share expansion state across official SubFiles roots. Opening a file from
  // another entry point reveals its ancestors without losing manual expansion.
  useEffect(() => {
    const ancestors = [workspaceRootPath]
    const entryByPath = new Map(entries.map(entry => [normalizedProjectPath(entry.path).toLocaleLowerCase(), entry]))
    let entry = entryByPath.get(selectedKey)
    const visited = new Set<string>()
    while (entry?.parent && !visited.has(entry.parent)) {
      visited.add(entry.parent)
      ancestors.push(entry.parent)
      entry = entryByPath.get(normalizedProjectPath(entry.parent).toLocaleLowerCase())
    }
    setOpenFolders(current => ancestors.every(path => current.includes(path)) ? current : [...new Set([...current, ...ancestors])])
  }, [entries, selectedKey])
  const render = (parent: string): ReactNode => (byParent.get(parent) ?? []).map((entry) => entry.isDirectory
    ? <FolderItem key={entry.path} value={entry.path}><FolderTrigger title={entry.path}>{entry.name}</FolderTrigger><FolderPanel><SubFiles open={openFolders} onOpenChange={setOpenFolders}>{render(entry.path)}</SubFiles></FolderPanel></FolderItem>
    : <FileItem key={entry.path} title={entry.path} aria-label={`打开 ${entry.name}`} aria-current={normalizedProjectPath(entry.path).toLocaleLowerCase() === selectedKey ? 'true' : undefined} onClick={() => onSelect(entry.path)}>{entry.name}</FileItem>)
  return <Files open={openFolders} onOpenChange={setOpenFolders} className="fox-animate-file-tree" aria-label="项目文件"><FolderItem value={workspaceRootPath}><FolderTrigger title={workspaceName}>{workspaceName}</FolderTrigger><FolderPanel><SubFiles open={openFolders} onOpenChange={setOpenFolders}>{render('')}</SubFiles></FolderPanel></FolderItem></Files>
}

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

function ProjectFilePreviewPane({ conversationId, path, displayPath }: { conversationId: string; path: string; displayPath: string }) {
  const [preview, setPreview] = useState<ProjectFilePreview | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [applications, setApplications] = useState<ArtifactApplication[]>([])
  const [busyAction, setBusyAction] = useState<ProjectFileActionResponse['action'] | 'project_folder' | null>(null)

  useEffect(() => {
    let cancelled = false
    setPreview(null)
    setLoading(true)
    setError(null)
    void desktopClient.readProjectFile(conversationId, path)
      .then((response) => { if (!cancelled) setPreview(response) })
      .catch((cause) => { if (!cancelled) setError(cause instanceof Error ? cause.message : String(cause)) })
      .finally(() => { if (!cancelled) setLoading(false) })
    void desktopClient.projectFileAction(conversationId, path, 'inspect')
      .then((response) => { if (!cancelled) setApplications(response.applications) })
      .catch(() => { if (!cancelled) setApplications([]) })
    return () => { cancelled = true }
  }, [conversationId, path])

  const runAction = async (action: ProjectFileActionResponse['action'], applicationId?: string) => {
    if (busyAction || action === 'inspect') return
    setBusyAction(action)
    try {
      const response = await desktopClient.projectFileAction(conversationId, path, action, applicationId)
      setApplications(response.applications)
      if (action === 'copy_path') toast.success('已复制受控路径')
    } catch (cause) {
      toast.error(desktopErrorDetails(cause).message)
    } finally {
      setBusyAction(null)
    }
  }

  const openProjectFolder = async () => {
    if (busyAction) return
    setBusyAction('project_folder')
    try {
      await desktopClient.openProjectFolder(conversationId)
    } catch (cause) {
      toast.error(desktopErrorDetails(cause).message)
    } finally {
      setBusyAction(null)
    }
  }

  return (
    <div className="fox-file-preview fox-project-file-preview">
      <div className="fox-project-file-toolbar">
        <div className="fox-file-location" title={displayPath}><span>{displayPath}</span></div>
        <div className="fox-project-file-actions">
          <Tooltip>
            <TooltipTrigger asChild><button type="button" aria-label="打开文件夹" disabled={Boolean(busyAction)} onClick={() => void openProjectFolder()}><FolderOpen size={15} /></button></TooltipTrigger>
            <TooltipContent>打开文件夹</TooltipContent>
          </Tooltip>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="sm" variant="outline" className="fox-file-open-trigger" aria-label="打开方式" disabled={Boolean(busyAction)}><ExternalLink className="fox-file-open-icon" /><span>打开</span><ChevronDown className="fox-file-open-chevron" /></Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="fox-result-open-menu">
              <DropdownMenuLabel>文件操作</DropdownMenuLabel>
              <DropdownMenuItem onSelect={() => void runAction('open')}><ExternalLink size={14} />系统打开</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void runAction('reveal')}><FolderOpen size={14} />打开文件夹</DropdownMenuItem>
              <DropdownMenuItem onSelect={() => void runAction('copy_path')}><Copy size={14} />复制路径</DropdownMenuItem>
              <DropdownMenuSeparator />
              {applications.length > 0
                ? applications.map((application) => <DropdownMenuItem key={application.id} onSelect={() => void runAction('open_with', application.id)}><Terminal size={14} />{application.label}</DropdownMenuItem>)
                : <DropdownMenuItem disabled>未检测到已支持的编辑器</DropdownMenuItem>}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      <div className="fox-project-file-content">
        {loading
          ? <div className="fox-panel-loading"><LoaderCircle className="animate-spin" />正在读取文件</div>
          : error
            ? <div className="fox-result-unavailable"><AlertTriangle size={21} /><strong>预览失败</strong><span>{error}</span></div>
            : /\.html?$/i.test(path) && preview?.content ? <Suspense fallback={<pre>正在加载预览…</pre>}><HtmlFilePreview key={path} name={displayPath} content={preview.content} /></Suspense> : <pre>{preview?.content ?? ''}</pre>}
        {preview?.truncated && <small className="fox-result-truncated">预览已截断，仅显示文件开头部分。</small>}
      </div>
    </div>
  )
}

function ArtifactFilePreview({ detail, artifactId, displayPath }: { detail: ConversationDetail | null; artifactId?: string | null; displayPath: string }) {
  const conversationId = detail?.conversation.id ?? null
  const artifact = detail?.artifacts.find((item) => item.id === artifactId) ?? null
  const [inspection, setInspection] = useState<ArtifactInspectResponse | null>(null)
  const [loading, setLoading] = useState(false)
  const [busyAction, setBusyAction] = useState<ArtifactActionResponse['action'] | 'project_folder' | null>(null)
  const [error, setError] = useState<DesktopErrorDetails | null>(null)
  const [feedback, setFeedback] = useState('')

  useEffect(() => {
    if (!conversationId || !artifact) {
      setInspection(null)
      setError(null)
      setFeedback('')
      return
    }
    let cancelled = false
    setInspection(null)
    setLoading(true)
    setError(null)
    setFeedback('')
    void desktopClient.inspectArtifact(conversationId, artifact.id)
      .then((response) => { if (!cancelled) setInspection(response) })
      .catch((cause) => { if (!cancelled) setError(desktopErrorDetails(cause)) })
      .finally(() => { if (!cancelled) setLoading(false) })
    return () => { cancelled = true }
  }, [artifact?.id, conversationId])

  const runAction = async (action: ArtifactActionResponse['action'], applicationId?: string) => {
    if (!conversationId || !artifact || busyAction) return
    setBusyAction(action)
    setError(null)
    setFeedback('')
    try {
      const response = await desktopClient.artifactAction(conversationId, artifact.id, action, applicationId)
      setInspection(response)
      const application = response.applications.find((item) => item.id === applicationId)
      setFeedback(
        action === 'preview'
          ? '已刷新文件预览'
          : action === 'open'
            ? '已使用系统默认方式打开'
            : action === 'open_with'
              ? application
                ? '已使用 ' + application.label + ' 打开'
                : '已交给编程软件打开'
              : action === 'reveal'
                ? '已在文件夹中定位'
                : '已复制受控路径'
      )
    } catch (cause) {
      setError(desktopErrorDetails(cause))
    } finally {
      setBusyAction(null)
    }
  }

  const openProjectFolder = async () => {
    if (!conversationId || busyAction) return
    setBusyAction('project_folder')
    setError(null)
    setFeedback('')
    try {
      await desktopClient.openProjectFolder(conversationId)
      setFeedback('已打开项目文件夹')
    } catch (cause) {
      setError(desktopErrorDetails(cause))
    } finally {
      setBusyAction(null)
    }
  }

  if (!artifact) {
    return <div className="fox-empty-panel"><FileText size={29} /><strong>对话文件</strong><span>每次回答产生的新建或修改文件会显示在回答底部，点击后在这里打开。</span></div>
  }

  const applications = inspection?.applications ?? []
  return (
    <div className="fox-result-panel is-single-file">
      <div className="fox-result-file-toolbar">
        <div className="fox-file-location" title={displayPath}><span>{displayPath}</span></div>
        <div className="fox-result-file-actions">
          <Tooltip>
            <TooltipTrigger asChild>
              <button type="button" aria-label="打开文件夹" disabled={loading || Boolean(busyAction) || !detail?.conversation.projectRoot} onClick={() => void openProjectFolder()}><FolderOpen size={15} /></button>
            </TooltipTrigger>
            <TooltipContent>打开文件夹</TooltipContent>
          </Tooltip>
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="sm" variant="outline" className="fox-file-open-trigger" aria-label="打开方式" disabled={loading || Boolean(busyAction)}><ExternalLink className="fox-file-open-icon" /><span>打开</span><ChevronDown className="fox-file-open-chevron" /></Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="fox-result-open-menu">
              <DropdownMenuLabel>文件操作</DropdownMenuLabel>
              <DropdownMenuItem disabled={!inspection?.capabilities.open} onSelect={() => void runAction('open')}><ExternalLink size={14} />系统打开</DropdownMenuItem>
              <DropdownMenuItem disabled={!inspection?.capabilities.reveal} onSelect={() => void runAction('reveal')}><FolderOpen size={14} />打开文件夹</DropdownMenuItem>
              <DropdownMenuItem disabled={!inspection?.capabilities.copyPath} onSelect={() => void runAction('copy_path')}><Copy size={14} />复制路径</DropdownMenuItem>
              <DropdownMenuSeparator />
              {applications.length > 0
                ? applications.map((application) => <DropdownMenuItem key={application.id} onSelect={() => void runAction('open_with', application.id)}><Terminal size={14} />{application.label}</DropdownMenuItem>)
                : <DropdownMenuItem disabled>未检测到已支持的编辑器</DropdownMenuItem>}
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      <div className="fox-result-detail">
        {artifact && <div className="fox-result-file-meta">
          <span>{artifact.artifactType === 'created_file' ? '新建' : artifact.artifactType === 'modified_file' ? '已修改' : '文件结果'}</span>
          <span>{artifactSummaryLine(artifact)}</span>
          <span>{formatFileSize(artifact.byteSize)}</span>
          {artifact.delivery?.versionNo ? <span>v{artifact.delivery.versionNo}</span> : null}
          {artifact.delivery ? <span>{artifactVerificationLabel(artifact.delivery.verificationStatus)}</span> : null}
        </div>}
        {artifact?.delivery?.summary ? <p className="fox-result-delivery-note">{safeDiagnosticText(artifact.delivery.summary)}</p> : null}
        {error && <div className="fox-result-error" role="alert"><AlertTriangle size={15} /><div><strong>操作未能完成</strong><span>{error.message}</span>{error.retryable && <small>可以稍后重试。</small>}</div></div>}
        {feedback && <p className="fox-result-feedback" role="status">{feedback}</p>}
        <div className="fox-result-raw">
          {inspection?.preview.available && inspection.preview.content !== null
            ? inspection.preview.kind === 'html' ? <Suspense fallback={<pre>正在加载预览…</pre>}><HtmlFilePreview key={artifact.id} name={artifact.displayName} content={inspection.preview.content} /></Suspense> : <pre>{inspection.preview.content}</pre>
            : <div className="fox-result-unavailable"><FileText size={21} /><strong>{inspection?.preview.reason ?? (loading ? '正在请求 Host 预览…' : '暂时没有可用预览')}</strong><span>{inspection?.preview.kind === 'html' ? 'HTML 预览暂不可用，可使用顶部“打开方式”查看原文件。' : '可使用顶部“打开方式”查看原文件。'}</span></div>}
          {inspection?.preview.truncated && <small className="fox-result-truncated">预览已截断，仅显示前 512 KB。</small>}
        </div>
      </div>
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
      <div className="fox-browser-tabs" role="tablist" aria-label="网页记录">
        <button type="button" role="tab" aria-selected={activeTab === 'page'} className={activeTab === 'page' ? 'is-active' : ''} onClick={() => setActiveTab('page')}><Globe2 size={13} />网页预览</button>
        <button type="button" role="tab" aria-selected={activeTab === 'activity'} className={activeTab === 'activity' ? 'is-active' : ''} onClick={() => setActiveTab('activity')}><Activity size={13} />网络记录{webCalls.length > 0 && <small>{webCalls.length}</small>}</button>
      </div>
      <WebPreviewNavigation className="fox-browser-address">
        <WebPreviewNavigationButton disabled={!browserUrl} tooltip="刷新" onClick={() => setFrameVersion((version) => version + 1)}><RotateCcw size={14} /></WebPreviewNavigationButton>
        <form onSubmit={(event) => { event.preventDefault(); navigateTo(address) }}>
          <input aria-label="网页地址" value={address} onChange={(event) => setAddress(event.target.value)} placeholder="输入网址，例如 example.com" spellCheck={false} />
        </form>
        <WebPreviewNavigationButton disabled={!browserUrl} tooltip="在系统浏览器中打开" onClick={() => void openExternal()}><ExternalLink size={14} /></WebPreviewNavigationButton>
      </WebPreviewNavigation>
      {activeTab === 'page'
        ? browserUrl
          ? <div className="fox-browser-frame-wrap"><WebPreviewBody key={`${browserUrl}:${frameVersion}`} className="fox-browser-frame" src={browserUrl} /><p><ShieldCheck size={12} />网页运行在隔离预览中；若网站拒绝嵌入，请点击右上角外部打开。</p></div>
          : <div className="fox-browser-empty"><img className="fox-panel-mascot" src={FOX_ASSISTANT_AVATAR} alt="" /><strong>输入网址即可手动预览</strong><span>这里不能代替 Agent 操作网页；真实联网调用与结果保留在“网络记录”中。</span></div>
        : webCalls.length > 0
          ? <ScrollArea className="fox-browser-activity"><div>{webCalls.map((tool) => {
              const summary = webActivitySummary(tool)
              const status = tool.status === 'completed' ? '已完成' : tool.status === 'failed' ? '失败' : '执行中'
              const content = <><span className={`fox-browser-activity-icon is-${tool.status}`}><Globe2 size={14} /></span><div><strong>{summary.label}</strong><small>{summary.target}</small><pre>{toolResultPreview(tool)}</pre></div><Badge variant={tool.status === 'failed' ? 'destructive' : tool.status === 'completed' ? 'secondary' : 'outline'}>{status}</Badge>{summary.navigableUrl && <ChevronRight size={14} />}</>
              return summary.navigableUrl
                ? <button type="button" key={tool.id} onClick={() => navigateTo(summary.navigableUrl!)}>{content}</button>
                : <div key={tool.id} className="fox-browser-activity-row">{content}</div>
            })}</div></ScrollArea>
          : <div className="fox-browser-empty"><Globe2 size={30} /><strong>这段对话还没有联网记录</strong><span>Agent 调用 web_search、web_read 或 http_request 后，目标、状态和结果会显示在这里。</span></div>}
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
  | { kind: 'tool'; id: string; createdAt: number; toolName: string; status: string }

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

function childRunFailureMessage(run: ChildRunRecord) {
  // The concrete execution cap is internal accounting: the user reads that the
  // sub-task did not finish in time and that its result was not used.
  if (childRunFailureTitle(run) === '运行超时') return '子任务未在限定时间内完成，结果未采用'
  return run.errorMessage ?? (run.status === 'interrupted' ? '运行意外中断，本次结果未采用' : '协作助手未返回可采用结果')
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
const collapsedChildAgentLimit = 10

function ChildAgentPanel({ detail, onOpenChildAgent }: { detail: ConversationDetail | null; onOpenChildAgent: (run: ChildRunRecord, avatarIndex: number) => void }) {
  const previewRuns = useMemo(childAgentVisualQaRuns, [])
  const runs = useMemo(
    () => [...(detail?.childRuns?.length ? detail.childRuns : previewRuns)].sort((left, right) => right.createdAt - left.createdAt),
    [detail?.childRuns, previewRuns],
  )
  const [showAllAgents, setShowAllAgents] = useState(false)
  const visibleRuns = showAllAgents ? runs : runs.slice(0, collapsedChildAgentLimit)

  return (
    <AIAgent className="fox-agent-panel">
      <AIAgentContent className="fox-agent-content">
        {runs.length === 0
          ? <div className="fox-agent-empty"><Bot size={27} /><strong>这段对话还没有协作助手</strong><span>协作助手开始运行后，会显示在这个列表中。</span></div>
          : <nav className="fox-agent-list" aria-label="协作助手列表">
              {visibleRuns.map((run, index) => (
                <button type="button" key={run.childRunId} className="fox-agent-list-item" aria-label={`打开 ${run.workerAgentName} 消息`} onClick={() => onOpenChildAgent(run, index)}>
                  <img className="fox-agent-list-avatar" src={childAgentAvatars[index % childAgentAvatars.length]} alt="" />
                  <span className="fox-agent-list-copy"><strong>{run.workerAgentName}</strong><small title={run.objective}>{run.objective}</small></span>
                  <em className={`is-${run.status}`}>{childRunStatusLabel(run.status)}</em>
                  <ChevronRight className="fox-agent-list-open" aria-hidden="true" />
                </button>
              ))}
              {runs.length > collapsedChildAgentLimit && <button type="button" className="fox-agent-list-more" aria-expanded={showAllAgents} onClick={() => setShowAllAgents((value) => !value)}>
                <span>{showAllAgents ? '收起' : `显示更多（${runs.length - collapsedChildAgentLimit}）`}</span>
                {showAllAgents ? <ChevronUp /> : <ChevronDown />}
              </button>}
            </nav>}
      </AIAgentContent>
    </AIAgent>
  )
}

function childUserMessageNeedsCollapse(content: string) {
  return content.length > 84 || content.split(/\r?\n/).length > 3
}

function ChildAgentConversationPanel({ run, avatarIndex, onBack }: { run: ChildRunRecord; avatarIndex: number; onBack: () => void }) {
  const [childDetail, setChildDetail] = useState<ConversationDetail | null>(null)
  const [childDetailLoading, setChildDetailLoading] = useState(false)
  const [childDetailError, setChildDetailError] = useState<string | null>(null)
  const [expandedUserMessages, setExpandedUserMessages] = useState<Set<string>>(() => new Set())
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
        <div><strong title={run.objective}>{run.workerAgentName}</strong></div>
        <span className={`is-${run.status}`}><ChildRunStatusIcon run={run} />{childRunStatusLabel(run.status)}</span>
      </header>
      <div className="fox-agent-conversation-scroll">
        {childDetailLoading && !childDetail
          ? <div className="fox-agent-inline-state"><LoaderCircle className="animate-spin" />正在读取协作助手消息</div>
          : childDetailError && !childDetail
            ? <div className="fox-agent-inline-state is-error"><AlertTriangle />{childDetailError}</div>
            : transcript.map((item) => item.kind === 'tool'
              ? <div className={`fox-agent-tool-event is-${item.status}`} key={`tool:${item.id}`}>
                  {item.status === 'completed' ? <Check /> : item.status === 'failed' ? <AlertTriangle /> : <LoaderCircle className="animate-spin" />}
                  <span>{childToolStatusLabel(item.status)} · <code>{item.toolName}</code></span>
                </div>
              : item.role === 'user'
                ? <article className="fox-agent-message is-user" key={`message:${item.id}`}>
                    <div className="fox-agent-user-message">
                      <div className={`fox-agent-user-bubble ${childUserMessageNeedsCollapse(item.content) ? 'is-collapsible' : ''} ${expandedUserMessages.has(item.id) ? 'is-expanded' : ''}`}>
                        <p>{item.content}</p>
                        {childUserMessageNeedsCollapse(item.content) && <button type="button" className="fox-agent-message-toggle" aria-label={expandedUserMessages.has(item.id) ? '收起用户消息' : '展开用户消息'} aria-expanded={expandedUserMessages.has(item.id)} onClick={() => setExpandedUserMessages((current) => { const next = new Set(current); if (next.has(item.id)) next.delete(item.id); else next.add(item.id); return next })}>{expandedUserMessages.has(item.id) ? <ChevronUp /> : <ChevronDown />}</button>}
                      </div>
                      <time>{new Date(item.createdAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</time>
                    </div>
                  </article>
                : <article className="fox-agent-message is-assistant" key={`message:${item.id}`}>
                    <img src={childAgentAvatars[avatarIndex % childAgentAvatars.length]} alt="" />
                    <div><small>{run.workerAgentName} · {new Date(item.createdAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</small><MarkdownResponse>{item.content}</MarkdownResponse></div>
                  </article>)}
        {!childDetailLoading && !childDetailError && transcript.length === 0 && <p className="fox-agent-transcript-empty">这个协作助手还没有可见消息。</p>}
        {childRunIsActive(run) && <div className="fox-agent-live-line"><LoaderCircle className="animate-spin" />协作助手正在运行，消息会自动更新</div>}
        {(run.status === 'failed' || run.status === 'interrupted') && <div className="fox-agent-run-error" role="alert">
          <AlertTriangle />
          <div><strong>{childRunFailureTitle(run)}</strong><p>{childRunFailureMessage(run)}</p></div>
        </div>}
      </div>
    </section>
  )
}

function ContextContent({ mode, detail, fileTabs, activeFileTabId, activity, onOpenFile, onOpenEvidence, onOpenChildAgent }: { mode: RightMode; detail: ConversationDetail | null; fileTabs: OpenFileTab[]; activeFileTabId: string | null; activity: Omit<ActivityDetailPanelProps, "detail">; onOpenFile: (path: string, artifactId: string | null) => void; onOpenEvidence?: (evidence: TaskEvidenceRecord) => void; onOpenChildAgent: (run: ChildRunRecord, avatarIndex: number) => void }) {
  const conversationId = detail?.conversation.id ?? null
  const [selectedChange, setSelectedChange] = useState('')
  const [fileSearch, setFileSearch] = useState('')
  const [projectFiles, setProjectFiles] = useState<ProjectFileEntry[]>([])
  const [fileLoading, setFileLoading] = useState(false)
  const [fileError, setFileError] = useState<string | null>(null)

  useEffect(() => {
    if (mode !== 'files' || !conversationId) return
    let cancelled = false
    setFileLoading(true)
    setFileError(null)
    void desktopClient.listProjectFiles(conversationId).then((items) => {
      if (cancelled) return
      setProjectFiles(items)
    }).catch((cause) => {
      if (!cancelled) setFileError(cause instanceof Error ? cause.message : String(cause))
    }).finally(() => { if (!cancelled) setFileLoading(false) })
    return () => { cancelled = true }
  }, [conversationId, mode])

  const activeFileTab = fileTabs.find((tab) => tab.id === activeFileTabId) ?? null
  const selectedArtifact = detail?.artifacts.find((item) => item.id === activeFileTab?.artifactId) ?? null

  if (mode === 'files') {
    const normalizedSearch = fileSearch.trim().toLowerCase()
    const visibleFiles = projectFiles.filter((item) => !item.isDirectory && (!normalizedSearch || item.path.toLowerCase().includes(normalizedSearch)))
    if (!conversationId || !detail?.conversation.projectRoot) return <div className="fox-empty-panel"><Folders size={29} /><strong>尚未选择项目</strong><span>为当前对话授权一个项目文件夹后即可浏览。</span></div>
    const selectFile = (path: string) => {
      const matchingArtifact = detail.artifacts.find((artifact) => artifactProjectPath(artifact, detail.conversation.projectRoot!, projectFiles) === path)
      onOpenFile(path, matchingArtifact?.id ?? null)
    }
    const openedFilePath = activeFileTab?.path ?? ''
    const openedFileDisplayPath = openedFilePath ? projectDisplayPath(openedFilePath, detail.conversation.projectRoot) : ''
    return (
      <div className={`fox-file-panel ${openedFilePath ? 'is-file-open' : 'is-tree-only'}`}>
        {openedFilePath
          ? selectedArtifact
            ? <ArtifactFilePreview detail={detail} artifactId={selectedArtifact.id} displayPath={openedFileDisplayPath} />
            : <ProjectFilePreviewPane conversationId={conversationId} path={openedFilePath} displayPath={openedFileDisplayPath} />
          : <div className="fox-file-tree-pane">
              <div className="fox-panel-search"><Search size={14} /><input value={fileSearch} onChange={(event) => setFileSearch(event.target.value)} placeholder="搜索文件" />{fileSearch && <button type="button" aria-label="清除搜索" onClick={() => setFileSearch('')}><X size={12} /></button>}</div>
              <ScrollArea className="fox-file-tree">
                {fileLoading ? <div className="fox-panel-loading"><LoaderCircle className="animate-spin" />正在读取项目文件</div> : normalizedSearch ? <div className="fox-file-search-results">{visibleFiles.map((item) => <button type="button" key={item.path} onClick={() => selectFile(item.path)}><FileText size={14} /><span><strong>{item.name}</strong><small>{item.parent || detail.conversation.projectRoot}</small></span></button>)}{visibleFiles.length === 0 && <p>没有匹配的文件</p>}</div> : <ProjectFileTree entries={projectFiles} workspaceName={detail.conversation.projectRoot.split(/[\\/]/).at(-1) || detail.conversation.projectRoot} selectedFile="" onSelect={selectFile} />}
              </ScrollArea>
              {fileError && <p className="fox-file-tree-error"><AlertTriangle size={14} />{fileError}</p>}
            </div>}
      </div>
    )
  }
  if (mode === 'knowledge') {
    const sources = runtimeProcess(detail?.runtimeEvents ?? []).sources
    const items = [
      ...(detail?.knowledgeBindings ?? []).map((item) => ({ id: `binding:${item.knowledgeBaseId}`, title: item.knowledgeBaseName || item.knowledgeBaseId, detail: '已绑定到当前对话', kind: '资料库' })),
      ...sources.map((item) => ({ id: `source:${item.id}`, title: item.title, detail: item.excerpt || '本次回答引用的知识来源', kind: '引用' })),
    ].filter((item, index, all) => all.findIndex((candidate) => candidate.id === item.id) === index)
    if (!items.length) return <div className="fox-empty-panel"><BookOpen size={29} /><strong>这段对话还没有引用资料</strong><span>从输入框的“+”绑定资料库，检索结果也会保留在这里。</span></div>
    return <ScrollArea className="fox-source-list">{items.map((item, index) => <button type="button" key={item.id}><span>{index + 1}</span><div><strong>{item.title}</strong><small>{item.kind} · {item.detail}</small></div></button>)}</ScrollArea>
  }
  if (mode === 'changes') {
    const changeFiles = (detail?.toolCalls ?? []).filter((item) => isChangeOrCommandTool(item.toolName))
    const activeChange = changeFiles.find((item) => item.id === selectedChange) ?? changeFiles.at(-1)
    const isCommand = Boolean(activeChange && isCommandTool(activeChange.toolName))
    const diffLines = activeChange && !isCommand ? toolDiff(activeChange, detail?.approvals ?? []).split('\n').map((line) => [line.startsWith('+') ? 'added' : line.startsWith('-') ? 'removed' : line.startsWith('@@') ? 'meta' : 'context', line]) : []
    const command = activeChange && isCommand ? commandOutput(activeChange) : null
    const activeExecution = activeChange ? toolExecutionStatus(activeChange) : null
    const linkedEvidence = activeChange ? (detail?.evidence ?? []).filter((evidence) => evidence.refKind === 'tool_call' && (evidence.refId === activeChange.id || evidence.refId === activeChange.runtimeToolCallId)) : []
    if (!changeFiles.length) return <div className="fox-empty-panel"><FileEdit size={29} /><strong>这段对话还没有修改或命令记录</strong><span>Fox 的写入、编辑和命令记录会显示在这里。</span></div>
    return (
      <div className="fox-change-panel">
        <div className="fox-change-list">
          {changeFiles.map((item) => <button type="button" key={item.id} className={activeChange?.id === item.id ? 'is-active' : ''} onClick={() => setSelectedChange(item.id)}>{isCommandTool(item.toolName) ? <Terminal size={14} /> : <FileEdit size={14} />}<span><strong>{isCommandTool(item.toolName) ? item.toolName : toolTarget(item)}</strong><small>{item.toolName} · {toolStatusLabel(item)}</small></span></button>)}
        </div>
        <div className="fox-diff-view">
          <div className="fox-diff-head">{isCommand ? <Terminal size={14} /> : <FileEdit size={14} />}<strong>{activeChange ? (isCommand ? activeChange.toolName : toolTarget(activeChange)) : '更改详情'}</strong><span><Badge variant={activeChange?.status === 'failed' || activeExecution?.requiresAttention ? 'destructive' : 'outline'}>{activeChange ? toolStatusLabel(activeChange, activeExecution) : '未知'}</Badge>{linkedEvidence.length > 0 && <Badge variant="secondary">{linkedEvidence.length} 条证据</Badge>}</span></div>
          {linkedEvidence.length > 0 && <div className="fox-linked-evidence">{linkedEvidence.map((evidence) => <button type="button" key={evidence.id} disabled={!onOpenEvidence} onClick={() => onOpenEvidence?.(evidence)}><ShieldCheck size={12} /><span>{evidence.summary}</span><small>{evidence.validityStatus}</small></button>)}</div>}
          {isCommand && activeChange && command ? <div className="fox-command-detail">
            <div className="fox-command-facts">{command.execution && <span>执行 <b>{command.execution.statusLabel}</b></span>}{command.execution?.requiresAttention && <span>原因 <b>{command.execution.uncertain ? '结果未知，未自动重试' : '已拒绝'}</b></span>}<span>退出码 <b>{command.exitCode === null ? '—' : command.exitCode}</b></span><span>耗时 <b>{command.duration === null ? '进行中' : formatDurationMs(command.duration)}</b></span>{command.cwd && <span title={command.cwd}>目录 <b>{command.cwd}</b></span>}</div>
            <ScrollArea className="fox-command-output"><pre>{command.output || activeChange.errorMessage || '命令没有返回输出。'}</pre></ScrollArea>
          </div> : <ScrollArea className="fox-diff-scroll"><code>{diffLines.map(([tone, line], index) => <span key={index} className={`is-${tone}`}><i>{index + 1}</i><b>{line || ' '}</b></span>)}</code></ScrollArea>}
        </div>
      </div>
    )
  }
  if (mode === 'activity') return <Suspense fallback={<div className="fox-context-empty">正在加载活动详情…</div>}><ActivityDetailPanel detail={detail} {...activity} /></Suspense>
  if (mode === 'browser') {
    return <BrowserPanel detail={detail} />
  }
  if (mode === 'agents') {
    return <ChildAgentPanel detail={detail} onOpenChildAgent={onOpenChildAgent} />
  }
  return null
}

const rightItems: Array<[RightMode, string, ReactNode]> = [
  ['knowledge', '资料库', <BookOpen size={16} />],
  ['files', '文件', <Folders size={16} />],
  ['changes', '更改', <FileEdit size={16} />],
  ['activity', '运行详情', <Activity size={16} />],
  ['browser', '网页与网络', <Globe2 size={16} />],
  ['agents', '协作助手', <Bot size={16} />]
]

/** The right-hand sidebar: opened pages (including one tab per open file), the
 *  `＋` page menu, and the page body. Exported so its DOM can be driven directly by
 *  the acceptance tests without mounting the whole Workbench. */
export function RightPanel({ mode, tabs, detail, activity, width, compact = false, maximized = false, fileTabs, activeFileTabId, activeChildAgent, onMode, onCloseMode, onOpenFile, onActivateFile, onCloseFile, onOpenChildAgent, onBackChildAgent, onOpenEvidence, onToggleMaximized, onCollapse }: { mode: RightMode | null; tabs: RightMode[]; detail: ConversationDetail | null; activity: Omit<ActivityDetailPanelProps, 'detail'>; width: number; compact?: boolean; maximized?: boolean; fileTabs: OpenFileTab[]; activeFileTabId: string | null; activeChildAgent: ChildAgentSelection | null; onMode: (mode: RightMode) => void; onCloseMode: (mode: RightMode) => void; onOpenFile: (path: string, artifactId: string | null) => void; onActivateFile: (tabId: string) => void; onCloseFile: (tabId: string) => void; onOpenChildAgent: (run: ChildRunRecord, avatarIndex: number) => void; onBackChildAgent: () => void; onOpenEvidence?: (evidence: TaskEvidenceRecord) => void; onToggleMaximized: () => void; onCollapse: () => void }) {
  const activeChildAgentRun = activeChildAgent
    ? detail?.childRuns.find((run) => run.childRunId === activeChildAgent.run.childRunId) ?? activeChildAgent.run
    : null
  const homeItemCounts: Record<RightMode, number> = {
    knowledge: detail?.knowledgeBindings.filter((binding) => binding.enabled !== false).length ?? 0,
    files: detail?.artifacts.length ?? 0,
    changes: detail?.toolCalls.filter((item) => isChangeOrCommandTool(item.toolName)).length ?? 0,
    activity: detail?.toolCalls.length ?? 0,
    browser: detail?.toolCalls.filter(isWebToolCall).length ?? 0,
    agents: detail?.childRuns.length ?? 0,
  }
  return (
    <aside className={`fox-context-panel ${mode === 'files' ? 'is-files' : ''} ${compact ? 'is-compact' : ''} ${maximized ? 'is-maximized' : ''}`} style={compact || maximized ? undefined : { width }}>
      <div className="fox-context-tabbar">
        <div className="fox-context-tabs" role="tablist" aria-label="已打开的侧边栏页面">
          {tabs.map((tabMode) => {
            const item = rightItems.find(([id]) => id === tabMode)
            if (!item) return null
            // A file tab *is* the files view. Keeping the mode tab beside it left a
            // second, default-looking tab for the same panel — the file tabs own it
            // while any are open, and the list stays reachable from the + menu.
            if (tabMode === 'files' && fileTabs.length > 0) return null
            const active = tabMode === mode && (tabMode !== 'files' || activeFileTabId === null)
            return <div key={tabMode} className={`fox-context-tab ${active ? 'is-active' : ''}`}>
              <button type="button" role="tab" aria-selected={active} title={item[1]} onClick={() => onMode(tabMode)}>{item[2]}<span>{item[1]}</span></button>
              <button type="button" className="fox-context-tab-close" aria-label={`关闭${item[1]}页面`} onClick={() => onCloseMode(tabMode)}><X size={12} /></button>
            </div>
          })}
          {fileTabs.map((tab) => {
            const active = mode === 'files' && activeFileTabId === tab.id
            return <div key={`file:${tab.id}`} className={`fox-context-tab is-file-tab ${active ? 'is-active' : ''}`}>
              <button type="button" role="tab" aria-selected={active} title={tab.path} onClick={() => onActivateFile(tab.id)}><FileText size={14} /><span>{tab.name}</span></button>
              <button type="button" className="fox-context-tab-close" aria-label={`关闭 ${tab.name}`} onClick={() => onCloseFile(tab.id)}><X size={12} /></button>
            </div>
          })}
        </div>
        <div className="fox-context-tab-actions">
          <DropdownMenu>
            <DropdownMenuTrigger asChild><button type="button" aria-label="打开页面" title="打开页面"><Plus size={14} /></button></DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="fox-context-add-menu">
              {rightItems.map(([id, label, icon]) => <DropdownMenuItem key={id} onSelect={() => onMode(id)}>{icon}<span>{label}</span></DropdownMenuItem>)}
            </DropdownMenuContent>
          </DropdownMenu>
          <button type="button" aria-label={maximized ? '还原右侧边栏' : '全屏显示右侧边栏'} title={maximized ? '还原' : '全屏'} onClick={onToggleMaximized}>{maximized ? <Minimize2 size={14} /> : <Maximize2 size={14} />}</button>
          <button type="button" className="is-sidebar-open" aria-label="关闭面板" title="关闭面板" onClick={onCollapse}><PanelRight size={14} /></button>
        </div>
      </div>
      {mode === 'agents' && activeChildAgent && activeChildAgentRun
        ? <ChildAgentConversationPanel key={activeChildAgentRun.childRunId} run={activeChildAgentRun} avatarIndex={activeChildAgent.avatarIndex} onBack={onBackChildAgent} />
        : mode
        ? <ContextContent mode={mode} detail={detail} activity={activity} fileTabs={fileTabs} activeFileTabId={activeFileTabId} onOpenFile={onOpenFile} onOpenEvidence={onOpenEvidence} onOpenChildAgent={onOpenChildAgent} />
        : <div className="fox-context-home"><nav aria-label="侧边栏功能">{rightItems.map(([id, label, icon]) => {
            const count = homeItemCounts[id]
            return <button type="button" key={id} onClick={() => onMode(id)}>{icon}<span>{label}</span>{count > 0 && <small>{count}</small>}</button>
          })}</nav></div>}
    </aside>
  )
}
