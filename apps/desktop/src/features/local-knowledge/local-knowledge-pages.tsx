import { Fragment, lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ChangeEvent, type FormEvent, type ReactNode } from 'react'
import { AlertCircle, ArrowLeft, ArrowRight, ArrowUpDown, BookOpen, Brain, CheckCircle2, ChevronDown, ChevronRight, ChevronsUpDown, Clock3, Copy, Cpu, CornerDownLeft, Database, Download, ExternalLink, Eye, FileQuestion, FileStack, FileText, FlaskConical, Folder, FolderInput, FolderOpen, FolderPlus, Gauge, HardDrive, History, Info, Layers3, ListFilter, ListTodo, LoaderCircle, MessageCircle, MoreHorizontal, PackageOpen, Pause, Pencil, Plus, Quote, RefreshCw, RotateCcw, Search, Settings2, Sparkles, Star, Trash2, Upload, WandSparkles, X, XCircle } from 'lucide-react'
import { createPortal } from 'react-dom'
import { ExpandingSearch } from '@/components/arc/expanding-search/expanding-search'
import { FileDropzone, type FileDropzoneItem } from '@/components/arc/file-dropzone/file-dropzone'
import { FoxEmptyState } from '@/components/fox-ui/empty-state'
import { desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import { MessageResponse } from '@/components/ai-elements/message-response'
import { Badge } from '@/components/ui/badge'
import { Breadcrumb, BreadcrumbItem, BreadcrumbLink, BreadcrumbList, BreadcrumbPage, BreadcrumbSeparator } from '@/components/ui/breadcrumb'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader } from '@/components/ui/card'
import { Dialog, DialogClose, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator, DropdownMenuShortcut, DropdownMenuSub, DropdownMenuSubContent, DropdownMenuSubTrigger, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Progress } from '@/components/ui/progress'
import { SkeletonContent } from '@/components/ui/skeleton'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import { cn } from '@/lib/utils'
import type { KnowledgeDocumentSourceMetadata } from '@/features/conversations/model/types'
import type { LocalKnowledgeDocumentAsk } from '@/features/workspace/types'
import { notify as toast } from '@/features/notifications'
import { detectShortcutPlatform, formatShortcut, matchesShortcutAction } from '@/features/settings/shortcut-model'
import { useShortcutPreferences } from '@/features/settings/use-shortcuts'
import { previewKind } from '@/features/knowledge/knowledge-preview-model'
import { binaryValue, createKnowledgeCachedPreviewSource, type KnowledgeCachedPreviewOpener } from '@/features/knowledge/knowledge-preview-source'
import { createOperationState, reduceOperationEvent, type OperationState } from './operation-reducer'
import { defaultLocalKnowledgeGateway } from './tauri-gateway'
import { LocalFileTypeIcon } from './local-file-type-icon'
import type { KnowledgeChunkPreview, KnowledgeJob, KnowledgeJobStatus, KnowledgeOperationStage, KnowledgeRetrievalCase, KnowledgeRetrievalResult, KnowledgeSearchMode, LocalEmbeddingModel, LocalKnowledgeBase, LocalKnowledgeBaseCreateRequest, LocalKnowledgeBaseDetail, LocalKnowledgeCatalogFile, LocalKnowledgeDocument, LocalKnowledgeFolder, LocalKnowledgeGateway, LocalKnowledgeImportFile, LocalKnowledgeStorageMigration, LocalKnowledgeStorageStatus, OperationAccepted, StorageMigrationStage, StorageMigrationStatus, VectorBackendHealth } from './model'
import { formatBytes, formatJobStage } from './model'
import { searchSnippetParts } from './search-snippet'
import { useLibraryBrowser, navigateLibraryFolder, setLibraryView, publishLibraryFolders } from './library-browser-state'
import { LibraryFileManager, LibraryViewSwitch } from './library-file-manager'
import { duplicateLibraryImportPaths, type LibraryFile } from './library-file-model'
import FolderComponent from '@/components/ui/folder-component'
import { LibraryRemoteSection } from './library-remote-section'
import { LocalFolderSection, LocalFolderDetailPage, LOCAL_FOLDERS_CHANGED } from './local-folder-pages'
import { Laptop } from 'lucide-react'
import type { NavigateWorkspace } from '@/features/workspace/types'
import '@/styles/local-knowledge.css'

const KnowledgePdfViewer = lazy(() => import('@/features/knowledge/knowledge-pdf-viewer'))
const KnowledgeDocxViewer = lazy(() => import('@/features/knowledge/knowledge-docx-viewer'))
const KnowledgePptxViewer = lazy(() => import('@/features/knowledge/knowledge-pptx-viewer'))
const KnowledgeSpreadsheetViewer = lazy(() => import('@/features/knowledge/knowledge-spreadsheet-viewer'))
const MAX_LOCAL_TEXT_PREVIEW_BYTES = 2 * 1024 * 1024
const KNOWLEDGE_JOB_PAGE_SIZE = 8
const RECENT_DOCUMENT_LIMIT = 8
const HOME_EXAMPLE_QUESTIONS = ['怎么把本机文件加入资料库？', '智能搜索和按字词查找有什么区别？']
const RECENT_DOCUMENT_STORAGE_KEY = 'fox.local-knowledge.recent-documents'
const LOCAL_KNOWLEDGE_ACTIVITY_STORAGE_KEY = 'fox.local-knowledge.activity-history'
const LOCAL_KNOWLEDGE_ACTIVITY_LIMIT = 80
const LOCAL_KNOWLEDGE_MASCOT = '/mascot/fox/status/fox_give_flowers.png'
const FOX_GUIDE_KNOWLEDGE_BASE_ID = 'fox-user-guide'

export type LocalKnowledgeView = 'home' | 'files' | 'list' | 'detail' | 'documents' | 'import' | 'jobs' | 'models' | 'retrieval' | 'settings'

interface RecentLocalKnowledgeDocument {
  knowledgeBaseId: string
  knowledgeBaseName: string
  documentId: string
  name: string
  extension: string
  relativePath: string
  openedAt: string
}

type KnowledgeDetailSubview = 'documents' | 'retrieval' | 'jobs'
type LocalKnowledgeActivityKind = 'document_deleted' | 'document_imported' | 'folder_created' | 'job_stage'
type KnowledgeActivityStatus = 'complete' | 'active' | 'warning'

interface LocalKnowledgeActivity {
  id: string
  knowledgeBaseId: string
  kind: LocalKnowledgeActivityKind
  title: string
  detail: string
  occurredAt: string
  jobId?: string
  jobStage?: KnowledgeOperationStage
  progress?: number
  status?: KnowledgeActivityStatus
}

interface KnowledgeActivityEvent {
  id: string
  title: string
  detail: string
  occurredAt: string
  progress: number
  status: KnowledgeActivityStatus
}

function readLocalKnowledgeActivities(): LocalKnowledgeActivity[] {
  if (typeof window === 'undefined') return []
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(LOCAL_KNOWLEDGE_ACTIVITY_STORAGE_KEY) ?? '[]')
    if (!Array.isArray(value)) return []
    return value.filter((item): item is LocalKnowledgeActivity => {
      if (!item || typeof item !== 'object') return false
      const record = item as Record<string, unknown>
      return ['id', 'knowledgeBaseId', 'kind', 'title', 'detail', 'occurredAt'].every((key) => typeof record[key] === 'string')
    }).slice(0, LOCAL_KNOWLEDGE_ACTIVITY_LIMIT)
  } catch {
    return []
  }
}

function saveLocalKnowledgeActivities(items: LocalKnowledgeActivity[]): void {
  if (typeof window === 'undefined') return
  window.localStorage.setItem(LOCAL_KNOWLEDGE_ACTIVITY_STORAGE_KEY, JSON.stringify(items.slice(0, LOCAL_KNOWLEDGE_ACTIVITY_LIMIT)))
}

function recordLocalKnowledgeActivity(activity: Omit<LocalKnowledgeActivity, 'id' | 'occurredAt'> & { occurredAt?: string }): void {
  const next: LocalKnowledgeActivity = {
    ...activity,
    id: `${Date.now()}-${Math.random().toString(36).slice(2)}`,
    occurredAt: activity.occurredAt ?? new Date().toISOString(),
  }
  saveLocalKnowledgeActivities([next, ...readLocalKnowledgeActivities()])
}

function recordImportedDocuments(knowledgeBaseId: string, files: Array<{ name: string; relativePath?: string }>): void {
  const current = readLocalKnowledgeActivities()
  const now = Date.now()
  const imported = files.slice(0, 20).map((file, index): LocalKnowledgeActivity => {
    const detail = file.relativePath || file.name
    const wasDeleted = current.some((item) => item.knowledgeBaseId === knowledgeBaseId && item.kind === 'document_deleted' && (item.detail === detail || item.detail.endsWith(`/${file.name}`)))
    return {
      id: `${now + index}-${Math.random().toString(36).slice(2)}`,
      knowledgeBaseId,
      kind: 'document_imported',
      title: wasDeleted ? '重新添加资料' : '添加资料',
      detail,
      occurredAt: new Date(now + index).toISOString(),
    }
  })
  saveLocalKnowledgeActivities([...imported.reverse(), ...current])
}

function knowledgeJobStageActivityTitle(job: KnowledgeJob, stage: KnowledgeOperationStage): string {
  if (job.type === 'delete') return stage === 'committing' ? '完成资料移除' : '移除资料'
  const titleByStage: Record<KnowledgeOperationStage, string> = {
    validating: job.type === 'import' ? '校验添加的文件' : '校验资料',
    copying: '复制添加的文件',
    hashing: '计算文件指纹',
    parsing: '读取资料',
    chunking: '切分段落',
    embedding: '整理内容',
    vector_upsert: '写入搜索数据',
    verifying: '校验搜索结果',
    committing: job.type === 'rebuild'
      ? '完成重新整理'
      : job.type === 'import'
        ? '完成资料添加'
        : job.type === 'parse'
          ? '完成资料读取'
          : '完成整理',
  }
  return titleByStage[stage]
}

function mergeKnowledgeJobStageActivities(knowledgeBaseId: string, jobs: KnowledgeJob[]): LocalKnowledgeActivity[] {
  let next = readLocalKnowledgeActivities()
  jobs.forEach((job) => {
    if (!job.stage) return
    const active = job.status === 'queued' || job.status === 'running' || job.status === 'paused'
    const warning = job.status === 'failed' || job.status === 'interrupted' || job.status === 'cancelled'
    const status: KnowledgeActivityStatus = warning ? 'warning' : active ? 'active' : 'complete'
    const activityId = `job-stage-${job.id}-${job.stage}`

    next = next.map((activity) => activity.kind === 'job_stage'
      && activity.jobId === job.id
      && activity.jobStage !== job.stage
      && activity.status === 'active'
      ? { ...activity, progress: 100, status: 'complete' }
      : activity)

    const activity: LocalKnowledgeActivity = {
      id: activityId,
      knowledgeBaseId,
      kind: 'job_stage',
      title: knowledgeJobStageActivityTitle(job, job.stage),
      detail: `${formatJobStage(job.stage)} · ${jobStatusLabel(job.status, job.outcome)}${active ? ` · ${job.progress}%` : ''}`,
      occurredAt: job.updatedAt,
      jobId: job.id,
      jobStage: job.stage,
      progress: job.progress,
      status,
    }
    next = [activity, ...next.filter((item) => item.id !== activityId)]
  })
  next.sort((left, right) => right.occurredAt.localeCompare(left.occurredAt))
  saveLocalKnowledgeActivities(next)
  return next
}

function readRecentDocuments(): RecentLocalKnowledgeDocument[] {
  if (typeof window === 'undefined') return []
  try {
    const value: unknown = JSON.parse(window.localStorage.getItem(RECENT_DOCUMENT_STORAGE_KEY) ?? '[]')
    if (!Array.isArray(value)) return []
    return value.filter((item): item is RecentLocalKnowledgeDocument => {
      if (!item || typeof item !== 'object') return false
      const record = item as Record<string, unknown>
      return ['knowledgeBaseId', 'knowledgeBaseName', 'documentId', 'name', 'extension', 'relativePath', 'openedAt']
        .every((key) => typeof record[key] === 'string')
    }).slice(0, RECENT_DOCUMENT_LIMIT)
  } catch {
    return []
  }
}

function rememberRecentDocument(base: LocalKnowledgeBase, document: LocalKnowledgeDocument): void {
  if (typeof window === 'undefined') return
  const next: RecentLocalKnowledgeDocument = {
    knowledgeBaseId: base.id,
    knowledgeBaseName: base.name,
    documentId: document.id,
    name: document.name,
    extension: document.extension,
    relativePath: document.relativePath,
    openedAt: new Date().toISOString(),
  }
  const recent = readRecentDocuments().filter((item) => item.knowledgeBaseId !== next.knowledgeBaseId || item.documentId !== next.documentId)
  window.localStorage.setItem(RECENT_DOCUMENT_STORAGE_KEY, JSON.stringify([next, ...recent].slice(0, RECENT_DOCUMENT_LIMIT)))
}

export interface LocalKnowledgePageProps {
  gateway?: LocalKnowledgeGateway
  className?: string
  onNavigateWorkspace?: NavigateWorkspace
}

export interface LocalKnowledgeNavigationProps extends LocalKnowledgePageProps {
  knowledgeBaseId?: string
  documentId?: string
  createRequest?: number
  onCreateRequestHandled?: () => void
  onBack?: () => void
  onNavigate?: (view: LocalKnowledgeView, knowledgeBaseId?: string, documentId?: string) => void
  onAskDocument?: (payload: LocalKnowledgeDocumentAsk) => void
}

type EmbeddedLocalKnowledgeNavigationProps = LocalKnowledgeNavigationProps & { embedded?: boolean }

function formatDate(value: string | null | undefined): string {
  if (!value) return '尚未记录'
  return new Date(value).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}

function formatRelativeDate(value: string | null | undefined): string {
  if (!value) return '尚未记录'
  const date = new Date(value)
  const today = new Date()
  const startOfToday = new Date(today.getFullYear(), today.getMonth(), today.getDate()).getTime()
  const startOfDate = new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
  const dayDelta = Math.round((startOfToday - startOfDate) / 86_400_000)
  const time = date.toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
  if (dayDelta === 0) return `今天 ${time}`
  if (dayDelta === 1) return `昨天 ${time}`
  return `${date.getMonth() + 1}月${date.getDate()}日 ${time}`
}

function displayLocalPath(value: string): string {
  if (value.startsWith('\\\\?\\UNC\\')) return `\\\\${value.slice(8)}`
  if (value.startsWith('\\\\?\\')) return value.slice(4)
  return value
}

function baseStatusLabel(status: LocalKnowledgeBase['status']): string {
  return ({ empty: '还没有资料', indexing: '正在整理', ready: '可以使用', error: '有错误', migrating: '迁移中' })[status]
}

function documentStatusLabel(status: LocalKnowledgeDocument['status']): string {
  return ({ queued: '排队中', parsing: '读取中', indexed: '已整理', error: '读取失败', stale: '待重新整理' })[status]
}

function jobStatusLabel(status: KnowledgeJobStatus, outcome: KnowledgeJob['outcome']): string {
  if (status === 'completed' && outcome === 'partial') return '只完成一部分'
  return ({ queued: '排队中', running: '进行中', paused: '已暂停', completed: '已完成', failed: '失败', cancelled: '已取消', interrupted: '已中断' })[status]
}

function storageMigrationStatusLabel(status: StorageMigrationStatus): string {
  return ({ queued: '等待迁移', running: '迁移中', paused: '已暂停', completed: '迁移完成', failed: '迁移失败', cancelled: '已取消', interrupted: '已中断' })[status]
}

function storageMigrationStageLabel(stage: StorageMigrationStage | null): string {
  return ({
    validating: '校验目标目录',
    quiescing: '暂停资料库任务',
    flushing: '写入并刷新索引',
    closing_handles: '释放文件句柄',
    copying: '复制资料库文件',
    verifying: '校验迁移结果',
    switching: '切换存储配置',
    reopening: '重新打开数据库',
    self_checking: '执行完整性检查',
    awaiting_cleanup: '等待清理旧目录',
    rolling_back: '回滚迁移',
  } as Record<StorageMigrationStage, string>)[stage ?? 'validating']
}

function storageMigrationActive(migration: LocalKnowledgeStorageMigration | null): boolean {
  return migration != null && ['queued', 'running', 'paused'].includes(migration.status)
}

export function StorageLocationDialog({ gateway, open, onOpenChange }: { gateway: LocalKnowledgeGateway; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [status, setStatus] = useState<LocalKnowledgeStorageStatus | null>(null)
  const [migration, setMigration] = useState<LocalKnowledgeStorageMigration | null>(null)
  const [selectedPath, setSelectedPath] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [picking, setPicking] = useState(false)
  const [starting, setStarting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const loadStatus = async () => {
    setLoading(true)
    try {
      setStatus(await gateway.getStorageStatus())
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => {
    if (open) void loadStatus()
  }, [gateway, open])

  useEffect(() => {
    const migrationId = status?.migrationId
    if (!migrationId || status?.restartRequired) return
    let disposed = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const poll = async () => {
      try {
        const next = await gateway.getStorageMigration(migrationId)
        if (disposed) return
        setMigration(next)
        if (storageMigrationActive(next)) {
          timer = globalThis.setTimeout(() => void poll(), 750)
          return
        }
        if (next.status === 'completed') {
          toast.success('本机资料库存储迁移已完成', { description: '请重启 Fox，使新的存储位置生效。', sourceType: 'knowledge_storage_migration', sourceId: migrationId })
          try {
            const refreshed = await gateway.getStorageStatus()
            if (!disposed) setStatus(refreshed)
          } catch {
          }
        } else if (next.error) {
          setError(next.error.message)
        }
      } catch (cause) {
        if (!disposed) setError(cause instanceof Error ? cause.message : String(cause))
      }
    }
    void poll()
    return () => {
      disposed = true
      if (timer) globalThis.clearTimeout(timer)
    }
  }, [gateway, status?.migrationId, status?.restartRequired])

  const migrationBusy = storageMigrationActive(migration) || Boolean(status?.migrationId && !status.restartRequired && !migration)
  const busy = loading || picking || starting || migrationBusy || Boolean(status?.restartRequired)
  const pendingPath = selectedPath ?? status?.pendingRootPath ?? null

  const chooseDirectory = async () => {
    if (busy) return
    setPicking(true)
    setError(null)
    try {
      const path = await gateway.pickStorageDirectory()
      if (path) {
        setSelectedPath(path)
      }
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setPicking(false)
    }
  }

  // Degraded at startup (「知识库路径不可写」): the migration cannot run (it
  // writes to the unwritable database), so the new path is only recorded and
  // used after restart; nothing is copied.
  const degraded = Boolean(status?.degradedReason)
  const resetPath = async (path: string | null) => {
    if (!gateway.resetStoragePath || starting || loading) return
    setStarting(true)
    setError(null)
    try {
      setStatus(await gateway.resetStoragePath(path))
      setSelectedPath(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setStarting(false)
    }
  }

  const confirmMigration = async () => {
    if (!selectedPath || busy) return
    setStarting(true)
    setError(null)
    try {
      const accepted = await gateway.startStorageMigration(selectedPath)
      setStatus((current) => current ? { ...current, pendingRootPath: selectedPath, migrationId: accepted.operationId, restartRequired: false } : current)
      setSelectedPath(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setStarting(false)
    }
  }

  const migrationError = migration?.error?.message
  const completed = status?.restartRequired || migration?.status === 'completed'

  return (
    <Dialog open={open} onOpenChange={(nextOpen) => { if (!migrationBusy && !starting) onOpenChange(nextOpen) }}>
      <DialogContent className="fox-local-kb-storage-dialog" showCloseButton={!migrationBusy && !starting}>
        <DialogHeader>
          <DialogTitle>保存位置</DialogTitle>
          <DialogDescription>{degraded ? '当前位置不可写，资料库暂时只读或不可用。选择一个可写的文件夹，重启 Fox 后使用新位置（原位置的数据不会被移动）。' : '选择新的根目录后，Fox 会自动迁移数据库、文件和本机索引。'}</DialogDescription>
        </DialogHeader>
        <div className="fox-local-kb-storage-body">
          {loading && <div className="fox-local-kb-storage-path is-loading"><LoaderCircle className="animate-spin" /><span>正在读取存储位置...</span></div>}
          {!loading && status && <>
            {status.degradedReason && <p className="fox-local-kb-storage-error" role="alert" data-testid="fox-kb-storage-degraded"><AlertCircle />{status.degradedReason}</p>}
            <label className="fox-local-kb-storage-label">当前位置</label>
            <div className="fox-local-kb-storage-path"><FolderOpen aria-hidden="true" /><code title={status.rootPath}>{status.rootPath}</code></div>
            {pendingPath && <><label className="fox-local-kb-storage-label">新位置</label><div className="fox-local-kb-storage-pending"><ArrowLeft aria-hidden="true" /><code title={pendingPath}>{pendingPath}</code></div></>}
            {migration && <div className="fox-local-kb-storage-progress" role="status" aria-live="polite"><div className="fox-local-kb-storage-progress-head"><span>{storageMigrationStatusLabel(migration.status)}</span><b>{migration.progress}%</b></div><Progress className="fox-local-kb-progress" value={migration.progress} aria-label="存储迁移进度" /><small>{storageMigrationStageLabel(migration.stage)}</small></div>}
            {completed && <p className="fox-local-kb-storage-success"><CheckCircle2 />{degraded ? '已记录新位置，重启 Fox 后生效。' : '迁移已完成，重启 Fox 后新存储位置才会生效。'}</p>}
            {(error || migrationError) && <p className="fox-local-kb-storage-error" role="alert"><AlertCircle />{migrationError ?? error}</p>}
          </>}
          {!loading && !status && error && <p className="fox-local-kb-storage-error" role="alert"><AlertCircle />{error}</p>}
        </div>
        <DialogFooter className="fox-local-kb-storage-actions">
          {!loading && !status && error ? <Button type="button" variant="outline" onClick={() => void loadStatus()}>重试</Button> : <>
            <Button type="button" variant="outline" onClick={() => void chooseDirectory()} disabled={busy}>{picking ? <LoaderCircle className="animate-spin" /> : <FolderOpen />}{picking ? '正在选择' : '选择文件夹'}</Button>
            {degraded && gateway.resetStoragePath ? <>
              <Button type="button" variant="outline" onClick={() => void resetPath(null)} disabled={busy}>恢复默认位置</Button>
              <Button type="button" onClick={() => void resetPath(selectedPath)} disabled={!selectedPath || busy}>{starting ? <LoaderCircle className="animate-spin" /> : <CheckCircle2 />}{starting ? '正在提交' : '使用此位置'}</Button>
            </> : <Button type="button" onClick={() => void confirmMigration()} disabled={!selectedPath || busy}>{starting ? <LoaderCircle className="animate-spin" /> : <CheckCircle2 />}{starting ? '正在提交' : '确认迁移'}</Button>}
          </>}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function LocalKnowledgeHeader({ title, description, onBack, actions }: { title: string; description: string; onBack?: () => void; actions?: ReactNode }) {
  return (
    <header className="fox-local-kb-header">
      <div className="fox-local-kb-header-main">
        {onBack && <Button aria-label="返回本机资料库" className="fox-local-kb-back" variant="ghost" size="icon" onClick={onBack}><ArrowLeft /></Button>}
        <div>
          <h1>{title}</h1>
          {description && <p>{description}</p>}
        </div>
      </div>
      {actions && <div className="fox-local-kb-header-actions">{actions}</div>}
    </header>
  )
}

function LocalKnowledgeLoading() {
  return <SkeletonContent className="fox-local-kb-loading" label="正在读取本机资料库..." lines={4} avatar />
}

function LocalKnowledgeError({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return <FoxEmptyState variant="error" icon={<AlertCircle />} title="资料库加载失败" description={message} action={onRetry && <Button size="sm" variant="outline" onClick={onRetry}>重试</Button>} />
}

function LocalKnowledgeEmpty({ title, description, action }: { title: string; description: string; action?: ReactNode }) {
  return <FoxEmptyState variant="empty" icon={<Database />} title={title} description={description} action={action} />
}

function isSelectableEmbeddingModel(model: LocalEmbeddingModel) {
  return model.status === 'ready' && model.integrityStatus === 'verified' && model.loadReady
}

function CreateKnowledgeBaseDialog({
  gateway,
  open,
  submitting,
  error,
  onOpenChange,
  onSubmit,
  onFolderCreated,
}: {
  gateway: LocalKnowledgeGateway
  onFolderCreated: (id: string) => void
  open: boolean
  submitting: boolean
  error: string | null
  onOpenChange: (open: boolean) => void
  onSubmit: (request: LocalKnowledgeBaseCreateRequest, files: LocalKnowledgeImportFile[]) => Promise<void>
}) {
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [models, setModels] = useState<LocalEmbeddingModel[]>([])
  const [embeddingModelId, setEmbeddingModelId] = useState('none')
  const [chunkSize, setChunkSize] = useState('512')
  const [chunkOverlap, setChunkOverlap] = useState('50')
  const [searchMode, setSearchMode] = useState<KnowledgeSearchMode>('keyword')
  const [moreOpen, setMoreOpen] = useState(false)
  const [fileSelections, setFileSelections] = useState<KnowledgeImportSelection[]>([])
  const files = fileSelections.filter((item) => item.status !== 'failed').map((item) => item.importFile)
  const [validationError, setValidationError] = useState<string | null>(null)
  const [modelsLoading, setModelsLoading] = useState(false)
  const [modelsLoadError, setModelsLoadError] = useState(false)
  const [step, setStep] = useState<1 | 2>(1)
  const [folderBusy, setFolderBusy] = useState(false)
  const busy = submitting || folderBusy

  useEffect(() => {
    if (!open) {
      setName('')
      setDescription('')
      setEmbeddingModelId('none')
      setChunkSize('512')
      setChunkOverlap('50')
      setSearchMode('keyword')
      setMoreOpen(false)
      setFileSelections([])
      setValidationError(null)
      setModels([])
      setModelsLoadError(false)
      setStep(1)
    }
  }, [open])

  useEffect(() => {
    if (!open) return
    let disposed = false
    setModelsLoading(true)
    setModelsLoadError(false)
    void gateway.listEmbeddingModels().then((items) => {
      if (disposed) return
      setModels(items)
      const selectable = items.filter(isSelectableEmbeddingModel)
      const preferred = selectable.find((item) => item.isDefault) ?? selectable[0]
      if (preferred) {
        setEmbeddingModelId(preferred.id)
        setSearchMode('hybrid')
      }
    }).catch(() => {
      if (disposed) return
      setModels([])
      setModelsLoadError(true)
    }).finally(() => {
      if (!disposed) setModelsLoading(false)
    })
    return () => { disposed = true }
  }, [gateway, open])

  const addFolder = async () => {
    setFolderBusy(true)
    setValidationError(null)
    try {
      const path = await gateway.pickFileSourceFolder()
      if (!path) return
      const source = await gateway.addFileSource(path)
      window.dispatchEvent(new Event(LOCAL_FOLDERS_CHANGED))
      onOpenChange(false)
      onFolderCreated(source.id)
    } catch (cause) {
      setValidationError(cause instanceof Error ? cause.message : String(cause))
    } finally { setFolderBusy(false) }
  }

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (files.length > LOCAL_IMPORT_MAX_FILES) { setValidationError(`单次最多导入 ${LOCAL_IMPORT_MAX_FILES} 个文件。请减少所选文件。`); return }
    const duplicates = duplicateLibraryImportPaths(files)
    if (duplicates.length) { setValidationError(`有同名文件会导入到同一位置：${duplicates.join('、')}。请返回挑选文件，移除重复项后再导入。`); return }
    if (step === 1) {
      if (!files.length || fileSelections.some((item) => item.status === 'failed')) { setValidationError('请先挑选可以导入的文件。'); return }
      setValidationError(null)
      setStep(2)
      return
    }
    if (fileSelections.some((item) => item.status === 'failed')) {
      setValidationError('请移除或重新选择无法导入的文件。')
      return
    }
    const normalizedName = name.trim()
    if (!normalizedName) {
      setValidationError('请输入资料库名称。')
      return
    }
    const normalizedChunkSize = Number(chunkSize)
    const normalizedChunkOverlap = Number(chunkOverlap)
    if (!Number.isInteger(normalizedChunkSize) || normalizedChunkSize < 128 || normalizedChunkSize > 4096) {
      setValidationError('段落长度需为 128–4096 的整数。')
      return
    }
    if (!Number.isInteger(normalizedChunkOverlap) || normalizedChunkOverlap < 0 || normalizedChunkOverlap >= normalizedChunkSize) {
      setValidationError('衔接长度需为非负整数，且必须小于段落长度。')
      return
    }
    if (searchMode !== 'keyword' && !models.some((model) => model.id === embeddingModelId && isSelectableEmbeddingModel(model))) {
      setValidationError('智能搜索需要选择一个已安装并测试通过的搜索模型。')
      return
    }
    setValidationError(null)
    void onSubmit({
      name: normalizedName,
      description: description.trim() || null,
      embeddingModelId: embeddingModelId === 'none' ? null : embeddingModelId,
      chunkSize: normalizedChunkSize,
      chunkOverlap: normalizedChunkOverlap,
      searchMode,
    }, files)
  }

  const requestOpenChange = (nextOpen: boolean) => {
    if (busy) return
    onOpenChange(nextOpen)
  }

  const selectableModels = models.filter(isSelectableEmbeddingModel)
  const chooseSearchMode = (smart: boolean) => {
    if (!smart) {
      setSearchMode('keyword')
      return
    }
    if (embeddingModelId === 'none') {
      const preferred = selectableModels.find((model) => model.isDefault) ?? selectableModels[0]
      if (preferred) setEmbeddingModelId(preferred.id)
    }
    setSearchMode('hybrid')
  }

  return (
    <Dialog open={open} onOpenChange={requestOpenChange}>
      <DialogContent className="fox-local-kb-create-dialog fox-kb-create-dialog" showCloseButton={!busy}>
        <DialogHeader>
          <div className="fox-library-create-steps" aria-label={`第 ${step} 步，共 3 步`}>{['挑选文件', '命名', '导入'].map((label, index) => <span key={label} className={step === index + 1 ? 'is-current' : step > index + 1 ? 'is-done' : ''} aria-current={step === index + 1 ? 'step' : undefined}><i>{index + 1}</i>{label}</span>)}</div>
          <DialogTitle>{step === 1 ? '新建资料库' : '给资料库起个名字'}</DialogTitle>
          <DialogDescription>{step === 1 ? '从电脑的不同位置挑选文件，可以多次添加。' : `${files.length} 个文件 · ${formatBytes(files.reduce((total, file) => total + file.sizeBytes, 0))}，导入后可以随时再添加。`}</DialogDescription>
        </DialogHeader>
        <form className="fox-local-kb-create-form" onSubmit={submit}>
          {step === 2 && <div>
            <label htmlFor="local-knowledge-base-name">名称 <span>*</span></label>
            <Input id="local-knowledge-base-name" autoFocus placeholder="例如：产品资料" value={name} onChange={(event) => { setName(event.target.value); setValidationError(null) }} disabled={busy} />
          </div>}
          {step === 1 && open && <><KnowledgeImportDropzone gateway={gateway} items={fileSelections} onItemsChange={setFileSelections} disabled={busy} onError={setValidationError} allowFolder={false} /><Button type="button" variant="outline" disabled={busy} onClick={() => void addFolder()}>{folderBusy ? <LoaderCircle className="animate-spin" /> : <FolderOpen />}将本机文件夹作为资料库</Button></>}
          {step === 1 && files.length > 0 && <div className="fox-library-picked-origins"><b>{files.length} 个文件 · 来自 {new Set(files.map((file) => file.sourcePath?.replace(/[\\/][^\\/]+$/, '') ?? '当前选择')).size} 个位置</b>{files.slice(0, 20).map((file, index) => <small key={`${file.sourcePath}:${index}`} title={file.sourcePath}>{file.sourcePath || file.relativePath || file.name}</small>)}</div>}
          {step === 2 && <div><label htmlFor="local-knowledge-base-description">简介 <small>（选填）</small></label><Textarea id="local-knowledge-base-description" placeholder="一句话说明这里放的是什么" value={description} onChange={(event) => setDescription(event.target.value)} disabled={busy} /></div>}
          {step === 2 && moreOpen && <div className="fox-kb-create-more">
            <label>搜索方式<Select value={searchMode === 'keyword' ? 'keyword' : 'smart'} onValueChange={(value) => chooseSearchMode(value === 'smart')} disabled={busy}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="keyword">按字词查找</SelectItem><SelectItem value="smart">智能搜索</SelectItem></SelectContent></Select></label>
            <p className="fox-kb-create-mode-hint">{searchMode === 'keyword' ? '速度快，适合找确切的名称和编号。' : '能理解意思相近的说法，需要先装好一个搜索模型。'}</p>
            <div className="fox-local-kb-index-settings">
              <h3 className="fox-local-kb-index-settings-title">高级</h3>
              <label>搜索模型<Select value={embeddingModelId} onValueChange={(value) => { if (value !== 'none' && !models.some((model) => model.id === value && isSelectableEmbeddingModel(model))) return; setEmbeddingModelId(value); if (value === 'none') setSearchMode('keyword') }} disabled={busy || modelsLoading}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="none">暂不使用</SelectItem>{models.map((model) => <SelectItem key={model.id} value={model.id} disabled={!isSelectableEmbeddingModel(model)}>{model.name} · {model.dimension} 维{isSelectableEmbeddingModel(model) ? '' : ` · ${modelStatusLabel(model)}`}</SelectItem>)}</SelectContent></Select></label>
              <label>段落长度<Input type="number" min={128} max={4096} step={1} value={chunkSize} onChange={(event) => setChunkSize(event.target.value)} disabled={busy} /></label>
              <label>衔接长度<Input type="number" min={0} step={1} value={chunkOverlap} onChange={(event) => setChunkOverlap(event.target.value)} disabled={busy} /></label>
              {modelsLoading && <p><LoaderCircle className="animate-spin" />正在读取搜索模型...</p>}
              {!modelsLoading && modelsLoadError && <p><AlertCircle />搜索模型列表暂不可用。可以先创建，稍后再编辑搜索方式。</p>}
              {!modelsLoading && !modelsLoadError && models.length === 0 && <p><Cpu />还没有可用的搜索模型。可先创建，再到“向量模型”页面安装。</p>}
              {!modelsLoading && models.length > 0 && !selectableModels.length && <p><Info />模型还没有安装并测试通过，可以先创建，之后再到“向量模型”页面完成。</p>}
            </div>
          </div>}
          {(validationError || error) && <p className="fox-local-kb-create-error" role="alert"><AlertCircle />{validationError ?? error}</p>}
          <DialogFooter className="fox-local-kb-create-actions">
            {step === 2 && <><Button type="button" variant="ghost" disabled={busy} onClick={() => setStep(1)}><ArrowLeft />上一步</Button><button type="button" className="fox-kb-create-more-toggle" aria-expanded={moreOpen} onClick={() => setMoreOpen((value) => !value)}><ChevronRight className={moreOpen ? 'is-open' : undefined} />更多选项</button></>}
            <Button type="button" variant="outline" onClick={() => requestOpenChange(false)} disabled={busy}>取消</Button>
            <Button type="submit" disabled={busy || !files.length || fileSelections.some((item) => item.status === 'failed')}>{submitting && <LoaderCircle className="animate-spin" />}{step === 1 ? '下一步' : '导入'}</Button>
          </DialogFooter>
        </form>
      </DialogContent>

    </Dialog>
  )
}

function KnowledgeBaseSettingsForm({
  gateway,
  base,
  open,
  submitting,
  error,
  readOnly = false,
  onOpenChange,
  onSubmit,
  onDelete,
  onDirtyChange,
}: {
  gateway: LocalKnowledgeGateway
  base: LocalKnowledgeBase
  open: boolean
  submitting: boolean
  error: string | null
  readOnly?: boolean
  onOpenChange: (open: boolean) => void
  onSubmit: (request: LocalKnowledgeBaseCreateRequest) => Promise<void>
  onDelete?: () => void
  onDirtyChange?: (dirty: boolean) => void
}) {
  const [name, setName] = useState(base.name)
  const [description, setDescription] = useState(base.description)
  const [models, setModels] = useState<LocalEmbeddingModel[]>([])
  const [embeddingModelId, setEmbeddingModelId] = useState(base.configuredEmbeddingModelId ?? 'none')
  const [chunkSize, setChunkSize] = useState(String(base.chunkSize))
  const [chunkOverlap, setChunkOverlap] = useState(String(base.chunkOverlap))
  const [searchMode, setSearchMode] = useState<KnowledgeSearchMode>(base.searchMode)
  const [advancedOpen, setAdvancedOpen] = useState(false)
  const [validationError, setValidationError] = useState<string | null>(null)
  const [dirty, setDirty] = useState(false)
  useEffect(() => { onDirtyChange?.(dirty) }, [dirty, onDirtyChange])

  useEffect(() => {
    if (!open) return
    setName(base.name)
    setDescription(base.description)
    setEmbeddingModelId(base.configuredEmbeddingModelId ?? 'none')
    setChunkSize(String(base.chunkSize))
    setChunkOverlap(String(base.chunkOverlap))
    setSearchMode(base.searchMode)
    setAdvancedOpen(false)
    setValidationError(null)
    setDirty(false)
    void gateway.listEmbeddingModels().then(setModels).catch(() => setModels([]))
  }, [base, gateway, open])

  const smart = searchMode !== 'keyword'
  const chooseSmart = (nextSmart: boolean) => {
    setDirty(true)
    if (!nextSmart) {
      setSearchMode('keyword')
      return
    }
    if (embeddingModelId === 'none') {
      const preferred = models.find((model) => model.isDefault && isSelectableEmbeddingModel(model)) ?? models.find(isSelectableEmbeddingModel)
      if (preferred) setEmbeddingModelId(preferred.id)
    }
    setSearchMode(searchMode === 'vector' ? 'vector' : 'hybrid')
  }

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const normalizedName = name.trim()
    if (!normalizedName) {
      setValidationError('请输入资料库名称。')
      return
    }
    const normalizedChunkSize = Number(chunkSize)
    const normalizedChunkOverlap = Number(chunkOverlap)
    if (!Number.isInteger(normalizedChunkSize) || normalizedChunkSize < 128 || normalizedChunkSize > 4096) {
      setValidationError('段落长度需为 128–4096 的整数。')
      return
    }
    if (!Number.isInteger(normalizedChunkOverlap) || normalizedChunkOverlap < 0 || normalizedChunkOverlap >= normalizedChunkSize) {
      setValidationError('衔接长度需为非负整数，且必须小于段落长度。')
      return
    }
    if (smart && !models.some((model) => model.id === embeddingModelId && isSelectableEmbeddingModel(model))) {
      setValidationError('智能搜索需要选择一个已安装并测试通过的搜索模型。')
      return
    }
    setValidationError(null)
    void onSubmit({
      name: normalizedName,
      description: description.trim() || null,
      embeddingModelId: embeddingModelId === 'none' ? '' : embeddingModelId,
      chunkSize: normalizedChunkSize,
      chunkOverlap: normalizedChunkOverlap,
      searchMode,
    })
  }

  // One discard confirmation at a time; the dialog closes only after the user
  // confirms (await), exactly as the synchronous check did before.
  const discardConfirmingRef = useRef(false)
  const requestOpenChange = (nextOpen: boolean) => {
    if (submitting || discardConfirmingRef.current) return
    if (nextOpen || !dirty) {
      onOpenChange(nextOpen)
      return
    }
    discardConfirmingRef.current = true
    void confirmDialog({ title: '放弃修改', description: '放弃尚未保存的资料库修改？', confirmLabel: '放弃修改', cancelLabel: '继续编辑', destructive: true }).then((confirmed) => {
      discardConfirmingRef.current = false
      if (confirmed) onOpenChange(false)
    })
  }

  return (
    <section className="fox-kb-settings-panel">
        {readOnly && <p className="fox-kb-settings-description">这是 Fox 自带的资料库，名称和搜索方式不能改，这里可以看它的存放位置和整理情况。</p>}
        <form className={cn('fox-kb-settings-form', readOnly && 'is-readonly')} onSubmit={submit}>
          <div>
            <label htmlFor="local-knowledge-base-edit-name">名称 {!readOnly && <span>*</span>}</label>
            <Input id="local-knowledge-base-edit-name" autoFocus value={name} onChange={(event) => { setName(event.target.value); setValidationError(null); setDirty(true) }} disabled={submitting || readOnly} />
          </div>
          <div>
            <label htmlFor="local-knowledge-base-edit-description">简介 <small>（可选）</small></label>
            <Textarea id="local-knowledge-base-edit-description" className="fox-local-kb-create-textarea" value={description} onChange={(event) => { setDescription(event.target.value); setDirty(true) }} disabled={submitting || readOnly} />
          </div>
          <div className="fox-kb-settings-field">
            <span className="fox-kb-settings-label">搜索方式</span>
            <div className="fox-kb-settings-modes" role="radiogroup" aria-label="搜索方式">
              <label className={cn('fox-kb-settings-mode', !smart && 'is-on')}>
                <input type="radio" name="local-knowledge-search-mode" checked={!smart} disabled={submitting || readOnly} onChange={() => chooseSmart(false)} />
                <span><b>按字词查找</b><small>速度快，适合找确切的名称和编号。</small></span>
              </label>
              <label className={cn('fox-kb-settings-mode', smart && 'is-on')}>
                <input type="radio" name="local-knowledge-search-mode" checked={smart} disabled={submitting || readOnly} onChange={() => chooseSmart(true)} />
                <span><b>智能搜索<em>推荐</em></b><small>能理解意思相近的说法。保存后会重新整理一次。</small></span>
              </label>
            </div>
          </div>
          <button type="button" className="fox-kb-settings-disclose" aria-expanded={advancedOpen} onClick={() => setAdvancedOpen((value) => !value)}><ChevronRight className={advancedOpen ? 'is-open' : undefined} />高级<small>{readOnly ? '存放位置、段落、上次整理' : '段落长度、搜索模型、存放位置'}</small></button>
          {advancedOpen && <div className="fox-kb-settings-advanced">
            <label>搜索模型<Select value={embeddingModelId} onValueChange={(value) => { if (value !== 'none' && !models.some((model) => model.id === value && isSelectableEmbeddingModel(model))) return; setEmbeddingModelId(value); if (value === 'none') setSearchMode('keyword'); setDirty(true) }} disabled={submitting}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="none">暂不使用</SelectItem>{models.map((model) => <SelectItem key={model.id} value={model.id} disabled={!isSelectableEmbeddingModel(model)}>{model.name} · {model.dimension} 维{isSelectableEmbeddingModel(model) ? '' : ` · ${modelStatusLabel(model)}`}</SelectItem>)}</SelectContent></Select></label>
            <div className="fox-local-kb-advanced-fields">
              <label>段落长度<Input type="number" min={128} max={4096} step={1} value={chunkSize} onChange={(event) => { setChunkSize(event.target.value); setDirty(true) }} disabled={submitting} /></label>
              <label>衔接长度<Input type="number" min={0} step={1} value={chunkOverlap} onChange={(event) => { setChunkOverlap(event.target.value); setDirty(true) }} disabled={submitting} /></label>
            </div>
            <dl className="fox-kb-settings-facts">
              <div><dt>存放位置</dt><dd title={base.storagePath}>{displayLocalPath(base.storagePath)}</dd></div>
              <div><dt>资料</dt><dd>{base.documentCount} 份</dd></div>
              <div><dt>段落</dt><dd>{countLabel(base.chunkCount)}</dd></div>
              <div><dt>上次整理</dt><dd>{formatDate(base.lastIndexedAt)}</dd></div>
            </dl>
          </div>}
          {(validationError || error) && <p className="fox-local-kb-create-error" role="alert"><AlertCircle />{validationError ?? error}</p>}
          <div className="fox-kb-settings-foot">
            {onDelete && <Button type="button" variant="outline" className="fox-kb-settings-delete" onClick={onDelete} disabled={submitting}><Trash2 />删除资料库</Button>}
            <Button type="button" variant="outline" onClick={() => requestOpenChange(false)} disabled={submitting}>{readOnly ? '关闭' : '取消'}</Button>
            {!readOnly && <Button type="submit" disabled={submitting}>{submitting && <LoaderCircle className="animate-spin" />}保存</Button>}
          </div>
        </form>
    </section>
  )
}

function LocalKnowledgeSettingsDialog({ gateway = defaultLocalKnowledgeGateway, knowledgeBaseId, onNavigate, onBack, onSaved }: LocalKnowledgeNavigationProps & { onSaved?: (base: LocalKnowledgeBase) => void }) {
  const [base, setBase] = useState<LocalKnowledgeBaseDetail | null>(null)
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [dirty, setDirty] = useState(false)
  const load = useCallback(async () => {
    if (!knowledgeBaseId) { setLoading(false); return }
    setLoading(true)
    try { setBase(await gateway.getKnowledgeBase(knowledgeBaseId)); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }, [gateway, knowledgeBaseId])
  useEffect(() => { void load() }, [load])
  const returnToList = () => { onBack?.() }
  const discardConfirmingRef = useRef(false)
  const requestReturnToList = () => {
    if (saving || discardConfirmingRef.current) return
    if (!dirty) {
      returnToList()
      return
    }
    discardConfirmingRef.current = true
    void confirmDialog({ title: '放弃修改', description: '放弃尚未保存的资料库修改？', confirmLabel: '放弃修改', cancelLabel: '继续编辑', destructive: true }).then((confirmed) => {
      discardConfirmingRef.current = false
      if (confirmed) returnToList()
    })
  }
  const save = async (request: LocalKnowledgeBaseCreateRequest) => {
    if (!base) return
    setSaving(true)
    setError(null)
    try {
      const updated = await gateway.updateKnowledgeBase(base.id, request)
      setBase((current) => current ? { ...current, ...updated } : current)
      setDirty(false)
      toast.success('已保存资料库设置。')
      onSaved?.(updated)
      onBack?.()
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setSaving(false) }
  }
  const remove = async () => {
    if (!base || base.id === FOX_GUIDE_KNOWLEDGE_BASE_ID) return
    setSaving(true)
    setError(null)
    try { await gateway.deleteKnowledgeBase(base.id); returnToList(); onNavigate?.('list') }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setSaving(false) }
  }
  const content = <>
    {loading && <LocalKnowledgeLoading />}
    {!loading && !base && (error ? <LocalKnowledgeError message={error} onRetry={() => void load()} /> : <LocalKnowledgeEmpty title="没有选择资料库" description="请从本机资料库进入设置。" />)}
    {!loading && base && <KnowledgeBaseSettingsForm gateway={gateway} base={base} open submitting={saving} error={deleteOpen ? null : error} onDirtyChange={setDirty} onOpenChange={(open) => { if (!open) returnToList() }} onSubmit={save} onDelete={base.id === FOX_GUIDE_KNOWLEDGE_BASE_ID ? undefined : () => { setError(null); setDeleteOpen(true) }} />}
    {base && base.id !== FOX_GUIDE_KNOWLEDGE_BASE_ID && <DeleteKnowledgeBaseDialog base={base} open={deleteOpen} submitting={saving} error={deleteOpen ? error : null} onOpenChange={(open) => { if (!saving) { setDeleteOpen(open); setError(null) } }} onConfirm={remove} />}
  </>
  return <Dialog open onOpenChange={(open) => { if (!open) requestReturnToList() }}><DialogContent className="fox-kb-settings-dialog" showCloseButton={!saving}><DialogHeader><DialogTitle>资料库设置</DialogTitle><DialogDescription>{base?.name || '管理资料库名称和搜索方式。'}</DialogDescription></DialogHeader>{content}</DialogContent></Dialog>
}

function DeleteKnowledgeBaseDialog({
  base,
  open,
  submitting,
  error,
  onOpenChange,
  onConfirm,
}: {
  base: LocalKnowledgeBase
  open: boolean
  submitting: boolean
  error: string | null
  onOpenChange: (open: boolean) => void
  onConfirm: () => Promise<void>
}) {
  return (
    <Dialog open={open} onOpenChange={(nextOpen) => { if (!submitting) onOpenChange(nextOpen) }}>
      <DialogContent className="fox-local-kb-create-dialog fox-local-kb-delete-dialog" showCloseButton={!submitting}>
        <DialogHeader>
          <DialogTitle>删除资料库</DialogTitle>
          <DialogDescription>只删除资料库里的资料副本和搜索结果，电脑上的原文件不受影响。</DialogDescription>
        </DialogHeader>
        <div className="fox-local-kb-delete-summary"><Trash2 /><div><strong>{base.name}</strong><span>{base.documentCount} 份资料副本会被删除，无法恢复</span></div></div>
        {error && <p className="fox-local-kb-create-error" role="alert"><AlertCircle />{error}</p>}
        <DialogFooter className="fox-local-kb-create-actions">
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)} disabled={submitting}>取消</Button>
          <Button type="button" variant="destructive" onClick={() => void onConfirm()} disabled={submitting}>{submitting ? <LoaderCircle className="animate-spin" /> : <Trash2 />}{submitting ? '正在删除' : '确认删除'}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}


function countLabel(value: number | null): string {
  return value == null ? '暂无统计' : String(value)
}


function searchModeLabel(base: LocalKnowledgeBase): string {
  if (!base.vectorIndexReady) return base.textIndexReady ? '仅按字词查找' : '按字词查找未就绪'
  if ((base.searchMode === 'vector' || base.searchMode === 'hybrid') && base.vectorIndexReady) return '智能搜索'
  return '按字词查找'
}

function KnowledgeBaseCard({ base, job, onOpen, onEdit, onDelete }: { base: LocalKnowledgeBase; job: KnowledgeJob | null; onOpen: () => void; onEdit: () => void; onDelete: () => void }) {
  const progress = job ? Math.max(0, Math.min(100, job.progress)) : 0
  const managed = base.id !== FOX_GUIDE_KNOWLEDGE_BASE_ID
  return (
    <article className="fox-kb-cover-card">
      <button type="button" className="fox-kb-card-open" onClick={onOpen} aria-label={`打开资料库${base.name}`}>
      <span className="fox-kb-cover" aria-hidden="true">
        <FolderComponent color="theme" size="xs" />
      </span>
      <span className="fox-kb-cover-body">
        <strong>{base.name}<span className="fox-library-source-label"><Laptop />本机</span></strong>
        <p title={base.description || undefined}>{base.description?.trim() || '无简介'}</p>
        {job
          ? <span className="fox-kb-cover-meta is-busy"><i className="fox-status-dot is-brand" aria-hidden="true" />正在整理<Progress asChild className="fox-local-kb-progress" value={progress} aria-label={`${base.name} 整理进度`}><span /></Progress><b>{progress}%</b></span>
          : <span className="fox-kb-cover-meta">{base.documentCount} 份资料{base.status === 'error' && <span className="fox-kb-cover-state is-error">{baseStatusLabel(base.status)}</span>}</span>}
      </span>
      </button>
      <span className="fox-kb-cover-menu" onClick={(event) => event.stopPropagation()} onKeyDown={(event) => event.stopPropagation()}>
        <DropdownMenu>
          <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-kb-cover-more" aria-label={`${base.name} 的更多操作`}><MoreHorizontal /></Button></DropdownMenuTrigger>
          <DropdownMenuContent align="end" sideOffset={5}>
            <DropdownMenuItem onSelect={onEdit}><Pencil />编辑信息</DropdownMenuItem>
            {managed && <><DropdownMenuSeparator /><DropdownMenuItem className="fox-local-kb-menu-danger" onSelect={onDelete}><Trash2 />删除资料库</DropdownMenuItem></>}
          </DropdownMenuContent>
        </DropdownMenu>
      </span>
    </article>
  )
}

function localCatalogMetadata(file: LocalKnowledgeCatalogFile): KnowledgeDocumentSourceMetadata {
  return {
    filename: file.name,
    mediaType: file.mimeType,
    size: file.sizeBytes,
    sourceRevision: `local:${file.id}:${file.modifiedAt ?? 'unknown'}:${file.sizeBytes}`,
    versionId: null,
    etag: null,
    lastModified: file.modifiedAt,
    acceptsRanges: true,
    previewApiVersion: 1,
    supportedPreviewVariants: ['source'],
    availableVariants: ['source'],
  }
}

function LocalFilePreviewDialog({ file, gateway, open, onOpenChange }: { file: LocalKnowledgeCatalogFile | null; gateway: LocalKnowledgeGateway; open: boolean; onOpenChange: (open: boolean) => void }) {
  const [text, setText] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const kind = file ? previewKind(file.name) : 'unsupported'
  const metadata = useMemo(() => file ? localCatalogMetadata(file) : null, [file])
  const openPreviewSource = useCallback<KnowledgeCachedPreviewOpener>(async ({ maxBytes, signal }) => {
    if (!file || !metadata) throw new Error('没有选择需要预览的本机文件。')
    if (file.sizeBytes > maxBytes) throw new Error(`${file.name} 超过内置预览大小限制，请使用系统应用打开。`)
    return createKnowledgeCachedPreviewSource({
      metadata,
      cacheKey: `local-file:${file.id}`,
      size: file.sizeBytes,
      signal,
      readCache: (start, end) => gateway.readLocalFileRange(file.id, start, end),
      releaseCache: async () => true,
    })
  }, [file, gateway, metadata])

  useEffect(() => {
    if (!open || !file || (kind !== 'markdown' && kind !== 'text')) {
      setText('')
      setLoading(false)
      setError(null)
      return
    }
    let disposed = false
    const load = async () => {
      setLoading(true)
      setError(null)
      try {
        const previewBytes = Math.min(file.sizeBytes, MAX_LOCAL_TEXT_PREVIEW_BYTES)
        const bytes = previewBytes > 0
          ? binaryValue(await gateway.readLocalFileRange(file.id, 0, previewBytes - 1))
          : new Uint8Array()
        if (!disposed) setText(new TextDecoder('utf-8', { fatal: false }).decode(bytes))
      } catch (cause) {
        if (!disposed) setError(cause instanceof Error ? cause.message : String(cause))
      } finally {
        if (!disposed) setLoading(false)
      }
    }
    void load()
    return () => { disposed = true }
  }, [file, gateway, kind, open])

  const handleFailure = useCallback((message: string) => setError(
    /os error [23]\b|系统找不到指定的(文件|路径)|No such file or directory/i.test(message)
      ? '原文件已移动或删除。请返回文件夹，点击刷新更新文件列表。'
      : message,
  ), [])
  const openWithSystem = async () => {
    if (!file) return
    try {
      await gateway.openLocalFile(file.id)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }
  const revealInFolder = async () => {
    if (!file) return
    try {
      await gateway.revealLocalFile(file.id)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  const preview = file && metadata && !error && (
    kind === 'pdf'
      ? <KnowledgePdfViewer knowledgeBaseId="local-files" documentId={file.id} filename={file.name} metadata={metadata} onFailure={handleFailure} openPreviewSource={openPreviewSource} />
      : kind === 'docx'
        ? <KnowledgeDocxViewer knowledgeBaseId="local-files" documentId={file.id} filename={file.name} metadata={metadata} onFailure={handleFailure} openPreviewSource={openPreviewSource} />
        : kind === 'presentation' && file.extension.toLowerCase() === 'pptx'
          ? <KnowledgePptxViewer knowledgeBaseId="local-files" documentId={file.id} filename={file.name} metadata={metadata} onFailure={handleFailure} openPreviewSource={openPreviewSource} />
          : kind === 'spreadsheet'
            ? <KnowledgeSpreadsheetViewer knowledgeBaseId="local-files" documentId={file.id} filename={file.name} metadata={metadata} onFailure={handleFailure} openPreviewSource={openPreviewSource} />
            : kind === 'markdown'
              ? <div className="fox-local-files-markdown-preview"><MessageResponse>{text}</MessageResponse></div>
              : kind === 'text'
                ? <pre className="fox-local-files-text-preview">{text}</pre>
                : null
  )
  const unsupported = kind === 'unsupported' || kind === 'legacy-office' || (kind === 'presentation' && file?.extension.toLowerCase() !== 'pptx')

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="fox-local-files-preview-dialog" showCloseButton={false}>
        <header className="fox-file-preview-toolbar">
          <DialogTitle className="fox-file-preview-title" title={file?.absolutePath}>{file?.name ?? '本机文件预览'}</DialogTitle>
          {file && <div className="fox-file-preview-meta" title={`${file.extension.toUpperCase()} · ${formatBytes(file.sizeBytes)} · ${formatDate(file.modifiedAt)}`}>{file.extension.toUpperCase()} · {formatBytes(file.sizeBytes)} · {formatDate(file.modifiedAt)}</div>}
          <div className="fox-file-preview-actions">
            <Button variant="ghost" size="icon" aria-label="打开所在文件夹" title="打开所在文件夹" onClick={() => void revealInFolder()}><FolderOpen /></Button>
            <Button variant="ghost" size="icon" aria-label="系统打开" title="系统打开" onClick={() => void openWithSystem()}><ExternalLink /></Button>
            <DialogClose asChild><Button variant="ghost" size="icon" aria-label="关闭弹窗" title="关闭预览"><X /></Button></DialogClose>
          </div>
        </header>
        <DialogDescription className="sr-only">{file?.absolutePath}</DialogDescription>
        <div className="fox-local-files-preview-body">
          {loading && <div className="fox-file-preview-state is-loading"><LoaderCircle className="animate-spin" /><b>正在准备预览</b></div>}
          {!loading && !error && !unsupported && <Suspense fallback={<div className="fox-file-preview-state is-loading"><LoaderCircle className="animate-spin" /><b>正在加载查看器</b></div>}>{preview}</Suspense>}
          {!loading && (unsupported || error) && <div className="fox-file-preview-state"><FileQuestion /><b>{error ? '预览加载失败' : '暂不支持内置预览'}</b><p>{error ?? '当前文件格式暂不支持在 Fox 中直接预览。'}</p><Button onClick={() => void openWithSystem()}><ExternalLink />系统打开</Button></div>}
        </div>
      </DialogContent>
    </Dialog>
  )
}

function localDocumentMetadata(document: LocalKnowledgeDocument): KnowledgeDocumentSourceMetadata {
  return {
    filename: document.name,
    mediaType: document.mimeType,
    size: document.sizeBytes,
    sourceRevision: document.sourceRevision ?? `local:${document.id}:${document.updatedAt}`,
    versionId: null,
    etag: null,
    lastModified: document.updatedAt,
    acceptsRanges: true,
    previewApiVersion: 1,
    supportedPreviewVariants: ['source'],
    availableVariants: ['source'],
  }
}

function LocalKnowledgeDocumentReader({ knowledgeBaseId, knowledgeBaseName, document, gateway, onBack, onDeleted, onJobAccepted, onAskDocument }: { knowledgeBaseId: string; knowledgeBaseName?: string; document: LocalKnowledgeDocument; gateway: LocalKnowledgeGateway; onBack: () => void; onDeleted: () => Promise<void>; onJobAccepted: () => void; onAskDocument?: (payload: LocalKnowledgeDocumentAsk) => void; embedded?: boolean }) {
  const [text, setText] = useState('')
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [chunks, setChunks] = useState<KnowledgeChunkPreview[]>([])
  const [chunksOpen, setChunksOpen] = useState(false)
  const [chunksLoading, setChunksLoading] = useState(false)
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [actionBusy, setActionBusy] = useState(false)
  const kind = previewKind(document.name)
  const metadata = useMemo(() => localDocumentMetadata(document), [document])
  const openPreviewSource = useCallback<KnowledgeCachedPreviewOpener>(async ({ maxBytes, signal }) => {
    if (document.sizeBytes > maxBytes) throw new Error(`${document.name} 超过内置预览大小限制，请使用系统应用打开。`)
    return createKnowledgeCachedPreviewSource({
      metadata,
      cacheKey: `local-knowledge:${knowledgeBaseId}:${document.id}:${document.sourceRevision ?? document.updatedAt}`,
      size: document.sizeBytes,
      signal,
      readCache: (start, end) => gateway.readDocumentFileRange(knowledgeBaseId, document.id, start, end),
      releaseCache: async () => true,
    })
  }, [document, gateway, knowledgeBaseId, metadata])

  useEffect(() => {
    if (kind !== 'markdown' && kind !== 'text') {
      setText('')
      setLoading(false)
      setError(null)
      return
    }
    let disposed = false
    setLoading(true)
    setError(null)
    const previewBytes = Math.min(document.sizeBytes, MAX_LOCAL_TEXT_PREVIEW_BYTES)
    void (previewBytes > 0
      ? gateway.readDocumentFileRange(knowledgeBaseId, document.id, 0, previewBytes - 1)
      : Promise.resolve([]))
      .then((value) => {
        if (!disposed) setText(new TextDecoder('utf-8', { fatal: false }).decode(binaryValue(value)))
      })
      .catch((cause) => {
        if (!disposed) setError(cause instanceof Error ? cause.message : String(cause))
      })
      .finally(() => {
        if (!disposed) setLoading(false)
      })
    return () => { disposed = true }
  }, [document, gateway, kind, knowledgeBaseId])

  const openWithSystem = async () => {
    try { await gateway.openDocumentFile(knowledgeBaseId, document.id) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }
  const revealInFolder = async () => {
    try { await gateway.revealDocumentFile(knowledgeBaseId, document.id) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }
  const openChunks = async () => {
    setChunksOpen(true)
    setChunksLoading(true)
    setError(null)
    try { setChunks(await gateway.listDocumentChunks(knowledgeBaseId, document.id, { limit: 100, offset: 0 })) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setChunksLoading(false) }
  }
  const reparse = async () => {
    setActionBusy(true)
    setError(null)
    try { await gateway.reparseDocument(knowledgeBaseId, document.id); onJobAccepted() }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setActionBusy(false) }
  }
  const remove = async () => {
    setActionBusy(true)
    setError(null)
    try {
      await gateway.deleteDocument(knowledgeBaseId, document.id)
      recordLocalKnowledgeActivity({ knowledgeBaseId, kind: 'document_deleted', title: '移除资料', detail: document.relativePath || document.name })
      setDeleteOpen(false)
      await onDeleted()
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setActionBusy(false) }
  }
  const handleFailure = useCallback((message: string) => setError(message), [])
  const unsupported = kind === 'unsupported' || kind === 'legacy-office' || (kind === 'presentation' && document.extension.toLowerCase() !== 'pptx')
  const preview = !error && (
    kind === 'pdf'
      ? <KnowledgePdfViewer knowledgeBaseId={knowledgeBaseId} documentId={document.id} filename={document.name} metadata={metadata} onFailure={handleFailure} openPreviewSource={openPreviewSource} />
      : kind === 'docx'
        ? <KnowledgeDocxViewer knowledgeBaseId={knowledgeBaseId} documentId={document.id} filename={document.name} metadata={metadata} onFailure={handleFailure} openPreviewSource={openPreviewSource} />
        : kind === 'presentation' && document.extension.toLowerCase() === 'pptx'
          ? <KnowledgePptxViewer knowledgeBaseId={knowledgeBaseId} documentId={document.id} filename={document.name} metadata={metadata} onFailure={handleFailure} openPreviewSource={openPreviewSource} />
          : kind === 'spreadsheet'
            ? <KnowledgeSpreadsheetViewer knowledgeBaseId={knowledgeBaseId} documentId={document.id} filename={document.name} metadata={metadata} onFailure={handleFailure} openPreviewSource={openPreviewSource} />
            : kind === 'markdown'
              ? <div className="fox-local-files-markdown-preview"><MessageResponse>{text}</MessageResponse></div>
              : kind === 'text'
                ? <pre className="fox-local-files-text-preview">{text}</pre>
                 : null
  )
  const copyText = async (value: string, message: string) => {
    try {
      await navigator.clipboard.writeText(value)
      toast.success(message)
    } catch {
      setError('无法访问剪贴板，请手动复制。')
    }
  }
  const readerMenu = <DropdownMenu>
    <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" aria-label="更多操作"><MoreHorizontal /></Button></DropdownMenuTrigger>
    <DropdownMenuContent className="fox-kb-reader-menu" align="end" sideOffset={5}>
      <DropdownMenuItem onSelect={() => void openChunks()}><Layers3 />查看分段</DropdownMenuItem>
      <DropdownMenuItem disabled={actionBusy} onSelect={() => void reparse()}><RefreshCw />重新读取</DropdownMenuItem>
      <DropdownMenuItem onSelect={() => void copyText(document.relativePath, '已复制文件路径。')}><Copy />复制路径</DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem variant="destructive" onSelect={() => setDeleteOpen(true)}><Trash2 />从资料库移除</DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>

  return (
    <section className="fox-local-kb-document-reader fox-kb-reader">
      <div className="fox-kb-reader-bar">
        <Button aria-label="返回资料列表" variant="ghost" size="icon" onClick={onBack}><ArrowLeft /></Button>
        <nav className="fox-kb-reader-crumb" aria-label="资料位置">
          {knowledgeBaseName && <span title={knowledgeBaseName}>{knowledgeBaseName}</span>}
          {document.relativePath.split('/').slice(0, -1).map((part) => <span key={part}><ChevronRight aria-hidden="true" />{part}</span>)}
          <ChevronRight aria-hidden="true" /><b title={document.name}>{document.name}</b>
        </nav>
        <div className="fox-local-kb-document-reader-meta" title={`${formatBytes(document.sizeBytes)} · ${formatDate(document.updatedAt)} 更新 · ${documentStatusLabel(document.status)}`}><LocalFileTypeIcon extension={document.extension} /><span>{formatBytes(document.sizeBytes)} · {formatDate(document.updatedAt)} 更新</span>{document.status !== 'indexed' && <span className="fox-kb-reader-state"><i className={cn('fox-status-dot', document.status === 'error' ? 'is-error' : 'is-empty')} aria-hidden="true" />{documentStatusLabel(document.status)}</span>}</div>
        <div className="fox-kb-reader-actions">
          <Button variant="ghost" size="icon" aria-label="问问这份资料" title="问问这份资料" disabled={!onAskDocument} onClick={() => onAskDocument?.({ knowledgeBaseId, knowledgeBaseName: knowledgeBaseName ?? knowledgeBaseId, documentId: document.id, documentName: document.name, relativePath: document.relativePath, mimeType: document.mimeType, sizeBytes: document.sizeBytes })}><MessageCircle /></Button>
          <Button variant="ghost" size="icon" aria-label="系统打开" title="系统打开" onClick={() => void openWithSystem()}><ExternalLink /></Button>
          <Button variant="ghost" size="icon" aria-label="在文件夹中显示" title="在文件夹中显示" onClick={() => void revealInFolder()}><FolderOpen /></Button>
          {readerMenu}
        </div>
      </div>
      <div className="fox-local-kb-document-reader-body">
        {loading && <div className="fox-file-preview-state is-loading"><LoaderCircle className="animate-spin" /><b>正在准备预览</b></div>}
        {!loading && !error && !unsupported && <Suspense fallback={<div className="fox-file-preview-state is-loading"><LoaderCircle className="animate-spin" /><b>正在加载查看器</b></div>}>{preview}</Suspense>}
        {!loading && (unsupported || error) && <div className="fox-file-preview-state"><FileQuestion /><b>{error ? '预览加载失败' : '暂不支持内置预览'}</b><p>{error ?? '当前文件格式暂不支持在 Fox 中直接预览。'}</p><Button onClick={() => void openWithSystem()}><ExternalLink />系统打开</Button></div>}
      </div>
      <Dialog open={chunksOpen} onOpenChange={setChunksOpen}>
        <DialogContent className="fox-local-kb-chunks-dialog">
          <DialogHeader><DialogTitle>查看分段</DialogTitle><DialogDescription>检查读取后的段落边界、锚点和顺序。当前最多显示 100 段。</DialogDescription></DialogHeader>
          {chunksLoading && <LocalKnowledgeLoading />}
          {!chunksLoading && chunks.length === 0 && <LocalKnowledgeEmpty title="还没有分段" description="请先重新读取这份资料。" />}
          {!chunksLoading && chunks.length > 0 && <div className="fox-local-kb-chunk-list">{chunks.map((chunk) => <article key={chunk.id}><header><strong>第 {chunk.chunkIndex + 1} 段</strong><span>{chunk.anchor ?? '无锚点'}{chunk.startOffset != null && chunk.endOffset != null ? ` · ${chunk.startOffset}–${chunk.endOffset}` : ''}</span></header><p>{chunk.content}</p></article>)}</div>}
        </DialogContent>
      </Dialog>
      <Dialog open={deleteOpen} onOpenChange={(open) => { if (!actionBusy) setDeleteOpen(open) }}>
        <DialogContent className="fox-local-kb-create-dialog" showCloseButton={!actionBusy}>
          <DialogHeader><DialogTitle>从资料库移除</DialogTitle><DialogDescription>只删除这份资料在资料库里的副本和分段，电脑上的原文件不变。</DialogDescription></DialogHeader>
          <div className="fox-local-kb-delete-summary"><Trash2 /><div><strong>{document.name}</strong><span>{document.relativePath}</span></div></div>
          <DialogFooter><Button variant="outline" onClick={() => setDeleteOpen(false)} disabled={actionBusy}>取消</Button><Button variant="destructive" onClick={() => void remove()} disabled={actionBusy}>{actionBusy ? <LoaderCircle className="animate-spin" /> : <Trash2 />}确认删除</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  )
}

export function LocalKnowledgeHomePage({ gateway = defaultLocalKnowledgeGateway, className, onNavigate }: LocalKnowledgePageProps & { onNavigate?: (view: LocalKnowledgeView, knowledgeBaseId?: string, documentId?: string) => void }) {
  const [bases, setBases] = useState<LocalKnowledgeBase[]>([])
  const [jobs, setJobs] = useState<KnowledgeJob[]>([])
  const [recent] = useState(readRecentDocuments)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [example, setExample] = useState('')
  const [showAllRecent, setShowAllRecent] = useState(false)
  const loadRevision = useRef(0)

  const load = async () => {
    const revision = ++loadRevision.current
    setLoading(true)
    const basesRequest = gateway.listKnowledgeBases()
    void gateway.listJobs().then((items) => { if (revision === loadRevision.current) setJobs(items) }).catch(() => {})
    try {
      const nextBases = await basesRequest
      if (revision !== loadRevision.current) return
      setBases(nextBases)
      setError(null)
    } catch (cause) {
      if (revision === loadRevision.current) setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      if (revision === loadRevision.current) setLoading(false)
    }
  }

  useEffect(() => { void load(); return () => { loadRevision.current += 1 } }, [gateway])

  const availableBaseIds = useMemo(() => new Set(bases.map((base) => base.id)), [bases])
  const visibleRecent = useMemo(() => recent
    .filter((item) => loading || error || availableBaseIds.has(item.knowledgeBaseId))
    .slice(0, showAllRecent ? recent.length : RECENT_DOCUMENT_LIMIT), [availableBaseIds, recent, showAllRecent, loading, error])
  const guide = bases.find((base) => base.id === FOX_GUIDE_KNOWLEDGE_BASE_ID)
  // There is no cross-knowledge-base search, so the box queries one library:
  // the one you last read, otherwise the first one you created.
  const searchBase = useMemo(() => bases.find((base) => base.id === recent[0]?.knowledgeBaseId)
    ?? bases.find((base) => base.id !== FOX_GUIDE_KNOWLEDGE_BASE_ID) ?? bases[0] ?? null, [bases, recent])
  const baseNameById = useMemo(() => new Map(bases.map((base) => [base.id, base.name])), [bases])
  const runningJob = useMemo(() => jobs.filter((job) => job.status === 'queued' || job.status === 'running' || job.status === 'paused')
    .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))[0] ?? null, [jobs])
  const totalDocuments = bases.reduce((total, base) => total + base.documentCount, 0)

  return (
    <div className={cn('fox-local-kb-page fox-local-kb-home-page', className)}>
      {runningJob && <button type="button" className="fox-kb-home-jobline" onClick={() => onNavigate?.('jobs')}>
        <LoaderCircle className="animate-spin" aria-hidden="true" />
        <span>正在整理「{baseNameById.get(runningJob.knowledgeBaseId) ?? runningJob.knowledgeBaseId}」</span>
        <Progress asChild className="fox-kb-home-jobline-bar" value={runningJob.progress} aria-label="资料整理进度"><span /></Progress>
        <b>{runningJob.progress}%</b>
      </button>}
      <div className="fox-local-kb-content fox-local-kb-home-content">
        <section className="fox-local-kb-home-hero">
          <div className="fox-local-kb-home-greeting">
            <img src={LOCAL_KNOWLEDGE_MASCOT} alt="" />
            <div>
              <h1>今天想找点什么？</h1>
              <p>{bases.length} 个资料库，{totalDocuments} 份资料</p>
            </div>
          </div>
          {searchBase && <KnowledgeBaseSearch
            key={example}
            gateway={gateway}
            base={searchBase}
            initialQuery={example}
            omni
            hint={null}
            below={<p className="fox-kb-home-try"><span>试试</span>{HOME_EXAMPLE_QUESTIONS.map((question) => <button type="button" key={question} onClick={() => setExample(question)}><CornerDownLeft />{question}</button>)}</p>}
            onOpenAdvanced={() => onNavigate?.('retrieval', searchBase.id)}
            onOpenDocument={(documentId) => onNavigate?.('documents', searchBase.id, documentId)}
          />}
        </section>

        {error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}

        <section className="fox-local-kb-home-recent">
          <div className="fox-kb-home-shead">
            <h3>最近查看</h3>
            {recent.length > RECENT_DOCUMENT_LIMIT && <button type="button" onClick={() => setShowAllRecent((current) => !current)}>{showAllRecent ? '收起' : '全部'}<ChevronRight /></button>}
          </div>
          {visibleRecent.length > 0 && <div className="fox-local-kb-recent-list">
            {visibleRecent.map((item) => <button type="button" key={`${item.knowledgeBaseId}:${item.documentId}`} onClick={() => onNavigate?.('documents', item.knowledgeBaseId, item.documentId)}><LocalFileTypeIcon extension={item.extension} /><span className="fox-local-kb-recent-name">{item.name}</span><span className="fox-local-kb-recent-kb">{item.knowledgeBaseName}</span><time className="fox-local-kb-recent-time">{formatRelativeDate(item.openedAt)}</time></button>)}
          </div>}
          {!loading && visibleRecent.length === 0 && <div className="fox-local-kb-home-recent-empty"><p>阅读过的资料会出现在这里。</p></div>}
          {loading && recent.length === 0 && <div className="fox-local-kb-recent-loading"><LoaderCircle className="animate-spin" />正在读取最近打开的资料...</div>}
        </section>

        {guide && <p className="fox-kb-home-foot"><BookOpen aria-hidden="true" />第一次使用？<button type="button" onClick={() => onNavigate?.('detail', guide.id)}>看看使用指南</button></p>}
      </div>
    </div>
  )
}

export function LocalKnowledgeListPage({ gateway = defaultLocalKnowledgeGateway, className, onNavigate, onNavigateWorkspace, createRequest, onCreateRequestHandled, initialEditId }: LocalKnowledgePageProps & { initialEditId?: string; onNavigate?: (view: LocalKnowledgeView, knowledgeBaseId?: string) => void; createRequest?: number; onCreateRequestHandled?: () => void }) {
  const [editId, setEditId] = useState<string | null>(initialEditId ?? null)
  useEffect(() => { if (initialEditId) setEditId(initialEditId) }, [initialEditId])
  const closeEditor = () => { setEditId(null); if (initialEditId) onNavigate?.('list') }
  const [items, setItems] = useState<LocalKnowledgeBase[]>([])
  const [jobs, setJobs] = useState<KnowledgeJob[]>([])
  const [query, setQuery] = useState('')
  const [searchExpanded, setSearchExpanded] = useState(false)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [createOpen, setCreateOpen] = useState(false)
  const [storageOpen, setStorageOpen] = useState(false)
  // 「知识库路径不可写：<路径>」 when startup degraded the knowledge store.
  const [storageIssue, setStorageIssue] = useState<string | null>(null)
  useEffect(() => {
    let disposed = false
    void gateway.getStorageStatus().then((status) => { if (!disposed) setStorageIssue(status.degradedReason ?? null) }).catch(() => undefined)
    return () => { disposed = true }
  }, [gateway, storageOpen])
  const [creating, setCreating] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)
  const [deleteBase, setDeleteBase] = useState<LocalKnowledgeBase | null>(null)
  const [saving, setSaving] = useState(false)
  const [mutationError, setMutationError] = useState<string | null>(null)
  const [fileMatches, setFileMatches] = useState<LocalKnowledgeCatalogFile[]>([])
  const [fileSearchError, setFileSearchError] = useState(false)
  const [matchedPreviewFile, setMatchedPreviewFile] = useState<LocalKnowledgeCatalogFile | null>(null)
  const loadRevision = useRef(0)

  const load = async () => {
    const revision = ++loadRevision.current
    setLoading(true)
    const basesRequest = gateway.listKnowledgeBases()
    void gateway.listJobs().then((items) => { if (revision === loadRevision.current) setJobs(items) }).catch(() => {})
    try {
      const nextItems = await basesRequest
      if (revision !== loadRevision.current) return
      setItems(nextItems)
      setError(null)
    }
    catch (cause) { if (revision === loadRevision.current) setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { if (revision === loadRevision.current) setLoading(false) }
  }

  useEffect(() => { void load(); return () => { loadRevision.current += 1 } }, [gateway])
  useEffect(() => {
    let disposed = false
    if (!query.trim()) { setFileMatches([]); setFileSearchError(false); return }
    const timer = window.setTimeout(() => { void gateway.listLocalFiles({ query: query.trim(), limit: 8 }).then((files) => { if (!disposed) { setFileMatches(files); setFileSearchError(false) } }).catch(() => { if (!disposed) setFileSearchError(true) }) }, 200)
    return () => { disposed = true; window.clearTimeout(timer) }
  }, [gateway, query])
  useEffect(() => {
    if (![...activeJobByBase.values()].length) return
    const timer = window.setInterval(() => { void gateway.listJobs().then(setJobs).catch(() => {}); void gateway.listKnowledgeBases().then(setItems).catch(() => {}) }, 1800)
    return () => window.clearInterval(timer)
  }, [gateway, jobs.some((job) => ['queued', 'running', 'paused'].includes(job.status))])

  // A card shows one progress bar instead of a status dot, so only the newest
  // unfinished job per knowledge base is surfaced.
  const activeJobByBase = useMemo(() => {
    const next = new Map<string, KnowledgeJob>()
    for (const job of jobs) {
      if (job.status !== 'queued' && job.status !== 'running' && job.status !== 'paused') continue
      const current = next.get(job.knowledgeBaseId)
      if (!current || current.updatedAt < job.updatedAt) next.set(job.knowledgeBaseId, job)
    }
    return next
  }, [jobs])

  const openCreateDialog = () => {
    setCreateError(null)
    setCreateOpen(true)
  }

  useEffect(() => {
    if (createRequest == null || createRequest <= 0) return
    openCreateDialog()
    onCreateRequestHandled?.()
  }, [createRequest, onCreateRequestHandled])

  const createKnowledgeBase = async (request: LocalKnowledgeBaseCreateRequest, files: LocalKnowledgeImportFile[]) => {
    setCreating(true)
    setCreateError(null)
    try {
      const created = await gateway.createKnowledgeBase(request)
      if (files.length) {
        // The library exists even if the first import fails, so this must not
        // surface as "creation failed".
        try {
          recordImportedDocuments(created.id, files)
          await gateway.startImport({ knowledgeBaseId: created.id, files })
        } catch {
          toast.error('资料库已创建，但资料添加失败，可以在资料库页面重试。')
        }
      }
      setCreateOpen(false)
      await load()
      toast.success(`已创建「${created.name}」`, { description: '资料在后台导入，可以继续浏览。' })
    } catch (cause) {
      setCreateError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setCreating(false)
    }
  }

  const removeBase = async () => {
    if (!deleteBase) return
    setSaving(true)
    setMutationError(null)
    try {
      await gateway.deleteKnowledgeBase(deleteBase.id)
      setDeleteBase(null)
      await load()
    } catch (cause) {
      setMutationError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSaving(false)
    }
  }

  const filteredItems = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase()
    return items.filter((item) => !normalizedQuery || [item.name, item.description, item.storagePath].some((value) => value.toLocaleLowerCase().includes(normalizedQuery)))
  }, [items, query])

  return (
    <div className={cn('fox-local-kb-page fox-library-unified', className)}>
      <div className="fox-local-kb-content fox-local-kb-list-content">
        <LocalKnowledgeHeader title="资料库" description="集中浏览本机资料和 Yuxi 远程资料。" actions={<>
          <div className={cn("fox-library-list-search", searchExpanded && "is-expanded")}><ExpandingSearch onExpandedChange={setSearchExpanded} anchor="end" label="搜索所有资料库和本机文件" placeholder="搜索资料库和本机文件" items={[]} value={query} onValueChange={setQuery} showResults={false} keyboardShortcut clearLabel="清空搜索" expandedWidth={320} className="fox-kb-expanding-search" /></div>
          <Button variant="ghost" size="icon" aria-label="存储位置" title="存储位置" onClick={() => setStorageOpen(true)}><HardDrive /></Button>
          <Button onClick={openCreateDialog}><Plus />新建资料库</Button>
        </>} />
        {storageIssue && <div className="fox-local-kb-storage-banner" role="alert" data-testid="fox-kb-storage-banner"><AlertCircle /><span>{storageIssue}<small>资料库暂时只读或不可用。</small></span><Button variant="outline" size="sm" onClick={() => setStorageOpen(true)}>修改存储位置</Button></div>}
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        <LocalFolderSection gateway={gateway} query={query} libraryCount={items.length} matchedLibraryCount={filteredItems.length} librariesLoading={loading || Boolean(error)} onOpen={(id) => onNavigate?.('files', id)}>
          {!loading && !error && filteredItems.map((base) => <KnowledgeBaseCard key={base.id} base={base} job={activeJobByBase.get(base.id) ?? null} onOpen={() => onNavigate?.('detail', base.id)} onEdit={() => setEditId(base.id)} onDelete={() => { setMutationError(null); setDeleteBase(base) }} />)}
        </LocalFolderSection>
        {query.trim() && <div className="fox-library-search-results"><h3>匹配的本机文件</h3>{fileSearchError ? <p>本机文件搜索暂不可用。</p> : fileMatches.length ? fileMatches.map((file) => <button type="button" key={file.id} onClick={() => setMatchedPreviewFile(file)}><LocalFileTypeIcon extension={file.extension} /><span><b>{file.name}</b><small>{file.absolutePath}</small></span><ChevronRight /></button>) : <p>没有匹配的已收录文件。</p>}</div>}
        <LibraryRemoteSection query={query} navigate={onNavigateWorkspace} />
      </div>
      <LocalFilePreviewDialog gateway={gateway} file={matchedPreviewFile} open={Boolean(matchedPreviewFile)} onOpenChange={(open) => { if (!open) setMatchedPreviewFile(null) }} />
      {editId && <LocalKnowledgeSettingsDialog key={editId} gateway={gateway} knowledgeBaseId={editId} onBack={closeEditor} onSaved={(updated) => setItems((current) => current.map((item) => item.id === updated.id ? { ...item, ...updated } : item))} onNavigate={() => { closeEditor(); void load() }} />}
      <StorageLocationDialog gateway={gateway} open={storageOpen} onOpenChange={setStorageOpen} />
      <CreateKnowledgeBaseDialog onFolderCreated={(id) => onNavigate?.('files', id)} gateway={gateway} open={createOpen} submitting={creating} error={createError} onOpenChange={(open) => { setCreateOpen(open); if (open) setCreateError(null) }} onSubmit={createKnowledgeBase} />
      {deleteBase && <DeleteKnowledgeBaseDialog base={deleteBase} open submitting={saving} error={mutationError} onOpenChange={(open) => { if (!open) setDeleteBase(null) }} onConfirm={removeBase} />}
    </div>
  )
}

/** Marks the matched words inside a search snippet. Returns the plain text when
 *  lower-casing would change its length, so indices cannot drift apart. */
function SearchSnippet({ content, query }: { content: string; query: string }) {
  const snippet = searchSnippetParts(content, query)
  return <p className={cn('fox-kb-hit-snippet', !snippet.match && 'has-no-match')} title={content}>
    <span className="fox-kb-hit-before">{snippet.prefixEllipsis && '…'}{snippet.before}</span>
    {snippet.match && <mark>{snippet.match}</mark>}
    <span className="fox-kb-hit-after">{snippet.after}{snippet.suffixEllipsis && '…'}</span>
  </p>
}

/** The hub's search box: it replaces the old 检索测试 tab, so hits read as
 *  paragraphs instead of a scoring table. */
function KnowledgeBaseSearch({ gateway, base, hint, initialQuery = '', omni = false, controlContainer, below, onActiveChange, onOpenAdvanced, onOpenDocument }: {
  gateway: LocalKnowledgeGateway
  base: LocalKnowledgeBase
  hint: { text: string; label: string; run: () => void } | null
  initialQuery?: string
  omni?: boolean
  controlContainer?: HTMLElement | null
  below?: ReactNode
  onActiveChange?: (active: boolean) => void
  onOpenAdvanced: () => void
  onOpenDocument: (documentId: string) => void
}) {
  const { preferences: shortcutPreferences } = useShortcutPreferences()
  const searchBinding = shortcutPreferences.search
  const searchShortcut = searchBinding ? formatShortcut(searchBinding, detectShortcutPlatform()).text : null
  const searchLabel = `在${base.name}里搜索${searchShortcut ? `（${searchShortcut}）` : ''}`
  const [query, setQuery] = useState(initialQuery)
  const [term, setTerm] = useState(initialQuery)
  const [mode, setMode] = useState<KnowledgeSearchMode>(base.searchMode)
  const [items, setItems] = useState<KnowledgeRetrievalResult['items']>([])
  const [searched, setSearched] = useState(false)
  const [running, setRunning] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const searchRevision = useRef(0)
  // 智能搜索 needs a vector index; without one the box can only match words.
  const canSearchSmart = base.vectorIndexReady
  const activeMode: KnowledgeSearchMode = mode === 'keyword' || canSearchSmart ? mode : 'keyword'

  const inputRef = useRef<HTMLInputElement>(null)
  // The current library consumes search before the global sidebar listener.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (controlContainer !== undefined || document.querySelector('[role="dialog"]')) return
      if (!matchesShortcutAction(event, 'search') || !inputRef.current) return
      event.preventDefault()
      inputRef.current?.focus()
      inputRef.current?.select()
    }
    window.addEventListener('keydown', onKeyDown, true)
    return () => window.removeEventListener('keydown', onKeyDown, true)
  }, [controlContainer])

  const clearSearch = () => {
    searchRevision.current += 1
    setQuery(''); setTerm(''); setItems([]); setSearched(false); setRunning(false); setError(null)
  }

  useEffect(() => () => { searchRevision.current += 1 }, [])

  const run = (nextTerm: string, nextMode = activeMode) => {
    if (!nextTerm.trim()) { clearSearch(); return }
    const revision = ++searchRevision.current
    setTerm(nextTerm)
    setMode(nextMode)
    setRunning(true)
    setError(null)
    setSearched(true)
    void gateway.testRetrieval(base.id, nextTerm, nextMode, 10).then((result) => {
      if (revision === searchRevision.current) setItems(result.items)
    }).catch((cause) => {
      if (revision !== searchRevision.current) return
      setItems([])
      setError(cause instanceof Error ? cause.message : String(cause))
    }).finally(() => { if (revision === searchRevision.current) setRunning(false) })
  }

  // The home page hands over a suggested question, which should search at once.
  useEffect(() => { if (initialQuery) run(initialQuery) }, [initialQuery])

  // 原型 05-kb-search：一旦进入搜索结果，下面的资料列表就让位给结果。
  useEffect(() => {
    onActiveChange?.(searched)
    return () => onActiveChange?.(false)
  }, [searched, onActiveChange])

  const submit = (event: FormEvent) => {
    event.preventDefault()
    const next = query.trim()
    if (!next) { clearSearch(); return }
    run(next)
  }

  const copyCitation = () => {
    const first = items[0]
    if (!first) return
    void navigator.clipboard?.writeText(`${first.documentName} · ${first.relativePath}`)
  }

  const copyItemCitation = (item: KnowledgeRetrievalResult['items'][number]) => {
    void navigator.clipboard?.writeText(`${item.documentName} · ${item.relativePath}\n${item.content}`)
  }

  return <div className={cn('fox-kb-search-block', omni && 'is-omni')}>
    {controlContainer !== undefined ? controlContainer && createPortal(<ExpandingSearch reserveSpace anchor="end" label={searchLabel} placeholder="搜索资料内容，或直接问一个问题" items={[]} value={query} onValueChange={(next) => { if (!next.trim()) clearSearch(); else setQuery(next) }} onSearch={(next) => run(next)} showResults={false} keyboardShortcut clearLabel="清空搜索" expandedWidth={360} className="fox-kb-expanding-search" />, controlContainer) : <form className="fox-kb-search" role="search" onSubmit={submit}>
      <Search aria-hidden="true" />
      <input ref={inputRef} value={query} onChange={(event) => setQuery(event.target.value)} aria-label={`在${base.name}里搜索`} placeholder={omni ? '搜索资料内容，或直接问一个问题' : `在「${base.name}」里搜索，或直接提问`} />
      {query && <button type="button" aria-label="清空搜索" onClick={clearSearch}><XCircle /></button>}
      {omni && searchShortcut && <kbd className="fox-kb-search-scope" aria-hidden="true">{searchShortcut}</kbd>}
    </form>}
    {below}
    {hint && <p className="fox-kb-hub-hint"><i className="fox-status-dot is-empty" aria-hidden="true" /><span>{hint.text}</span><button type="button" onClick={hint.run}>{hint.label}</button></p>}
    {searched && <div className="fox-kb-search-bar">
        <Select value={activeMode} onValueChange={(value) => run(term, value as KnowledgeSearchMode)}>
          <SelectTrigger className="fox-kb-search-pick" aria-label="搜索方式"><span>{activeMode === 'keyword' ? '按字词查找' : '智能搜索'}</span></SelectTrigger>
          <SelectContent><SelectItem value="keyword">按字词查找</SelectItem>{canSearchSmart && <SelectItem value="hybrid">智能搜索</SelectItem>}</SelectContent>
        </Select>
      <span>{running ? '正在查找…' : `找到 ${items.length} 段相关内容`}</span>
      <div className="fox-kb-search-bar-spacer" />
      <DropdownMenu>
        <DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" aria-label="搜索结果更多操作"><MoreHorizontal /></Button></DropdownMenuTrigger>
        <DropdownMenuContent align="end" sideOffset={5}>
          <DropdownMenuItem onSelect={onOpenAdvanced}><FlaskConical />高级检索对比</DropdownMenuItem>
          <DropdownMenuItem disabled={!items.length} onSelect={copyCitation}><Quote />复制第一条引用</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>}
    {error && <p className="fox-local-kb-create-error" role="alert"><AlertCircle />{error}</p>}
    {searched && !running && !error && items.length === 0 && <FoxEmptyState variant="search" icon={<Search />} title="没有找到相关内容" description="换个说法再试试。" action={<Button variant="outline" size="sm" onClick={clearSearch}>清空搜索</Button>} />}
    {items.map((item, index) => <article className="fox-kb-hit" key={item.chunkId} tabIndex={0} onClick={() => onOpenDocument(item.documentId)} onKeyDown={(event) => { if (event.key === 'Enter') onOpenDocument(item.documentId) }}>
      <header>
        <LocalFileTypeIcon extension={item.documentName.split('.').pop() ?? ''} />
        <b title={item.relativePath}>{item.documentName}</b>
        <span className="fox-kb-hit-folder">{item.relativePath.split('/').slice(0, -1).join('/') || '全部资料'}</span>
        {index === 0 && <span className="fox-kb-hit-rel"><i className="fox-status-dot" aria-hidden="true" />最相关</span>}
        <span className="fox-kb-hit-acts" onClick={(event) => event.stopPropagation()} onKeyDown={(event) => event.stopPropagation()}>
          <Button variant="ghost" size="icon-sm" aria-label="复制这段引用" title="复制这段引用" onClick={() => copyItemCitation(item)}><Quote /></Button>
          <Button variant="ghost" size="icon-sm" aria-label="打开这份资料" title="打开这份资料" onClick={() => onOpenDocument(item.documentId)}><FileText /></Button>
        </span>
      </header>
      <SearchSnippet content={item.content} query={term} />
    </article>)}
  </div>
}

function KnowledgeActivityClock({ jobs, knowledgeBaseName, onOpenJobs }: {
  jobs: KnowledgeJob[]
  knowledgeBaseName: string
  onOpenJobs: () => void
}) {
  const busy = jobs.some((job) => job.status === 'queued' || job.status === 'running' || job.status === 'paused')
  return (
    <Popover>
      <PopoverTrigger asChild><Button variant="ghost" size="icon" aria-label="最近动态" className={cn('fox-kb-activity-trigger', busy && 'is-busy')}><Clock3 /></Button></PopoverTrigger>
      <PopoverContent align="end" sideOffset={6} className="fox-kb-activity-pop">
        <p className="fox-kb-activity-title">最近动态</p>
        {jobs.length === 0 && <p className="fox-kb-activity-empty">还没有整理记录。</p>}
        {jobs.slice(0, 4).map((job) => <button type="button" className="fox-kb-activity-row" key={job.id} onClick={onOpenJobs}>
          {job.status === 'failed' || job.status === 'interrupted' ? <AlertCircle /> : job.status === 'cancelled' ? <XCircle /> : job.status === 'running' || job.status === 'queued' ? <LoaderCircle className="animate-spin" /> : <CheckCircle2 />}
          <span><b>{knowledgeJobTitle(job, knowledgeBaseName)}</b><small>{formatRelativeDate(job.updatedAt)}</small></span>
        </button>)}
        <button type="button" className="fox-kb-activity-all" onClick={onOpenJobs}><ListTodo />查看全部处理进度</button>
      </PopoverContent>
    </Popover>
  )
}

export function LocalKnowledgeDetailPage({ gateway = defaultLocalKnowledgeGateway, className, knowledgeBaseId, documentId, onBack, onNavigate, onOperationAccepted, onAskDocument, initialImportOpen = false }: LocalKnowledgeNavigationProps & { initialImportOpen?: boolean; onOperationAccepted?: (accepted: OperationAccepted) => void }) {
  const [settingsOpen, setSettingsOpen] = useState(false)
  const browser = useLibraryBrowser(knowledgeBaseId)
  const [addRequest, setAddRequest] = useState<'file' | 'folder' | null>(null)
  const clearAddRequest = useCallback(() => setAddRequest(null), [])
  useEffect(() => { if (initialImportOpen) setAddRequest('file') }, [initialImportOpen, knowledgeBaseId])
  useEffect(() => { setActiveSubview(null); setActiveDocumentId(undefined) }, [browser.navigation])
  const [base, setBase] = useState<LocalKnowledgeBaseDetail | null>(null)
  const [configuredEmbeddingModel, setConfiguredEmbeddingModel] = useState<LocalEmbeddingModel | null>(null)
  const [modelsLoading, setModelsLoading] = useState(true)
  const [defaultEmbeddingModel, setDefaultEmbeddingModel] = useState<LocalEmbeddingModel | null>(null)
  const [jobs, setJobs] = useState<KnowledgeJob[]>([])
  const [activeSubview, setActiveSubview] = useState<KnowledgeDetailSubview | null>(null)
  const [activeDocumentId, setActiveDocumentId] = useState<string | undefined>(documentId)
  const [searchShowing, setSearchShowing] = useState(false)
  const [searchContainer, setSearchContainer] = useState<HTMLDivElement | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [mutationError, setMutationError] = useState<string | null>(null)
  const hadActiveJobsRef = useRef(false)
  const loadRevision = useRef(0)

  const leaveSubviewOrDetail = () => {
    if (activeSubview) {
      setActiveDocumentId(undefined)
      setActiveSubview(null)
      return
    }
    if (activeDocumentId) {
      setActiveDocumentId(undefined)
      return
    }
    onBack?.()
  }

  const navigateWithinSubview = (view: LocalKnowledgeView, nextKnowledgeBaseId?: string, documentId?: string) => {
    if (view === 'documents') {
      setActiveSubview(null)
      setActiveDocumentId(documentId)
      return
    }
    if (view === 'retrieval' || view === 'jobs') {
      setActiveSubview(view)
      setActiveDocumentId(undefined)
      return
    }
    onNavigate?.(view, nextKnowledgeBaseId, documentId)
  }

  const load = async () => {
    if (!knowledgeBaseId) return
    const revision = ++loadRevision.current
    setLoading(true)
    setModelsLoading(true)
    setConfiguredEmbeddingModel(null)
    setDefaultEmbeddingModel(null)
    const baseRequest = gateway.getKnowledgeBase(knowledgeBaseId)
    const models = gateway.listEmbeddingModels().catch(() => [] as LocalEmbeddingModel[])
    try {
      const nextBase = await baseRequest
      if (revision !== loadRevision.current) return
      setBase(nextBase)
      void models.then((embeddingModels) => {
        if (revision !== loadRevision.current) return
        setModelsLoading(false)
        setConfiguredEmbeddingModel(nextBase.configuredEmbeddingModelId ? embeddingModels.find((model) => model.id === nextBase.configuredEmbeddingModelId) ?? null : null)
        setDefaultEmbeddingModel(embeddingModels.find((model) => model.isDefault && model.status === 'ready' && model.integrityStatus === 'verified' && model.loadReady) ?? null)
      })
      const resolvedJobs = nextBase.recentJobs
      hadActiveJobsRef.current = resolvedJobs.some((job) => job.status === 'queued' || job.status === 'running' || job.status === 'paused')
      setJobs(resolvedJobs)
      setError(null)
    }
    catch (cause) { if (revision === loadRevision.current) setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { if (revision === loadRevision.current) setLoading(false) }
  }

  useEffect(() => { void load(); return () => { loadRevision.current += 1 } }, [gateway, knowledgeBaseId])

  useEffect(() => {
    setActiveSubview(null)
    setActiveDocumentId(documentId)
  }, [knowledgeBaseId, documentId])

  useEffect(() => {
    if (!knowledgeBaseId) return
    let disposed = false
    let refreshing = false
    const refreshActivity = async () => {
      if (refreshing) return
      refreshing = true
      try {
        const nextJobs = await gateway.listJobs(knowledgeBaseId)
        if (disposed) return
        setJobs(nextJobs)
        const hasActiveJobs = nextJobs.some((job) => job.status === 'queued' || job.status === 'running' || job.status === 'paused')
        const shouldRefreshBase = hasActiveJobs || hadActiveJobsRef.current
        hadActiveJobsRef.current = hasActiveJobs
        if (shouldRefreshBase) {
          const nextBase = await gateway.getKnowledgeBase(knowledgeBaseId)
          if (!disposed) setBase(nextBase)
        }
      } catch {
        // The detail page remains usable with the last successful activity snapshot.
      } finally {
        refreshing = false
      }
    }
    const timer = window.setInterval(() => void refreshActivity(), 1800)
    return () => { disposed = true; window.clearInterval(timer) }
  }, [gateway, knowledgeBaseId])

  const activeJobSubscriptionKey = jobs
    .filter((job) => job.status === 'queued' || job.status === 'running' || job.status === 'paused')
    .map((job) => job.operationId)
    .join('|')

  useEffect(() => {
    if (!knowledgeBaseId || !activeJobSubscriptionKey) return
    let disposed = false
    const activeJobs = jobs.filter((job) => job.status === 'queued' || job.status === 'running' || job.status === 'paused')
    const unlisten = activeJobs.map((job) => gateway.subscribeOperation(job.operationId, (event) => {
      if (disposed) return
      const observedState = reduceOperationEvent(createOperationState(job.operationId), event)
      setJobs((current) => current.map((item) => item.id === job.id ? operationStateForJob(item, observedState) : item))
    }))
    return () => { disposed = true; for (const stop of unlisten) stop() }
  }, [activeJobSubscriptionKey, gateway, knowledgeBaseId])

  const configuredEmbeddingModelReady = configuredEmbeddingModel?.status === 'ready'
    && configuredEmbeddingModel.integrityStatus === 'verified'
    && configuredEmbeddingModel.loadReady
    ? configuredEmbeddingModel
    : null
  const configuredBindingUnavailable = Boolean(base?.configuredEmbeddingModelId && !base.embeddingModel && !configuredEmbeddingModelReady)
  const effectiveEmbeddingModel = base?.embeddingModel
    ?? configuredEmbeddingModelReady
    ?? (!base?.configuredEmbeddingModelId ? defaultEmbeddingModel : null)

  const runningJob = jobs.find((job) => job.status === 'queued' || job.status === 'running' || job.status === 'paused') ?? null
  const searchHint = base && !modelsLoading && base.documentCount > 0 && !runningJob ? (() => {
    if (configuredBindingUnavailable) return { text: '绑定的搜索模型不可用，智能搜索暂时用不了。', label: '修复模型', run: () => onNavigate?.('models', base.id) }
    if (!base.vectorIndexReady) return {
      text: '智能搜索未开启，换个说法可能搜不到。',
      label: effectiveEmbeddingModel ? '开启' : '安装模型',
      run: effectiveEmbeddingModel ? () => void rebuildIndex() : () => onNavigate?.('models', base.id),
    }
    return null
  })() : null

  const rebuildIndex = async () => {
    if (!knowledgeBaseId) return
    setMutationError(null)
    try {
      const accepted = await gateway.startIndex(knowledgeBaseId, true)
      onOperationAccepted?.(accepted)
      navigateWithinSubview('jobs', knowledgeBaseId)
    } catch (cause) {
      setMutationError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  if (!knowledgeBaseId) return <div className={cn('fox-local-kb-page', className)}><div className="fox-local-kb-content"><LocalKnowledgeEmpty title="没有选择资料库" description="请从本机资料库列表进入一个资料库。" action={<Button onClick={onBack}>返回列表</Button>} /></div></div>


  return (
    <div className={cn('fox-local-kb-page fox-local-kb-detail-page', className)}>
      <div className="fox-local-kb-content fox-local-kb-detail-content" data-searching={searchShowing || undefined}>
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        {!loading && base && <>
          <header className="fox-kb-hub-head fox-library-detail-topbar">
            <Button aria-label={activeSubview ? '返回资料列表' : '返回资料库'} className="fox-local-kb-back" variant="ghost" size="icon" onClick={leaveSubviewOrDetail}><ArrowLeft /></Button>
            <nav className="fox-library-breadcrumb" aria-label="资料库路径">
              <button type="button" onClick={() => navigateLibraryFolder(base.id, '')} aria-current={!browser.path && !activeSubview ? 'page' : undefined} title={base.name}>{base.name}</button>
              {browser.path.split('/').filter(Boolean).map((name, index, parts) => <Fragment key={parts.slice(0, index + 1).join('/')}><span aria-hidden="true">/</span><button type="button" aria-current={index === parts.length - 1 && !activeSubview ? 'page' : undefined} onClick={() => navigateLibraryFolder(base.id, parts.slice(0, index + 1).join('/'))} title={name}>{name}</button></Fragment>)}
              {activeSubview && <><span aria-hidden="true">/</span><span aria-current="page">{activeSubview === 'jobs' ? '任务记录' : '检索测试'}</span></>}
            </nav>
            <div className="fox-kb-hub-actions">
              {activeSubview === null && <div className="fox-kb-search-control-slot" ref={setSearchContainer} />}
              <KnowledgeActivityClock jobs={jobs} knowledgeBaseName={base.name} onOpenJobs={() => { setActiveDocumentId(undefined); setActiveSubview('jobs') }} />
              <Button variant="ghost" size="icon" aria-label="资料库设置" onClick={() => setSettingsOpen(true)}><Settings2 /></Button>
              {activeSubview === null && <LibraryViewSwitch view={browser.view} onChange={(view) => setLibraryView(base.id, view)} />}
              {base.writable && <DropdownMenu>
                <DropdownMenuTrigger asChild><Button size="icon" aria-label="添加文件或文件夹" title="添加文件或文件夹"><Plus /></Button></DropdownMenuTrigger>
                <DropdownMenuContent align="end" sideOffset={6} className="fox-library-add-menu">
                  <DropdownMenuItem onSelect={() => { setActiveSubview(null); setActiveDocumentId(undefined); setAddRequest('file') }}><Upload />添加文件</DropdownMenuItem>
                  <DropdownMenuItem onSelect={() => { setActiveSubview(null); setActiveDocumentId(undefined); setAddRequest('folder') }}><FolderPlus />添加文件夹</DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>}
            </div>
          </header>
          {settingsOpen && <LocalKnowledgeSettingsDialog gateway={gateway} knowledgeBaseId={base.id} onBack={() => setSettingsOpen(false)} onNavigate={onNavigate} onSaved={(updated) => setBase((current) => current ? { ...current, ...updated } : current)} />}
          {mutationError && <p className="fox-local-kb-create-error" role="alert"><AlertCircle />{mutationError}</p>}
          {activeSubview === 'retrieval' && <LocalKnowledgeRetrievalPage embedded gateway={gateway} knowledgeBaseId={base.id} onBack={() => setActiveSubview(null)} onNavigate={navigateWithinSubview} />}
          {activeSubview === 'jobs' && <LocalKnowledgeJobsPage embedded gateway={gateway} knowledgeBaseId={base.id} onBack={() => setActiveSubview(null)} onNavigate={navigateWithinSubview} />}
          {activeSubview === null && <>
            <KnowledgeBaseSearch key={`${base.id}:${browser.navigation}`} gateway={gateway} base={base} hint={searchShowing ? searchHint : null} omni controlContainer={searchContainer} onActiveChange={setSearchShowing} onOpenAdvanced={() => setActiveSubview('retrieval')} onOpenDocument={(documentId) => setActiveDocumentId(documentId)} />
            <LocalKnowledgeDocumentsPanel base={base} addRequest={addRequest} onAddRequestHandled={clearAddRequest} gateway={gateway} knowledgeBaseId={base.id} documentId={activeDocumentId} onNavigate={navigateWithinSubview} onOperationAccepted={onOperationAccepted} onAskDocument={onAskDocument} />
          </>}
        </>}
      </div>
    </div>
  )
}

// Mirrors local_knowledge_picker.rs. Final file/path checks still run in Host.
const LOCAL_IMPORT_ACCEPT = '.pdf,.docx,.pptx,.xlsx,.md,.txt'
const LOCAL_IMPORT_MAX_BYTES = 256 * 1024 * 1024
const LOCAL_IMPORT_MAX_FILES = 5000

type KnowledgeImportSelection = FileDropzoneItem & { importFile: LocalKnowledgeImportFile }

function KnowledgeImportDropzone({ gateway, items, disabled, onItemsChange, onError, allowFolder = true }: {
  gateway: LocalKnowledgeGateway
  items: KnowledgeImportSelection[]
  disabled: boolean
  onItemsChange: (items: KnowledgeImportSelection[]) => void
  onError: (message: string | null) => void
  allowFolder?: boolean
}) {
  const [picking, setPicking] = useState(false)
  const [nativeDragging, setNativeDragging] = useState(false)
  const rootRef = useRef<HTMLDivElement>(null)
  const latest = useRef({ items, disabled, picking, onItemsChange, onError })
  latest.current = { items, disabled, picking, onItemsChange, onError }
  const alive = useRef(true)
  useEffect(() => { alive.current = true; return () => { alive.current = false } }, [])

  const addSelection = (incoming: Array<{ file: LocalKnowledgeImportFile; sizeKnown?: boolean }>) => {
    const current = latest.current
    if (current.disabled || current.picking) return
    const seen = new Set(current.items.map((item) => item.id))
    const added: KnowledgeImportSelection[] = []
    for (const { file, sizeKnown = true } of incoming) {
      const id = file.sourcePath ?? `${file.relativePath ?? file.name}:${file.sizeBytes}`
      if (seen.has(id)) continue
      seen.add(id)
      if (current.items.length + added.length >= LOCAL_IMPORT_MAX_FILES) {
        current.onError(`单次最多导入 ${LOCAL_IMPORT_MAX_FILES} 个文件。`)
        break
      }
      const extension = file.name.toLowerCase().match(/\.[^.]+$/)?.[0]
      const error = !extension || !LOCAL_IMPORT_ACCEPT.split(',').includes(extension)
        ? '不支持此格式；文件夹请使用「选择文件夹」。'
        : file.sizeBytes > LOCAL_IMPORT_MAX_BYTES ? '文件超过 256 MB 限制。'
          : !file.sourcePath && gateway.pickImportFiles ? '无法读取本机路径，请使用系统文件选择器重新选择。' : undefined
      added.push({ id, name: file.relativePath ?? file.name, size: file.sizeBytes, sizeKnown, status: error ? 'failed' : undefined, error, retryable: false, importFile: file })
    }
    current.onItemsChange([...current.items, ...added])
  }
  const addBrowserFiles = (files: File[]) => {
    onError(null)
    addSelection(files.map((file) => ({ file: { name: file.name, sourcePath: (file as File & { path?: string }).path, relativePath: file.webkitRelativePath || file.name, mimeType: file.type, sizeBytes: file.size } })))
  }
  const pick = async (kind: 'files' | 'folder') => {
    const select = kind === 'files' ? gateway.pickImportFiles : gateway.pickImportFolder
    if (!select || latest.current.disabled || latest.current.picking) return
    latest.current.picking = true
    setPicking(true)
    onError(null)
    try {
      const selected = await select.call(gateway)
      if (!alive.current) return
      latest.current.picking = false
      addSelection(selected.map((file) => ({ file })))
      if (kind === 'folder' && !selected.length) onError('所选文件夹中没有支持导入的文件。')
    } catch (cause) {
      if (alive.current) onError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      if (alive.current) { latest.current.picking = false; setPicking(false) }
    }
  }
  const addSelectionRef = useRef(addSelection)
  addSelectionRef.current = addSelection
  useEffect(() => {
    if (!desktopRuntimeAvailable) return
    let disposed = false
    let unlisten: (() => void) | undefined
    void import('@tauri-apps/api/webview').then(({ getCurrentWebview }) => getCurrentWebview().onDragDropEvent((event) => {
      if (disposed) return
      if (latest.current.disabled || latest.current.picking) { setNativeDragging(false); return }
      const payload = event.payload
      if (payload.type === 'leave') { setNativeDragging(false); return }
      const bounds = rootRef.current?.getBoundingClientRect()
      const scale = window.devicePixelRatio || 1
      const x = payload.position.x / scale, y = payload.position.y / scale
      const inside = Boolean(bounds && x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom)
      setNativeDragging(payload.type !== 'drop' && inside)
      if (payload.type !== 'drop' || !inside) return
      latest.current.onError(null)
      addSelectionRef.current(payload.paths.map((path) => ({ sizeKnown: false, file: { sourcePath: path, name: path.split(/[\\/]/).pop() ?? path, relativePath: path.split(/[\\/]/).pop() ?? path, mimeType: '', sizeBytes: 0 } })))
    })).then((stop) => { if (disposed) stop(); else unlisten = stop }).catch((cause) => {
      if (!disposed) latest.current.onError(`拖放暂不可用，请选择文件：${cause instanceof Error ? cause.message : String(cause)}`)
    })
    return () => { disposed = true; unlisten?.() }
  }, [])

  return <div ref={rootRef} className="fox-kb-file-dropzone">
    <FileDropzone items={items} onItemsChange={(next) => onItemsChange(next as KnowledgeImportSelection[])} onAddFiles={addBrowserFiles} onPickFiles={gateway.pickImportFiles ? () => void pick('files') : undefined} disabled={disabled || picking} externalDragging={nativeDragging} accept={LOCAL_IMPORT_ACCEPT} multiple maxSize={LOCAL_IMPORT_MAX_BYTES} maxFiles={LOCAL_IMPORT_MAX_FILES} compactAt={3} label={picking ? '正在选择文件…' : '拖入文件，或点击选择'} description="支持 PDF、DOCX、PPTX、XLSX、Markdown、TXT" note="每个文件不超过 256 MB；单次最多 5000 个文件。" dropLabel="松开以添加文件" />
    {allowFolder && gateway.pickImportFolder && <Button className="fox-kb-dropzone-folder" type="button" variant="outline" size="sm" disabled={disabled || picking} onClick={() => void pick('folder')}><FolderOpen />选择文件夹并保留目录结构</Button>}
    {items.length > 0 && <div className="fox-kb-dropzone-summary"><span>{items.filter((item) => item.status !== 'failed').length} 个待导入文件{items.some((item) => item.status === 'failed') && ` · ${items.filter((item) => item.status === 'failed').length} 个需移除或重新选择`}</span><Button type="button" variant="ghost" size="sm" disabled={disabled || picking} onClick={() => { onItemsChange([]); onError(null) }}>清空</Button></div>}
  </div>
}

function LocalKnowledgeDocumentsPanel({ base, gateway = defaultLocalKnowledgeGateway, className, knowledgeBaseId, documentId, onNavigate, addRequest, onAddRequestHandled, onOperationAccepted, onAskDocument }: LocalKnowledgeNavigationProps & { base: LocalKnowledgeBaseDetail; addRequest: 'file' | 'folder' | null; onAddRequestHandled: () => void; onOperationAccepted?: (accepted: OperationAccepted) => void }) {
  const [documents, setDocuments] = useState<LocalKnowledgeDocument[]>([])
  const [folders, setFolders] = useState<LocalKnowledgeFolder[]>([])
  const browser = useLibraryBrowser(knowledgeBaseId)
  const currentFolder = browser.path
  const setCurrentFolder = (path: string) => { if (knowledgeBaseId) navigateLibraryFolder(knowledgeBaseId, path) }
  const [createOpen, setCreateOpen] = useState(false)
  const [folderName, setFolderName] = useState('')
  const [creating, setCreating] = useState(false)
  const [folderError, setFolderError] = useState<string | null>(null)
  const [importOpen, setImportOpen] = useState(false)
  const [importSelections, setImportSelections] = useState<KnowledgeImportSelection[]>([])
  const importFiles = importSelections.filter((item) => item.status !== 'failed').map((item) => item.importFile)
  const [targetFolder, setTargetFolder] = useState('')
  const [submitting, setSubmitting] = useState(false)
  const [importError, setImportError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [removeFiles, setRemoveFiles] = useState<LibraryFile[]>([])
  const [removing, setRemoving] = useState(false)
  const [actionError, setActionError] = useState<string | null>(null)
  const previewReturnFocus = useRef<HTMLElement | null>(null)

  const load = async () => {
    if (!knowledgeBaseId) return
    setLoading(true)
    try {
      const [page, folderItems] = await Promise.all([gateway.listDocuments(knowledgeBaseId), gateway.listFolders(knowledgeBaseId)])
      setDocuments(page.items); setFolders(folderItems); setError(null)
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }

  useEffect(() => { void load() }, [gateway, knowledgeBaseId])
  useEffect(() => {
    if (!knowledgeBaseId || !documents.some((document) => document.status === 'queued' || document.status === 'parsing')) return
    let disposed = false
    const timer = window.setInterval(() => { void gateway.listDocuments(knowledgeBaseId).then((page) => { if (!disposed) setDocuments(page.items) }).catch(() => {}) }, 1800)
    return () => { disposed = true; window.clearInterval(timer) }
  }, [gateway, knowledgeBaseId, documents.some((document) => document.status === 'queued' || document.status === 'parsing')])
  useEffect(() => {
    if (!addRequest || !knowledgeBaseId) return
    if (addRequest === 'folder') {
      setFolderError(null)
      setCreateOpen(true)
    } else {
      setTargetFolder(currentFolder)
      setImportSelections([])
      setImportError(null)
      setImportOpen(true)
    }
    onAddRequestHandled()
  }, [addRequest, knowledgeBaseId, currentFolder, onAddRequestHandled])
  const selectedDocument = documentId ? documents.find((document) => document.id === documentId) ?? null : null

  useEffect(() => {
    if (base && selectedDocument) rememberRecentDocument(base, selectedDocument)
  }, [base, selectedDocument])

  const managedFiles = useMemo<LibraryFile[]>(() => [
    ...folders.map((folder) => ({ id: `folder:${folder.relativePath}`, name: folder.name, path: folder.relativePath, folder: true, size: 0 })),
    ...documents.map((document) => ({ id: document.id, name: document.name, path: document.relativePath, size: document.sizeBytes, modifiedAt: document.updatedAt, status: document.status })),
  ], [folders, documents])
  useEffect(() => { if (knowledgeBaseId && base?.id === knowledgeBaseId && !loading && !error) publishLibraryFolders(knowledgeBaseId, managedFiles) }, [knowledgeBaseId, base?.id, loading, error, managedFiles])

  const revealDocument = async (document: LocalKnowledgeDocument) => {
    if (!knowledgeBaseId) return
    try { await gateway.revealDocumentFile(knowledgeBaseId, document.id) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }

  const removeDocuments = async () => {
    if (!knowledgeBaseId || removing) return
    setRemoving(true)
    setActionError(null)
    const failures: string[] = []
    try {
      for (const file of removeFiles) {
        try {
          const removed = await gateway.deleteDocument(knowledgeBaseId, file.id)
          if (!removed) throw new Error('服务未确认移除')
          recordLocalKnowledgeActivity({ knowledgeBaseId, kind: 'document_deleted', title: '移除资料', detail: file.path || file.name })
        } catch (cause) { failures.push(`${file.name}：${cause instanceof Error ? cause.message : String(cause)}`) }
      }
      setRemoveFiles([])
      await load()
      if (failures.length) setActionError(`部分文件未移除：${failures.join('；')}`)
      else toast.success('已从资料库移除，电脑上的原文件已保留。')
    } finally { setRemoving(false) }
  }

  const reprocessDocuments = async (files: LibraryFile[]) => {
    if (!knowledgeBaseId || submitting) return
    setSubmitting(true)
    setActionError(null)
    const failures: string[] = []
    let acceptedCount = 0
    try {
      for (const file of files) {
        try { const accepted = await gateway.reparseDocument(knowledgeBaseId, file.id); onOperationAccepted?.(accepted); acceptedCount += 1 }
        catch (cause) { failures.push(`${file.name}：${cause instanceof Error ? cause.message : String(cause)}`) }
      }
      if (acceptedCount) toast.success(`${acceptedCount} 个文件已加入后台处理队列。`)
      if (failures.length) setActionError(`部分文件未开始处理：${failures.join('；')}`)
      await load()
    } finally { setSubmitting(false) }
  }

  const createFolder = async () => {
    if (!knowledgeBaseId || !folderName.trim()) return
    setCreating(true)
    setFolderError(null)
    try {
      await gateway.createFolder(knowledgeBaseId, folderName, currentFolder)
      recordLocalKnowledgeActivity({ knowledgeBaseId, kind: 'folder_created', title: '创建文件夹', detail: [currentFolder, folderName.trim()].filter(Boolean).join('/') })
      setFolderName('')
      setCreateOpen(false)
      await load()
    } catch (cause) {
      setFolderError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setCreating(false)
    }
  }

  const openImportDialog = (path = currentFolder) => {
    setTargetFolder(path)
    setImportSelections([])
    setImportError(null)
    setImportOpen(true)
  }

  const closeImportDialog = () => {
    if (submitting) return
    setImportOpen(false)
    setImportSelections([])
    setImportError(null)
  }

  const startImportFromDialog = async () => {
    if (!knowledgeBaseId || importFiles.length === 0 || submitting || importSelections.some((item) => item.status === 'failed')) return
    const duplicates = duplicateLibraryImportPaths(importFiles)
    if (duplicates.length) { setImportError(`有同名文件会导入到同一位置：${duplicates.join('、')}。请移除重复项后再导入。`); return }
    setSubmitting(true)
    setImportError(null)
    try {
      const accepted = await gateway.startImport({
        knowledgeBaseId,
        files: importFiles.map((file) => ({
          ...file,
          relativePath: [targetFolder, file.relativePath ?? file.name].filter(Boolean).join('/'),
        })),
      })
      recordImportedDocuments(knowledgeBaseId, importFiles.map((file) => ({ name: file.name, relativePath: [targetFolder, file.relativePath ?? file.name].filter(Boolean).join('/') })))
      onOperationAccepted?.(accepted)
      setImportOpen(false)
      setImportSelections([])
      toast.success('文件已加入后台导入队列。')
      await load()
    } catch (cause) {
      setImportError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSubmitting(false)
    }
  }



  return (
    <div className={cn('fox-local-kb-page', 'fox-local-kb-embedded-page', className)}>
      <div className={cn('fox-local-kb-content fox-local-kb-reader-page', 'fox-local-kb-embedded-content')}>
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        {!loading && !error && <section className="fox-local-kb-browser">
          {actionError && <p className="fox-local-kb-form-error" role="alert"><AlertCircle />{actionError}</p>}
          <LibraryFileManager contentOnly view={browser.view} files={managedFiles} path={currentFolder} onPathChange={setCurrentFolder} busy={removing || submitting} onOpen={(file) => onNavigate?.('documents', knowledgeBaseId, file.id)}
            onRemove={base?.writable ? setRemoveFiles : undefined}
            onReveal={(file) => { const document = documents.find((item) => item.id === file.id); if (document) void revealDocument(document) }}
            onReprocess={base?.writable ? (files) => void reprocessDocuments(files) : undefined}
            onAdd={base?.writable ? openImportDialog : undefined}
            onCreateFolder={base?.writable ? (path) => { setCurrentFolder(path); setFolderError(null); setCreateOpen(true) } : undefined}
            onDropFiles={base?.writable ? (files, path) => {
              setTargetFolder(path); setImportError(null); setImportOpen(true)
              setImportSelections(files.map((file) => {
                const sourcePath = (file as File & { path?: string }).path
                const invalid = !LOCAL_IMPORT_ACCEPT.split(',').includes(`.${file.name.split('.').pop()?.toLowerCase()}`) ? '不支持此格式' : file.size > LOCAL_IMPORT_MAX_BYTES ? '文件超过 256 MB' : !sourcePath && gateway.pickImportFiles ? '无法读取本机路径，请用系统文件选择器重新选择。' : undefined
                return { id: sourcePath ?? file.name, name: file.name, size: file.size, status: invalid ? 'failed' : undefined, error: invalid, importFile: { name: file.name, sourcePath, mimeType: file.type, sizeBytes: file.size } }
              }))
            } : undefined}
            onDropPaths={base?.writable ? (paths, path) => {
              setTargetFolder(path); setImportError(null); setImportOpen(true)
              setImportSelections(paths.slice(0, LOCAL_IMPORT_MAX_FILES).map((sourcePath) => {
                const name = sourcePath.split(/[\\/]/).pop() ?? sourcePath
                const invalid = !LOCAL_IMPORT_ACCEPT.split(',').includes(`.${name.split('.').pop()?.toLowerCase()}`) ? '不支持此格式，请选择文件。' : undefined
                return { id: sourcePath, name, size: 0, sizeKnown: false, status: invalid ? 'failed' : undefined, error: invalid, importFile: { name, sourcePath, mimeType: '', sizeBytes: 0 } }
              }))
            } : undefined} />
        </section>}
        <Dialog open={Boolean(selectedDocument)} onOpenChange={(open) => { if (!open) onNavigate?.('documents', knowledgeBaseId) }}><DialogContent className="fox-library-preview-dialog" aria-describedby={undefined} onOpenAutoFocus={() => { previewReturnFocus.current = globalThis.document.activeElement as HTMLElement }} onCloseAutoFocus={(event) => { event.preventDefault(); if (previewReturnFocus.current?.isConnected) previewReturnFocus.current.focus() }}>
          <DialogTitle className="sr-only">预览 {selectedDocument?.name}</DialogTitle>
          {!loading && !error && selectedDocument && knowledgeBaseId && <LocalKnowledgeDocumentReader key={selectedDocument.id} knowledgeBaseName={base?.name} knowledgeBaseId={knowledgeBaseId} document={selectedDocument} gateway={gateway} onBack={() => onNavigate?.('documents', knowledgeBaseId)} onDeleted={async () => { await load(); onNavigate?.('documents', knowledgeBaseId) }} onJobAccepted={() => onNavigate?.('jobs', knowledgeBaseId)} onAskDocument={onAskDocument} />}
        </DialogContent></Dialog>
      </div>
      <Dialog open={removeFiles.length > 0} onOpenChange={(open) => { if (!open && !removing) setRemoveFiles([]) }}><DialogContent showCloseButton={!removing}><DialogHeader><DialogTitle>从资料库移除 {removeFiles.length} 个文件？</DialogTitle><DialogDescription>移除资料副本和对应搜索数据，电脑上的原文件会保留。已有引用中的资料内容将不再可用。</DialogDescription></DialogHeader><ul className="fox-library-remove-list">{removeFiles.map((file) => <li key={file.id}>{file.name}</li>)}</ul><DialogFooter><Button variant="outline" disabled={removing} onClick={() => setRemoveFiles([])}>取消</Button><Button disabled={removing} onClick={() => void removeDocuments()}>{removing && <LoaderCircle className="animate-spin" />}确认移除</Button></DialogFooter></DialogContent></Dialog>
      <Dialog open={createOpen} onOpenChange={(open) => { setCreateOpen(open); if (!open) { setFolderName(''); setFolderError(null) } }}>
        <DialogContent className="fox-local-kb-folder-dialog"><DialogHeader><DialogTitle>新建文件夹</DialogTitle><DialogDescription>{currentFolder ? `在「${currentFolder}」中创建子文件夹。` : '在资料库根目录创建文件夹。'}</DialogDescription></DialogHeader><label className="fox-local-kb-folder-field"><span>文件夹名称</span><Input autoFocus value={folderName} onChange={(event) => setFolderName(event.target.value)} placeholder="例如：产品资料" onKeyDown={(event) => { if (event.key === 'Enter') void createFolder() }} /></label>{folderError && <p className="fox-local-kb-form-error" role="alert"><AlertCircle />{folderError}</p>}<DialogFooter><Button variant="outline" onClick={() => setCreateOpen(false)} disabled={creating}>取消</Button><Button onClick={() => void createFolder()} disabled={creating || !folderName.trim()}>{creating ? <LoaderCircle className="animate-spin" /> : <FolderPlus />}{creating ? '正在创建' : '创建文件夹'}</Button></DialogFooter></DialogContent>
      </Dialog>
      <Dialog open={importOpen} onOpenChange={(open) => { if (open) setImportOpen(true); else closeImportDialog() }}>
        <DialogContent className="fox-local-kb-import-dialog" showCloseButton={!submitting}>
          <DialogHeader><DialogTitle>添加资料</DialogTitle><DialogDescription>{base ? `添加到「${base.name}」，Fox 只复制一份到资料库，电脑上的原文件不变。` : '选择要加入本机资料库的文件。'}</DialogDescription></DialogHeader>
          <div className="fox-local-kb-import-dialog-body">
            {importOpen && <KnowledgeImportDropzone gateway={gateway} items={importSelections} onItemsChange={setImportSelections} disabled={submitting} onError={setImportError} />}
            <label className="fox-local-kb-import-target"><span>导入到</span><Select value={targetFolder || '__root__'} onValueChange={(value) => setTargetFolder(value === '__root__' ? '' : value)} disabled={submitting}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="__root__">根目录</SelectItem>{folders.map((folder) => <SelectItem key={folder.relativePath} value={folder.relativePath}>{folder.relativePath}</SelectItem>)}</SelectContent></Select></label>

            {importError && <p className="fox-local-kb-form-error" role="alert"><AlertCircle />{importError}</p>}
          </div>
          <DialogFooter><Button variant="outline" onClick={closeImportDialog} disabled={submitting}>取消</Button><Button onClick={() => void startImportFromDialog()} disabled={!knowledgeBaseId || importFiles.length === 0 || submitting || importSelections.some((item) => item.status === 'failed')}>{submitting ? <LoaderCircle className="animate-spin" /> : <Upload />}{submitting ? '正在导入' : '开始导入'}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}

export function LocalKnowledgeImportPage({ gateway = defaultLocalKnowledgeGateway, className, knowledgeBaseId, onBack, onNavigate, onOperationAccepted }: LocalKnowledgeNavigationProps & { onOperationAccepted?: (accepted: OperationAccepted) => void }) {
  const [base, setBase] = useState<LocalKnowledgeBase | null>(null)
  const [folders, setFolders] = useState<LocalKnowledgeFolder[]>([])
  const [targetFolder, setTargetFolder] = useState('')
  const [files, setFiles] = useState<LocalKnowledgeImportFile[]>([])
  const [picking, setPicking] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (!knowledgeBaseId) return
    void Promise.all([gateway.getKnowledgeBase(knowledgeBaseId), gateway.listFolders(knowledgeBaseId)]).then(([nextBase, nextFolders]) => { setBase(nextBase); setFolders(nextFolders) }).catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
  }, [gateway, knowledgeBaseId])

  const chooseFiles = (event: ChangeEvent<HTMLInputElement>) => {
    const selected = Array.from(event.target.files ?? []).map((file) => ({
      name: file.name,
      sourcePath: (file as File & { path?: string }).path,
      relativePath: file.name,
      mimeType: file.type,
      sizeBytes: file.size,
    }))
    setFiles(selected)
    setError(null)
  }

  const chooseHostFolder = async () => {
    if (!gateway.pickImportFolder) return
    setPicking(true)
    try {
      const selected = await gateway.pickImportFolder()
      setFiles(selected)
      setError(selected.length ? null : '所选文件夹中没有支持导入的文件。')
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setPicking(false)
    }
  }

  const chooseHostFiles = async () => {
    if (!gateway.pickImportFiles) return
    setPicking(true)
    try {
      setFiles(await gateway.pickImportFiles())
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setPicking(false)
    }
  }

  const startImport = async () => {
    if (!knowledgeBaseId || files.length === 0) return
    setSubmitting(true)
    try {
      const accepted = await gateway.startImport({
        knowledgeBaseId,
        files: files.map((file) => ({
          ...file,
          relativePath: [targetFolder, file.relativePath ?? file.name].filter(Boolean).join('/'),
        })),
      })
      recordImportedDocuments(knowledgeBaseId, files.map((file) => ({ name: file.name, relativePath: [targetFolder, file.relativePath ?? file.name].filter(Boolean).join('/') })))
      onOperationAccepted?.(accepted)
      onNavigate?.('jobs', knowledgeBaseId)
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setSubmitting(false) }
  }

  return (
    <div className={cn('fox-local-kb-page', className)}>
      <div className="fox-local-kb-content fox-local-kb-import-page">
        <LocalKnowledgeHeader title="导入文件" description={base ? `导入到「${base.name}」，文件只会写入本机资料库。` : '选择要加入本机资料库的文件。'} onBack={onBack} />
        <Card className="fox-local-kb-import-card"><div className="fox-local-kb-import-dropzone"><Upload /><strong>选择要导入的文件或文件夹</strong><small>支持 PDF、DOCX、PPTX、XLSX、Markdown、TXT；导入文件夹时会保留原有目录结构。</small><div className="fox-local-kb-import-pickers">{gateway.pickImportFiles ? <Button type="button" variant="outline" onClick={() => void chooseHostFiles()} disabled={picking}>{picking ? <LoaderCircle className="animate-spin" /> : <FileText />}选择文件</Button> : <label className="fox-local-kb-file-picker"><span>选择文件</span><input type="file" multiple onChange={chooseFiles} /></label>}{gateway.pickImportFolder && <Button type="button" variant="outline" onClick={() => void chooseHostFolder()} disabled={picking}>{picking ? <LoaderCircle className="animate-spin" /> : <FolderOpen />}选择文件夹</Button>}</div></div><label className="fox-local-kb-import-target"><span>导入到</span><Select value={targetFolder || '__root__'} onValueChange={(value) => setTargetFolder(value === '__root__' ? '' : value)}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="__root__">根目录</SelectItem>{folders.map((folder) => <SelectItem key={folder.relativePath} value={folder.relativePath}>{folder.relativePath}</SelectItem>)}</SelectContent></Select></label>{files.length > 0 && <div className="fox-local-kb-selected-files"><div className="fox-local-kb-section-head"><div><h2>待导入文件</h2><p>{files.length} 个文件 · {formatBytes(files.reduce((total, file) => total + file.sizeBytes, 0))}</p></div><Button variant="ghost" size="sm" onClick={() => setFiles([])}>清空</Button></div>{files.map((file) => <div className="fox-local-kb-selected-file" key={`${file.name}-${file.sourcePath ?? file.sizeBytes}`}><FileText /><span title={file.relativePath ?? file.name}>{file.relativePath ?? file.name}</span><small>{formatBytes(file.sizeBytes)}</small></div>)}</div>}{error && <p className="fox-local-kb-form-error"><AlertCircle />{error}</p>}<div className="fox-local-kb-form-actions"><Button variant="outline" onClick={onBack}>取消</Button><Button onClick={() => void startImport()} disabled={!knowledgeBaseId || files.length === 0 || submitting}>{submitting ? <LoaderCircle className="animate-spin" /> : <Upload />}开始导入</Button></div></Card>
      </div>
    </div>
  )
}

function operationStateForJob(job: KnowledgeJob, state: OperationState | undefined): KnowledgeJob {
  if (!state || state.lastSequence <= job.lastSequence) return job
  return {
    ...job,
    status: state.status === 'idle' ? job.status : state.status,
    stage: state.stage ?? job.stage,
    progress: state.progress,
    lastSequence: state.lastSequence,
    outcome: state.outcome ?? job.outcome,
    completedItems: typeof state.data.completedItems === 'number' ? state.data.completedItems : job.completedItems,
    failedItems: typeof state.data.failedItems === 'number' ? state.data.failedItems : job.failedItems,
    totalItems: typeof state.data.totalItems === 'number' ? state.data.totalItems : job.totalItems,
    error: state.error ?? job.error,
    updatedAt: state.lastEvent?.occurredAt ?? job.updatedAt,
  }
}

type KnowledgeJobFilter = 'all' | 'active' | 'attention' | 'completed'
type KnowledgeJobTypeFilter = 'all' | KnowledgeJob['type']
type KnowledgeJobDateFilter = 'all' | 'today' | 'week'

type KnowledgeJobStageStep = { label: string; stages: KnowledgeOperationStage[] }

const KNOWLEDGE_JOB_STAGE_STEPS: Record<KnowledgeJob['type'], KnowledgeJobStageStep[]> = {
  import: [
    { label: '校验', stages: ['validating'] },
    { label: '复制', stages: ['copying', 'hashing'] },
    { label: '读取', stages: ['parsing'] },
    { label: '保存', stages: ['committing'] },
  ],
  parse: [
    { label: '校验', stages: ['validating'] },
    { label: '读取', stages: ['parsing'] },
    { label: '切段', stages: ['chunking'] },
    { label: '保存', stages: ['committing'] },
  ],
  index: [
    { label: '校验', stages: ['validating'] },
    { label: '整理', stages: ['embedding'] },
    { label: '写入搜索结果', stages: ['vector_upsert'] },
    { label: '检查结果', stages: ['verifying'] },
    { label: '保存', stages: ['committing'] },
  ],
  rebuild: [
    { label: '校验', stages: ['validating'] },
    { label: '整理', stages: ['embedding'] },
    { label: '写入搜索结果', stages: ['vector_upsert'] },
    { label: '检查结果', stages: ['verifying'] },
    { label: '保存', stages: ['committing'] },
  ],
  delete: [
    { label: '校验', stages: ['validating'] },
    { label: '确认移除', stages: ['committing'] },
  ],
}

function knowledgeJobBucket(job: KnowledgeJob): Exclude<KnowledgeJobFilter, 'all'> {
  if (job.status === 'queued' || job.status === 'running' || job.status === 'paused') return 'active'
  if (job.status === 'failed' || job.status === 'interrupted' || (job.status === 'completed' && job.outcome === 'partial')) return 'attention'
  return 'completed'
}

function knowledgeJobTypeLabel(type: KnowledgeJob['type']): string {
  return ({ import: '添加资料', parse: '读取资料', index: '整理', delete: '移除资料', rebuild: '重新整理' })[type]
}

function knowledgeJobTitle(job: KnowledgeJob, knowledgeBaseName: string): string {
  const count = job.totalItems > 0 ? job.totalItems : Math.max(job.completedItems + job.failedItems, 1)
  if (job.type === 'import') return `添加 ${count} 份资料到 ${knowledgeBaseName}`
  if (job.type === 'parse') return `读取 ${count} 份资料`
  if (job.type === 'delete') return `移除 ${count} 份资料`
  return `${knowledgeJobTypeLabel(job.type)} ${count} 份资料`
}

/** History lines put the knowledge base in its own column, so the title only
 *  has to say what finished. */
function knowledgeJobHistoryTitle(job: KnowledgeJob): string {
  const count = job.totalItems > 0 ? job.totalItems : Math.max(job.completedItems + job.failedItems, 1)
  if (job.type === 'import') return `添加了 ${count} 份资料`
  if (job.type === 'parse') return `读取了 ${count} 份资料`
  if (job.type === 'delete') return `移除了 ${count} 份资料`
  return job.type === 'rebuild' ? '重新整理完成' : '整理完成'
}

function knowledgeJobStageIndex(type: KnowledgeJob['type'], stage: KnowledgeOperationStage | null): number {
  if (!stage) return 0
  const index = KNOWLEDGE_JOB_STAGE_STEPS[type].findIndex((item) => item.stages.includes(stage))
  return Math.max(index, 0)
}

function knowledgeJobDateKey(value: string): string {
  const date = new Date(value)
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}-${String(date.getDate()).padStart(2, '0')}`
}

function knowledgeJobDateGroup(value: string): { label: string; date: string } {
  const date = new Date(value)
  const today = new Date()
  const startOfToday = new Date(today.getFullYear(), today.getMonth(), today.getDate())
  const startOfDate = new Date(date.getFullYear(), date.getMonth(), date.getDate())
  const dayDelta = Math.round((startOfToday.getTime() - startOfDate.getTime()) / 86_400_000)
  return {
    label: dayDelta === 0 ? '今天' : dayDelta === 1 ? '昨天' : `${date.getMonth() + 1}月${date.getDate()}日`,
    date: knowledgeJobDateKey(value),
  }
}

function knowledgeJobInDateRange(job: KnowledgeJob, filter: KnowledgeJobDateFilter): boolean {
  if (filter === 'all') return true
  const updatedAt = new Date(job.updatedAt).getTime()
  const now = new Date()
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  if (filter === 'today') return updatedAt >= startOfToday
  return updatedAt >= startOfToday - 6 * 86_400_000
}

export function LocalKnowledgeJobsPage({ gateway = defaultLocalKnowledgeGateway, className, knowledgeBaseId, onBack, onNavigate, embedded = false }: EmbeddedLocalKnowledgeNavigationProps) {
  const [jobs, setJobs] = useState<KnowledgeJob[]>([])
  const [knowledgeBases, setKnowledgeBases] = useState<LocalKnowledgeBase[]>([])
  const [states, setStates] = useState<Record<string, OperationState>>({})
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [statusFilter, setStatusFilter] = useState<KnowledgeJobFilter>('all')
  const [typeFilter, setTypeFilter] = useState<KnowledgeJobTypeFilter>('all')
  const [baseFilter, setBaseFilter] = useState(knowledgeBaseId ?? 'all')
  const [dateFilter, setDateFilter] = useState<KnowledgeJobDateFilter>('all')
  const [expandedJobIds, setExpandedJobIds] = useState<Set<string>>(() => new Set())

  const [visibleJobCount, setVisibleJobCount] = useState(KNOWLEDGE_JOB_PAGE_SIZE)
  const loadMoreJobsRef = useRef<HTMLDivElement | null>(null)

  const load = async () => {
    setLoading(true)
    try {
      const [nextJobs, nextKnowledgeBases] = await Promise.all([
        gateway.listJobs(knowledgeBaseId),
        gateway.listKnowledgeBases().catch(() => [] as LocalKnowledgeBase[]),
      ])
      setJobs(nextJobs)
      setKnowledgeBases(nextKnowledgeBases)
      // 原型 08-jobs：进行中的任务默认也只是一行 + 3px 进度条，
      // 阶段清单要点开才展开，所以这里只保留用户已展开的项。
      setExpandedJobIds((current) => new Set(Array.from(current).filter((jobId) => nextJobs.some((job) => job.id === jobId))))
      setError(null)
    }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }

  useEffect(() => { void load() }, [gateway, knowledgeBaseId])
  useEffect(() => { setBaseFilter(knowledgeBaseId ?? 'all') }, [knowledgeBaseId])

  useEffect(() => {
    let disposed = false
    const unlisten: Array<() => void> = []
    const activeJobs = jobs.filter((job) => job.status === 'queued' || job.status === 'running' || job.status === 'paused')
    for (const job of activeJobs) {
      void gateway.getOperation(job.operationId).then((snapshot) => {
        if (disposed) return
        const initial = createOperationState(job.operationId)
        const state = snapshot.lastEvent ? reduceOperationEvent(initial, snapshot.lastEvent) : initial
        mergeKnowledgeJobStageActivities(job.knowledgeBaseId, [operationStateForJob(job, state)])
        setStates((current) => ({ ...current, [job.operationId]: state }))
      }).catch(() => undefined)
      unlisten.push(gateway.subscribeOperation(job.operationId, (event) => {
        if (disposed) return
        const observedState = reduceOperationEvent(createOperationState(job.operationId), event)
        mergeKnowledgeJobStageActivities(job.knowledgeBaseId, [operationStateForJob(job, observedState)])
        setStates((current) => {
          const previous = current[job.operationId] ?? createOperationState(job.operationId)
          return { ...current, [job.operationId]: reduceOperationEvent(previous, event) }
        })
      }))
    }
    return () => { disposed = true; for (const stop of unlisten) stop() }
  }, [gateway, jobs])

  const visibleJobs = useMemo(() => jobs.map((job) => operationStateForJob(job, states[job.operationId])), [jobs, states])
  const baseNameById = useMemo(() => new Map(knowledgeBases.map((base) => [base.id, base.name])), [knowledgeBases])
  const scopedJobs = useMemo(() => visibleJobs
    .filter((job) => baseFilter === 'all' || job.knowledgeBaseId === baseFilter)
    .filter((job) => typeFilter === 'all' || job.type === typeFilter)
    .filter((job) => knowledgeJobInDateRange(job, dateFilter))
    .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt)), [baseFilter, dateFilter, typeFilter, visibleJobs])
  const filteredJobs = useMemo(() => scopedJobs.filter((job) => statusFilter === 'all' || knowledgeJobBucket(job) === statusFilter), [scopedJobs, statusFilter])
  const displayedJobs = useMemo(() => filteredJobs.slice(0, visibleJobCount), [filteredJobs, visibleJobCount])
  const hasMoreJobs = visibleJobCount < filteredJobs.length
  const pinnedJobs = useMemo(() => displayedJobs.filter((job) => knowledgeJobBucket(job) !== 'completed'), [displayedJobs])
  const historyJobs = useMemo(() => displayedJobs.filter((job) => knowledgeJobBucket(job) === 'completed'), [displayedJobs])
  const groupedJobs = useMemo(() => {
    const groups = new Map<string, { label: string; date: string; jobs: KnowledgeJob[] }>()
    for (const job of historyJobs) {
      const dateGroup = knowledgeJobDateGroup(job.updatedAt)
      const group = groups.get(dateGroup.date) ?? { ...dateGroup, jobs: [] }
      group.jobs.push(job)
      groups.set(dateGroup.date, group)
    }
    return Array.from(groups.values())
  }, [historyJobs])
  useEffect(() => {
    setVisibleJobCount(KNOWLEDGE_JOB_PAGE_SIZE)
  }, [baseFilter, dateFilter, statusFilter, typeFilter])

  useEffect(() => {
    const sentinel = loadMoreJobsRef.current
    if (!sentinel || !hasMoreJobs) return
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) {
        setVisibleJobCount((current) => Math.min(current + KNOWLEDGE_JOB_PAGE_SIZE, filteredJobs.length))
      }
    }, { rootMargin: '120px 0px' })
    observer.observe(sentinel)
    return () => observer.disconnect()
  }, [filteredJobs.length, hasMoreJobs])

  const cancel = async (job: KnowledgeJob) => {
    try { const updated = await gateway.cancelJob(job.id); setJobs((current) => current.map((item) => item.id === updated.id ? updated : item)) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }

  const retry = async (job: KnowledgeJob) => {
    try { await gateway.retryJob(job.id); await load() }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }
  const activeFilterCount = (knowledgeBaseId ? 0 : baseFilter !== 'all' ? 1 : 0) + (typeFilter !== 'all' ? 1 : 0) + (dateFilter !== 'all' ? 1 : 0)
  const bucketCount = (bucket: Exclude<KnowledgeJobFilter, 'all'>) => scopedJobs.filter((job) => knowledgeJobBucket(job) === bucket).length
  const jobFilters = <div className="fox-local-kb-job-toolbar">
    <Tabs value={statusFilter} onValueChange={(value) => setStatusFilter(value as KnowledgeJobFilter)} className="fox-local-kb-job-filter-tabs">
      <TabsList aria-label="筛选任务状态"><TabsTrigger value="all">全部</TabsTrigger><TabsTrigger value="active">进行中<small>{bucketCount('active')}</small></TabsTrigger><TabsTrigger value="attention">需处理<small>{bucketCount('attention')}</small></TabsTrigger><TabsTrigger value="completed">已完成<small>{bucketCount('completed')}</small></TabsTrigger></TabsList>
    </Tabs>
    <Popover>
      <PopoverTrigger asChild><Button variant={activeFilterCount > 0 ? 'outline' : 'ghost'} size="icon" aria-label={activeFilterCount > 0 ? `筛选任务，已应用 ${activeFilterCount} 个条件` : '筛选任务'}><ListFilter /></Button></PopoverTrigger>
      <PopoverContent align="end" sideOffset={6} className="fox-local-kb-job-filter-pop">
        {!knowledgeBaseId && <label className="fox-local-kb-job-filter-field"><span>资料库</span><Select value={baseFilter} onValueChange={setBaseFilter}><SelectTrigger aria-label="筛选资料库"><SelectValue placeholder="全部资料库" /></SelectTrigger><SelectContent><SelectItem value="all">全部资料库</SelectItem>{knowledgeBases.map((base) => <SelectItem key={base.id} value={base.id}>{base.name}</SelectItem>)}</SelectContent></Select></label>}
        <label className="fox-local-kb-job-filter-field"><span>类型</span><Select value={typeFilter} onValueChange={(value) => setTypeFilter(value as KnowledgeJobTypeFilter)}><SelectTrigger aria-label="筛选任务类型"><SelectValue placeholder="全部类型" /></SelectTrigger><SelectContent><SelectItem value="all">全部类型</SelectItem>{(['import', 'parse', 'index', 'delete', 'rebuild'] as const).map((type) => <SelectItem key={type} value={type}>{knowledgeJobTypeLabel(type)}</SelectItem>)}</SelectContent></Select></label>
        <label className="fox-local-kb-job-filter-field"><span>时间</span><Select value={dateFilter} onValueChange={(value) => setDateFilter(value as KnowledgeJobDateFilter)}><SelectTrigger aria-label="筛选任务时间"><SelectValue placeholder="全部时间" /></SelectTrigger><SelectContent><SelectItem value="all">全部时间</SelectItem><SelectItem value="today">今天</SelectItem><SelectItem value="week">近 7 天</SelectItem></SelectContent></Select></label>
      </PopoverContent>
    </Popover>
  </div>
  return (
    <div className={cn('fox-local-kb-page', embedded && 'fox-local-kb-embedded-page', className)}>
      <div className={cn('fox-local-kb-content fox-local-kb-jobs-content', embedded && 'fox-local-kb-embedded-content')}>
        {!embedded && <LocalKnowledgeHeader title={knowledgeBaseId ? '任务状态' : '处理进度'} description="" onBack={knowledgeBaseId ? onBack : undefined} actions={jobFilters} />}
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        {!loading && !error && visibleJobs.length === 0 && <section className="fox-local-kb-job-board"><LocalKnowledgeEmpty title="没有任务记录" description="添加资料或重新整理后，进度会显示在这里。" action={knowledgeBaseId ? <Button onClick={() => onNavigate?.('documents', knowledgeBaseId)}>前往文档</Button> : <Button onClick={() => onNavigate?.('list')}>查看资料库</Button>} /></section>}
        {!loading && !error && visibleJobs.length > 0 && <section className="fox-local-kb-job-board">
          {embedded && jobFilters}
          {pinnedJobs.length > 0 && <div className="fox-kb-job-pinned">
            {pinnedJobs.map((job) => <JobTimelineItem key={job.id} job={job} knowledgeBaseName={baseNameById.get(job.knowledgeBaseId) ?? job.knowledgeBaseId} expanded={expandedJobIds.has(job.id)} onToggle={() => setExpandedJobIds((current) => { const next = new Set(current); if (next.has(job.id)) next.delete(job.id); else next.add(job.id); return next })} onCancel={() => void cancel(job)} onRetry={() => void retry(job)} />)}
          </div>}
          <div className="fox-local-kb-job-timeline" aria-live="polite">
            {displayedJobs.length === 0
              ? <LocalKnowledgeEmpty title="没找到任务，试着调整筛选条件" description="调整状态、资料库、任务类型或时间范围后再试。" />
              : <>{groupedJobs.map((group) => {
                return <section className="fox-local-kb-job-day" key={group.date}>
                  <header>
                    <strong>{group.label}</strong>
                  </header>
                  <div className="fox-local-kb-job-day-list">{group.jobs.map((job) => <JobTimelineItem key={job.id} job={job} knowledgeBaseName={baseNameById.get(job.knowledgeBaseId) ?? job.knowledgeBaseId} expanded={expandedJobIds.has(job.id)} onToggle={() => setExpandedJobIds((current) => { const next = new Set(current); if (next.has(job.id)) next.delete(job.id); else next.add(job.id); return next })} onCancel={() => void cancel(job)} onRetry={() => void retry(job)} />)}</div>
                </section>
              })}{hasMoreJobs && <div ref={loadMoreJobsRef} className="fox-local-kb-jobs-load-more" role="status">继续向下滚动，加载更多任务</div>}</> }
          </div>
        </section>}
      </div>
    </div>
  )
}

function JobTimelineItem({ job, knowledgeBaseName, expanded, onToggle, onCancel, onRetry }: { job: KnowledgeJob; knowledgeBaseName: string; expanded: boolean; onToggle: () => void; onCancel: () => void; onRetry: () => void }) {
  const bucket = knowledgeJobBucket(job)
  const partial = job.status === 'completed' && job.outcome === 'partial'
  const canCancel = job.status === 'queued' || job.status === 'running' || job.status === 'paused'
  const canRetry = job.status === 'failed' || job.status === 'interrupted' || partial
  const stageSteps = KNOWLEDGE_JOB_STAGE_STEPS[job.type]
  const currentStageIndex = knowledgeJobStageIndex(job.type, job.stage)
  const totalItems = job.totalItems > 0 ? job.totalItems : Math.max(job.completedItems + job.failedItems, 1)
  const completedItems = job.completedItems > 0
    ? job.completedItems
    : job.status === 'completed' && !partial ? totalItems : 0
  // Finished jobs are history: one line each, status carried by the dot alone.
  if (bucket === 'completed') return <div className="fox-local-kb-job-line">
    <i className={cn('fox-status-dot', partial ? 'is-warning' : 'is-ok')} aria-hidden="true" />
    <span className="fox-local-kb-job-line-name">{knowledgeJobHistoryTitle(job)}</span>
    <span className="fox-local-kb-job-line-kb">{knowledgeBaseName}</span>
    <time className="fox-local-kb-job-line-time">{new Date(job.updatedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })}</time>
  </div>
  return <article className={cn('fox-local-kb-job-row', `is-${bucket}`, expanded && 'is-expanded')}>
    <button type="button" className="fox-local-kb-job-row-head" aria-expanded={expanded} onClick={onToggle}>
      {bucket === 'active' ? <LoaderCircle className="fox-local-kb-job-row-icon animate-spin" aria-hidden="true" /> : <AlertCircle className="fox-local-kb-job-row-icon is-warning" aria-hidden="true" />}
      <span className="fox-local-kb-job-row-main"><strong>{knowledgeJobTitle(job, knowledgeBaseName)}</strong><small>{knowledgeJobTypeLabel(job.type)} · {knowledgeBaseName} · {formatRelativeDate(job.updatedAt)}</small></span>
      <span className={cn('fox-local-kb-job-row-status', `is-${bucket}`)}>{bucket === 'active' ? <>{job.progress}%</> : <><i className="fox-status-dot" aria-hidden="true" />{jobStatusLabel(job.status, job.outcome)}</>}</span>
      <ChevronDown className="fox-local-kb-job-row-chevron" aria-hidden="true" />
    </button>
    {bucket === 'active' && !expanded && <Progress className="fox-local-kb-job-row-progress" value={job.progress} aria-label={`任务进度 ${job.progress}%`} />}
    {expanded && <div className="fox-local-kb-job-row-body">
      {bucket === 'active' && <div className="fox-local-kb-job-row-stage"><span>当前阶段：{formatJobStage(job.stage)}</span><Progress className="fox-local-kb-progress" value={job.progress} aria-label="任务阶段进度" /><b>{job.progress}%</b></div>}
      <ol className="fox-local-kb-job-stages">{stageSteps.map((step, index) => {
        const state = job.status === 'completed' && !partial
          ? 'complete'
          : index < currentStageIndex
            ? 'complete'
            : index === currentStageIndex
              ? bucket === 'attention' ? 'warning' : 'active'
              : 'pending'
        return <li className={`is-${state}`} key={step.label}><span>{state === 'complete' ? <CheckCircle2 /> : index + 1}</span><strong>{step.label}</strong><small>{state === 'complete' ? '已完成' : state === 'active' ? '进行中' : state === 'warning' ? '需处理' : '未开始'}</small></li>
      })}</ol>
      {job.error && <p className="fox-local-kb-job-row-error"><AlertCircle /><span>{job.error.message}</span></p>}
      <footer className="fox-local-kb-job-row-footer"><span>已完成 {completedItems}/{totalItems} 项{job.failedItems > 0 ? ` · ${job.failedItems} 项失败` : ''}</span><span>{canCancel && <Button variant="outline" size="sm" onClick={onCancel}>取消</Button>}{canRetry && <Button variant="outline" size="sm" onClick={onRetry}><RotateCcw />重试</Button>}</span></footer>
    </div>}
  </article>
}

function modelStatusLabel(model: LocalEmbeddingModel): string {
  if (model.status === 'installing') return '下载中'
  if (model.status === 'ready' && model.loadReady) return '可使用'
  if (model.status === 'ready') return '待加载测试'
  if (model.status === 'error') return '安装失败'
  return '未安装'
}

// The catalog has no marketing copy, so the one-liner comes from the language
// tag the manifest already carries.
function modelFitLine(model: LocalEmbeddingModel): string {
  const langs = model.languages
  const zh = langs.some((lang) => lang.includes('中'))
  const en = langs.some((lang) => /英|english/i.test(lang))
  if (zh && en) return '中英文混合的资料更准确'
  if (zh) return '最适合中文资料'
  if (langs.length > 0) return `适合${langs.join('、')}资料`
  return '适合多种语言的资料'
}

function embeddingModelKey(model: LocalEmbeddingModel): string {
  return `${model.id}@${model.version}`
}

export function LocalKnowledgeModelsPage({ gateway = defaultLocalKnowledgeGateway, className }: LocalKnowledgeNavigationProps) {
  const [models, setModels] = useState<LocalEmbeddingModel[]>([])
  const [health, setHealth] = useState<VectorBackendHealth | null>(null)
  const [filesModel, setFilesModel] = useState<LocalEmbeddingModel | null>(null)
  const [healthOpen, setHealthOpen] = useState(false)
  const [loading, setLoading] = useState(true)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      const [nextModels, nextHealth] = await Promise.all([gateway.listEmbeddingModels(), gateway.getVectorBackendHealth()])
      setModels(nextModels)
      setHealth(nextHealth)
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [gateway])

  useEffect(() => { void load() }, [load])
  useEffect(() => {
    if (!models.some((model) => model.status === 'installing')) return
    const timer = globalThis.setInterval(() => void load(), 900)
    return () => globalThis.clearInterval(timer)
  }, [load, models])

  const run = async (modelId: string, action: () => Promise<unknown>, success: string) => {
    setBusyId(modelId)
    setError(null)
    try {
      await action()
      toast.success(success, { sourceType: 'embedding_model', sourceId: modelId, workspaceView: 'local-knowledge-models', entityId: modelId })
      await load()
    } catch (cause) {
      toast.error('向量模型操作失败', {
        description: cause instanceof Error ? cause.message : String(cause),
        sourceType: 'embedding_model',
        sourceId: modelId,
        workspaceView: 'local-knowledge-models',
        entityId: modelId,
      })
    } finally {
      setBusyId(null)
    }
  }

  const testModel = async (model: LocalEmbeddingModel) => {
    setBusyId(model.id)
    setError(null)
    try {
      const result = await gateway.testEmbeddingModel(model.id)
      const context = {
        mergeKey: `embedding-model-test:${model.id}:${Date.now()}`,
        sourceType: 'embedding_model_test',
        sourceId: model.id,
        workspaceView: 'local-knowledge-models',
        entityId: model.id,
        status: result.loadReady ? 'completed' : 'failed',
      }
      if (result.loadReady) {
        toast.success(`${model.name} 测试通过`, {
          ...context,
          description: `${result.message}\n维度：${result.dimension}\n耗时：${result.elapsedMs} ms`,
        })
      } else {
        toast.error(`${model.name} 测试失败`, {
          ...context,
          description: [
            result.message,
            result.errorCode ? `错误码：${result.errorCode}` : null,
            result.errorDetails,
          ].filter(Boolean).join('\n\n'),
        })
      }
      await load()
    } catch (cause) {
      toast.error(`${model.name} 测试没能完成`, {
        description: cause instanceof Error ? cause.message : String(cause),
        mergeKey: `embedding-model-test:${model.id}:${Date.now()}`,
        sourceType: 'embedding_model_test',
        sourceId: model.id,
        workspaceView: 'local-knowledge-models',
        entityId: model.id,
      })
      await load()
    } finally {
      setBusyId(null)
    }
  }

  const importPackage = async () => {
    setBusyId('import')
    setError(null)
    try {
      const path = await gateway.pickStorageDirectory()
      if (!path) return
      await gateway.importEmbeddingModelPackage(path)
      toast.success('模型包已导入并通过完整性校验。', { sourceType: 'embedding_model' })
      await load()
    } catch (cause) {
      toast.error('模型包导入失败', { description: cause instanceof Error ? cause.message : String(cause), sourceType: 'embedding_model' })
    } finally {
      setBusyId(null)
    }
  }

  const currentModel = useMemo(() => models.filter(isSelectableEmbeddingModel)
    .sort((left, right) => Number(right.isDefault) - Number(left.isDefault))[0] ?? null, [models])
  const otherModels = useMemo(() => models.filter((model) => embeddingModelKey(model) !== (currentModel ? embeddingModelKey(currentModel) : '')), [currentModel, models])
  const suggestedModel = useMemo(() => otherModels.find((model) => model.recommended && model.status === 'not_installed')
    ?? otherModels.find((model) => model.status === 'not_installed') ?? null, [otherModels])
  const engineOk = health != null && health.readWriteVerified && !health.fallbackActive

  const modelMenu = (model: LocalEmbeddingModel, busy: boolean) => <DropdownMenu>
    <DropdownMenuTrigger asChild><Button className="fox-local-model-more" variant="ghost" size="icon-sm" aria-label={`管理 ${model.name}`} disabled={busy}><MoreHorizontal /></Button></DropdownMenuTrigger>
    <DropdownMenuContent className="fox-local-model-menu" align="end" sideOffset={5}>
      <DropdownMenuItem onSelect={() => void testModel(model)}><WandSparkles />试一试</DropdownMenuItem>
      <DropdownMenuItem disabled={model.isDefault || !model.loadReady} onSelect={() => void run(model.id, () => gateway.setDefaultEmbeddingModel(model.id), '已设为默认搜索模型；没有单独绑定模型的资料库会自动用它。')}><Star />设为默认</DropdownMenuItem>
      <DropdownMenuItem onSelect={() => setFilesModel(model)}><FileStack />查看文件</DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem variant="destructive" onSelect={() => void run(model.id, () => gateway.deleteEmbeddingModel(model.id), '模型已删除。')}><Trash2 />删除</DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>

  return (
    <div className={cn('fox-local-kb-page', className)}>
      <div className="fox-local-kb-content fox-local-models-page">
        <LocalKnowledgeHeader title="向量模型" description="让 Fox 读懂文字的意思，搜索时不必记得原文怎么写。装好一个就够用。" actions={<DropdownMenu>
          <DropdownMenuTrigger asChild><Button variant="ghost" size="icon-sm" aria-label="模型页更多操作"><MoreHorizontal /></Button></DropdownMenuTrigger>
          <DropdownMenuContent className="fox-local-model-menu" align="end" sideOffset={5}>
            <DropdownMenuItem disabled={busyId != null} onSelect={() => void importPackage()}><PackageOpen />导入模型包</DropdownMenuItem>
            <DropdownMenuItem onSelect={() => setHealthOpen(true)}><Gauge />运行状态<em className="fox-local-model-menu-state">{health == null ? '未知' : engineOk ? '正常' : '兼容模式'}</em></DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>} />
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        {!loading && models.length > 0 && currentModel && <section className="fox-kb-model-hero">
          <span className="fox-kb-model-hero-icon" aria-hidden="true"><Brain /></span>
          <div className="fox-kb-model-hero-text">
            <span className="fox-kb-model-hero-status"><i className="fox-status-dot is-ok" aria-hidden="true" />使用中</span>
            <strong>{currentModel.name}</strong>
            <small>{modelFitLine(currentModel)} · {formatBytes(currentModel.sizeBytes)} · {currentModel.isDefault ? '所有资料库默认使用' : '当前资料库正在使用'}</small>
          </div>
          <div className="fox-kb-model-hero-actions">
            <Button variant="outline" size="sm" onClick={() => void testModel(currentModel)} disabled={busyId === currentModel.id}><WandSparkles />试一试</Button>
            {modelMenu(currentModel, busyId === currentModel.id)}
          </div>
        </section>}
        {!loading && models.length > 0 && !currentModel && <section className="fox-kb-model-hero is-empty">
          <span className="fox-kb-model-hero-icon" aria-hidden="true"><Cpu /></span>
          <div className="fox-kb-model-hero-text">
            <span className="fox-kb-model-hero-status"><i className="fox-status-dot is-empty" aria-hidden="true" />还没有装好</span>
            <strong>现在只能按字词查找</strong>
            <small>装好一个模型后，Fox 就能明白换个说法的查询，不必记得原文怎么写。</small>
          </div>
          {suggestedModel && <div className="fox-kb-model-hero-actions">
            <Button variant="outline" size="sm" onClick={() => void run(suggestedModel.id, () => gateway.startEmbeddingModelInstall(suggestedModel.id), '模型下载任务已创建。')} disabled={busyId != null}><Download />{suggestedModel.recommended ? '下载推荐模型' : '下载'}</Button>
          </div>}
        </section>}
        {!loading && otherModels.length > 0 && <><div className="fox-kb-model-section-bar">{currentModel && <h3 className="fox-kb-model-section">其他可选</h3>}<small className="fox-kb-model-engine">本机引擎{health == null ? '状态未知' : engineOk ? '运行正常' : '使用兼容模式'}</small></div>
          <div className="fox-local-model-list">
            {otherModels.map((model) => {
              const installing = model.status === 'installing' && model.download
              const busy = busyId === model.id
              const statusTone = model.status === 'ready' && model.loadReady ? 'is-ok' : model.status === 'error' ? 'is-error' : 'is-empty'
              return <div key={embeddingModelKey(model)} className="fox-local-model-row">
                <span className="fox-kb-model-row-icon" aria-hidden="true"><Brain /></span>
                <div className="fox-local-model-row-main">
                  <strong>{model.name}</strong>
                  <small>{modelFitLine(model)} · {formatBytes(model.sizeBytes)}</small>
                  {installing && <div className="fox-local-model-download" role="status" aria-live="polite"><Progress className="fox-local-kb-progress" value={model.download?.progress ?? 0} aria-label={`${model.name} 下载进度`} /><small>{model.download?.currentFile ? `正在下载 ${model.download.currentFile}` : '准备下载'} · {formatBytes(model.download?.downloadedBytes ?? 0)} / {formatBytes(model.download?.totalBytes ?? model.sizeBytes)}</small></div>}
                </div>
                {!installing && <span className="fox-local-model-row-status"><i className={cn('fox-status-dot', statusTone)} aria-hidden="true" />{modelStatusLabel(model)}</span>}
                {model.status === 'not_installed' && <Button variant="outline" size="sm" onClick={() => void run(model.id, () => gateway.startEmbeddingModelInstall(model.id), '模型下载任务已创建。')} disabled={busy}><Download />下载</Button>}
                {installing && model.download && <Button variant="ghost" size="icon-sm" className="fox-kb-model-pause" aria-label="暂停下载" onClick={() => void run(model.id, () => gateway.cancelEmbeddingModelDownload(model.download!.id), '下载已暂停，可随时继续。')} disabled={busy}><Pause /></Button>}
                {(model.status === 'error' || model.download?.status === 'cancelled' || model.download?.status === 'failed') && model.download && !installing && <Button variant="outline" size="sm" onClick={() => void run(model.id, () => gateway.retryEmbeddingModelDownload(model.download!.id), '已从断点继续下载。')} disabled={busy}><RotateCcw />继续</Button>}
                {isSelectableEmbeddingModel(model) && <Button variant="outline" size="sm" onClick={() => void run(model.id, () => gateway.setDefaultEmbeddingModel(model.id), '已设为默认搜索模型；没有单独绑定模型的资料库会自动用它。')} disabled={busy}><Star />设为默认</Button>}
                {model.status === 'ready' && modelMenu(model, busy)}
              </div>
            })}
          </div></>}
        {!loading && !error && health && otherModels.length === 0 && <p className={cn('fox-local-model-backend', engineOk ? 'is-ok' : 'is-warning')}>
          <i className={cn('fox-status-dot', engineOk ? 'is-ok' : 'is-warning')} aria-hidden="true" />
          {engineOk ? '本机引擎运行正常' : '本机引擎处于兼容模式，智能搜索可能受影响'}
        </p>}
        <Dialog open={filesModel != null} onOpenChange={(open) => { if (!open) setFilesModel(null) }}>
          <DialogContent className="fox-local-model-files-dialog">
            <DialogHeader>
              <DialogTitle>模型文件</DialogTitle>
              <DialogDescription>{filesModel?.name} · {filesModel ? formatBytes(filesModel.sizeBytes) : ''}</DialogDescription>
            </DialogHeader>
            <div className="fox-local-model-file-list">
              {filesModel?.files.length === 0 && <p>手动模型包已通过 manifest 校验。</p>}
              {filesModel?.files.map((file) => <div className="fox-local-model-file-row" key={file.name}><span><FileText /><strong title={file.name}>{file.name}</strong></span><small>{formatBytes(file.sizeBytes)}</small><code title={file.sha256}>{file.sha256.slice(0, 12)}…</code></div>)}
            </div>
          </DialogContent>
        </Dialog>
        <Dialog open={healthOpen} onOpenChange={setHealthOpen}>
          <DialogContent className="fox-kb-model-health">
            <DialogHeader>
              <DialogTitle>运行状态</DialogTitle>
              <DialogDescription>{engineOk ? '本机引擎运行正常。' : '引擎处于兼容模式，智能搜索的结果可能不如预期。'}</DialogDescription>
            </DialogHeader>
            <dl className="fox-kb-model-health-facts">
              <div><dt>引擎</dt><dd>{health?.backend ?? '未就绪'}</dd></div>
              <div><dt>读写检查</dt><dd>{health?.readWriteVerified ? '已通过' : '未通过'}</dd></div>
              <div><dt>模式</dt><dd>{health?.fallbackActive ? '兼容模式' : '正常'}</dd></div>
            </dl>
            {health?.message && <p className="fox-kb-model-health-message">{health.message}</p>}
          </DialogContent>
        </Dialog>
      </div>
    </div>
  )
}

export function LocalKnowledgeRetrievalPage({ gateway = defaultLocalKnowledgeGateway, className, knowledgeBaseId, onBack, onNavigate, embedded = false }: EmbeddedLocalKnowledgeNavigationProps) {
  const [base, setBase] = useState<LocalKnowledgeBaseDetail | null>(null)
  const [query, setQuery] = useState('')
  const [mode, setMode] = useState<KnowledgeSearchMode>('keyword')
  const [result, setResult] = useState<KnowledgeRetrievalResult | null>(null)
  const [cases, setCases] = useState<KnowledgeRetrievalCase[]>([])
  const [loading, setLoading] = useState(true)
  const [running, setRunning] = useState(false)
  const [comparison, setComparison] = useState<Array<{
    mode: KnowledgeSearchMode
    result: KnowledgeRetrievalResult
    hitCount: number
    expectedCount: number
    hybridTop3Overlap: number | null
    hybridTop3Count: number
  }> | null>(null)
  const [comparisonRunning, setComparisonRunning] = useState(false)
  const [contexts, setContexts] = useState<Record<string, { loading: boolean; text?: string; error?: string }>>({})
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    if (!knowledgeBaseId) return
    setLoading(true)
    try {
      const [nextBase, nextCases] = await Promise.all([gateway.getKnowledgeBase(knowledgeBaseId), gateway.listRetrievalCases(knowledgeBaseId)])
      setBase(nextBase)
      setCases(nextCases)
      if (!nextBase.vectorIndexReady && mode !== 'keyword') setMode('keyword')
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }, [gateway, knowledgeBaseId, mode])

  useEffect(() => { void load() }, [load])

  const search = async (nextQuery = query) => {
    if (!knowledgeBaseId || !nextQuery.trim()) return
    setRunning(true)
    setError(null)
    try { setResult(await gateway.testRetrieval(knowledgeBaseId, nextQuery.trim(), mode, 10)) }
    catch (cause) {
      const message = cause instanceof Error ? cause.message : String(cause)
      setError(message)
      toast.error('资料库检索失败', {
        description: message,
        mergeKey: `knowledge-retrieval:${knowledgeBaseId}:${mode}`,
        sourceType: 'knowledge_retrieval',
        sourceId: knowledgeBaseId,
        workspaceView: 'local-knowledge-retrieval',
        entityId: knowledgeBaseId,
      })
    }
    finally { setRunning(false) }
  }

  const saveCase = async () => {
    if (!knowledgeBaseId || !query.trim()) return
    try {
      await gateway.saveRetrievalCase({ knowledgeBaseId, question: query.trim(), expectedDocumentIds: result?.items.slice(0, 3).map((item) => item.documentId) ?? [] })
      toast.success('测试问题已保存。', { sourceType: 'knowledge_retrieval', sourceId: knowledgeBaseId, workspaceView: 'local-knowledge-retrieval', entityId: knowledgeBaseId })
      setCases(await gateway.listRetrievalCases(knowledgeBaseId))
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }

  const exportCases = async () => {
    if (!knowledgeBaseId) return
    try {
      const exported = await gateway.exportRetrievalCases(knowledgeBaseId)
      await navigator.clipboard.writeText(exported.markdown)
      toast.success('已复制脱敏后的 Markdown 测试集。', { sourceType: 'knowledge_retrieval', sourceId: knowledgeBaseId, workspaceView: 'local-knowledge-retrieval', entityId: knowledgeBaseId })
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }

  const copyCitation = async (item: KnowledgeRetrievalResult['items'][number]) => {
    const citation = `${item.documentName} · ${item.relativePath}${item.anchor ? ` · ${item.anchor}` : ''}`
    try {
      await navigator.clipboard.writeText(citation)
      toast.success('引用信息已复制。', { sourceType: 'knowledge_retrieval', sourceId: item.chunkId, workspaceView: 'local-knowledge-retrieval', entityId: knowledgeBaseId })
    } catch {
      setError('无法访问剪贴板，请手动复制引用。')
    }
  }
  const openOriginal = async (item: KnowledgeRetrievalResult['items'][number]) => {
    if (!knowledgeBaseId) return
    try {
      const opened = await gateway.openDocumentFile(knowledgeBaseId, item.documentId)
      if (!opened) throw new Error('系统没有可用的文件打开方式')
    } catch (cause) {
      setError(`${cause instanceof Error ? cause.message : String(cause)}；可以使用“定位文档”在 Fox 中查看。`)
    }
  }
  const toggleContext = async (item: KnowledgeRetrievalResult['items'][number]) => {
    if (!knowledgeBaseId) return
    if (contexts[item.chunkId]?.text || contexts[item.chunkId]?.error) {
      setContexts((current) => { const next = { ...current }; delete next[item.chunkId]; return next })
      return
    }
    setContexts((current) => ({ ...current, [item.chunkId]: { loading: true } }))
    try {
      const chunks = await gateway.listDocumentChunks(knowledgeBaseId, item.documentId, { limit: 500, offset: 0 })
      const index = chunks.findIndex((chunk) => chunk.id === item.chunkId)
      const start = Math.max(0, index - 1)
      const surrounding = index < 0 ? [] : chunks.slice(start, index + 2)
      const text = surrounding.length ? surrounding.map((chunk, position) => `${position === index - start ? '当前段落' : '相邻段落'}\n${chunk.content}`).join('\n\n') : item.content
      setContexts((current) => ({ ...current, [item.chunkId]: { loading: false, text } }))
    } catch (cause) {
      setContexts((current) => ({ ...current, [item.chunkId]: { loading: false, error: cause instanceof Error ? cause.message : String(cause) } }))
    }
  }
  const compareCase = async (item: KnowledgeRetrievalCase) => {
    if (!knowledgeBaseId || !base?.vectorIndexReady) return
    setComparisonRunning(true)
    setComparison(null)
    setError(null)
    setQuery(item.question)
    try {
      const modes: KnowledgeSearchMode[] = ['keyword', 'vector', 'hybrid']
      const results = await Promise.all(modes.map((value) => gateway.testRetrieval(knowledgeBaseId, item.question, value, 10)))
      const expected = new Set(item.expectedDocumentIds)
      const hybridTop3 = new Set(results.find((entry) => entry.mode === 'hybrid')?.items.slice(0, 3).map((hit) => hit.documentId) ?? [])
      setComparison(results.map((entry) => ({
        mode: entry.mode,
        result: entry,
        hitCount: expected.size ? new Set(entry.items.filter((hit) => expected.has(hit.documentId)).map((hit) => hit.documentId)).size : 0,
        expectedCount: expected.size,
        hybridTop3Overlap: entry.mode === 'hybrid'
          ? null
          : new Set(entry.items.slice(0, 3).filter((hit) => hybridTop3.has(hit.documentId)).map((hit) => hit.documentId)).size,
        hybridTop3Count: hybridTop3.size,
      })))
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setComparisonRunning(false)
    }
  }

  const buildVectorIndex = async () => {
    if (!knowledgeBaseId) return
    setError(null)
    try {
      await gateway.startIndex(knowledgeBaseId, true)
      onNavigate?.('jobs', knowledgeBaseId)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  const retrievalMenu = <DropdownMenu>
    <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" aria-label="高级检索操作"><MoreHorizontal /></Button></DropdownMenuTrigger>
    <DropdownMenuContent align="end" sideOffset={5}>
      <DropdownMenuItem disabled={!query.trim() || !result} onSelect={() => void saveCase()}>保存当前问题</DropdownMenuItem>
      <DropdownMenuItem disabled={cases.length === 0} onSelect={() => void exportCases()}>导出问题</DropdownMenuItem>
    </DropdownMenuContent>
  </DropdownMenu>

  if (!knowledgeBaseId) return <div className={cn('fox-local-kb-page', className)}><div className="fox-local-kb-content"><LocalKnowledgeEmpty title="没有选择资料库" description="请先进入一个本机资料库。" action={<Button onClick={onBack}>返回</Button>} /></div></div>

  return <div className={cn('fox-local-kb-page', embedded && 'fox-local-kb-embedded-page', className)}><div className={cn('fox-local-kb-content fox-local-retrieval-page', embedded && 'fox-local-kb-embedded-content')}>
    {!embedded && <LocalKnowledgeHeader
      title="高级检索"
      description=""
      onBack={onBack}
    />}
    {loading && <LocalKnowledgeLoading />}
    {!loading && error && <LocalKnowledgeError message={error} />}
    {!loading && base && <>
      <div className="fox-local-retrieval-bar">
        <label className="fox-local-retrieval-search"><Search aria-hidden="true" /><Input value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') void search() }} placeholder="输入问题，回车检索" aria-label="检索问题" /></label>
        <Tabs value={mode} onValueChange={(value) => setMode(value as KnowledgeSearchMode)} className="fox-local-retrieval-mode-tabs">
          <TabsList aria-label="搜索方式">
            <TabsTrigger value="keyword">按字词</TabsTrigger>
            <TabsTrigger value="vector" disabled={!base.vectorIndexReady} title={base.vectorIndexReady ? undefined : '还没有整理好'}>按意思</TabsTrigger>
            <TabsTrigger value="hybrid" disabled={!base.vectorIndexReady} title={base.vectorIndexReady ? undefined : '还没有整理好'}>混合</TabsTrigger>
          </TabsList>
        </Tabs>
        {retrievalMenu}
      </div>
      {mode !== 'keyword' && !base.vectorIndexReady && <p className="fox-local-retrieval-hint" role="status"><i className="fox-status-dot is-empty" aria-hidden="true" />智能搜索还没有开启{base.documentCount > 0 && <> · <button type="button" className="fox-local-kb-readiness-action" onClick={() => void buildVectorIndex()}>现在整理</button></>}</p>}
      {result && <section className="fox-local-retrieval-results" aria-live="polite">
        <div className="fox-local-kb-section-head"><div><h2>搜索结果</h2><p>{result.mode === 'hybrid' ? `${result.fusionVersion} · ` : ''}按字词 {result.timings.keywordMs} ms · 按意思 {result.timings.vectorMs} ms · 总计 {result.timings.totalMs} ms</p></div></div>
        {result.items.length === 0 ? <LocalKnowledgeEmpty title="没有找到相关内容" description="换个说法再试，或者先重新整理一次资料。" /> : <ol className="fox-local-retrieval-result-list">{result.items.map((item, index) => <li key={item.chunkId} className="fox-local-retrieval-result">
          <div className="fox-local-retrieval-result-head"><b>{index + 1}</b><div><strong>{item.documentName}</strong><small>{item.relativePath}{item.anchor ? ` · ${item.anchor}` : ''}</small></div><span className="fox-local-retrieval-score" title={`按字词 ${item.keywordScore?.toFixed(3) ?? '—'} · 按意思 ${item.vectorScore?.toFixed(3) ?? '—'}`}>{item.score.toFixed(3)}</span></div>
          <p>{item.content}</p>
          {contexts[item.chunkId] && <div className={`fox-local-retrieval-context ${contexts[item.chunkId].error ? 'is-error' : ''}`}>{contexts[item.chunkId].loading ? <><LoaderCircle className="animate-spin" />正在读取相邻段落</> : <pre>{contexts[item.chunkId].error ?? contexts[item.chunkId].text}</pre>}</div>}
          <footer className="fox-local-retrieval-result-actions"><Button variant="ghost" size="sm" onClick={() => void toggleContext(item)}>{contexts[item.chunkId] ? '收起上下文' : '展开上下文'}</Button><Button variant="ghost" size="sm" onClick={() => void copyCitation(item)}>复制引用</Button><Button variant="ghost" size="sm" onClick={() => onNavigate?.('documents', knowledgeBaseId, item.documentId)}>找到原文</Button><Button variant="ghost" size="sm" onClick={() => void openOriginal(item)}>打开原文件</Button></footer>
        </li>)}</ol>}
      </section>}
      {comparison && <section className="fox-local-retrieval-comparison">
        <div className="fox-local-kb-section-head"><div><h2>三种搜索方式对比</h2><p>同一个问题、各取前 10 段；没有标注答案时，显示与混合方式前 3 名的重合度。</p></div></div>
        <div className="fox-local-retrieval-comparison-list">{comparison.map((item) => <div key={item.mode} className="fox-local-retrieval-comparison-row"><strong>{item.mode === 'keyword' ? '关键词' : item.mode === 'vector' ? '向量' : '混合'}</strong><span>{item.result.timings.totalMs} ms</span><span>{item.result.items.length} 个命中</span><span>{item.expectedCount ? `命中 ${item.hitCount}/${item.expectedCount}` : '未标注答案'}</span><span>{item.hybridTop3Overlap == null ? '混合基准' : `Top 3 重合 ${item.hybridTop3Overlap}/${item.hybridTop3Count}`}</span><small>{item.result.items[0]?.documentName ?? '无结果'}</small></div>)}</div>
      </section>}
      <section className="fox-local-retrieval-cases">
        <div className="fox-local-kb-section-head"><div><h2>测试问题</h2><p>最多保留 20 条，可重复对比检索策略。</p></div></div>
        {cases.length === 0 ? <p className="fox-local-retrieval-empty">还没有保存测试问题。</p> : <div className="fox-local-retrieval-case-list">{cases.map((item) => <div key={item.id} className="fox-local-retrieval-case"><button type="button" onClick={() => { setQuery(item.question); void search(item.question) }}>{item.question}</button><span className="fox-local-retrieval-case-actions"><Button variant="ghost" size="sm" disabled={!base.vectorIndexReady || comparisonRunning} onClick={() => void compareCase(item)}>{comparisonRunning ? <LoaderCircle className="animate-spin" /> : <FlaskConical />}三模式对比</Button><Button variant="ghost" size="sm" onClick={() => void gateway.deleteRetrievalCase(item.id).then(() => setCases((current) => current.filter((entry) => entry.id !== item.id)))}><Trash2 />删除</Button></span></div>)}</div>}
      </section>
    </>}
  </div></div>
}

export function LocalKnowledgeWorkspace({ view = 'list', knowledgeBaseId, documentId, gateway = defaultLocalKnowledgeGateway, onNavigate, onNavigateWorkspace, onBack, createRequest, onCreateRequestHandled, onOperationAccepted, onAskDocument }: LocalKnowledgeNavigationProps & { view?: LocalKnowledgeView; createRequest?: number; onOperationAccepted?: (accepted: OperationAccepted) => void }) {
  const navigate = onNavigate
  switch (view) {
    case 'home': return <LocalKnowledgeHomePage gateway={gateway} onNavigate={navigate} />
    case 'files': return knowledgeBaseId ? <LocalFolderDetailPage key={knowledgeBaseId} gateway={gateway} sourceId={knowledgeBaseId} onBack={() => navigate?.('list')} renderPreview={(file, close) => <LocalFilePreviewDialog gateway={gateway} file={file} open={Boolean(file)} onOpenChange={(open) => { if (!open) close() }} />} /> : <LocalKnowledgeListPage gateway={gateway} onNavigate={navigate} onNavigateWorkspace={onNavigateWorkspace} />
    case 'detail': return <LocalKnowledgeDetailPage gateway={gateway} knowledgeBaseId={knowledgeBaseId} onBack={onBack} onNavigate={navigate} onOperationAccepted={onOperationAccepted} onAskDocument={onAskDocument} />
    case 'documents': return <LocalKnowledgeDetailPage gateway={gateway} knowledgeBaseId={knowledgeBaseId} documentId={documentId} onBack={onBack} onNavigate={navigate} onOperationAccepted={onOperationAccepted} onAskDocument={onAskDocument} />
    case 'import': return <LocalKnowledgeDetailPage gateway={gateway} knowledgeBaseId={knowledgeBaseId} onBack={onBack} onNavigate={navigate} initialImportOpen onOperationAccepted={onOperationAccepted} />
    case 'jobs': return <LocalKnowledgeJobsPage gateway={gateway} knowledgeBaseId={knowledgeBaseId} onBack={onBack} onNavigate={navigate} />
    case 'models': return <LocalKnowledgeModelsPage gateway={gateway} onBack={onBack} />
    case 'retrieval': return <LocalKnowledgeRetrievalPage gateway={gateway} knowledgeBaseId={knowledgeBaseId} onBack={onBack} onNavigate={navigate} />
    case 'settings': return <LocalKnowledgeListPage gateway={gateway} initialEditId={knowledgeBaseId} onNavigate={navigate} onNavigateWorkspace={onNavigateWorkspace} />
    default: return <LocalKnowledgeListPage gateway={gateway} onNavigate={navigate} onNavigateWorkspace={onNavigateWorkspace} createRequest={createRequest} onCreateRequestHandled={onCreateRequestHandled} />
  }
}
