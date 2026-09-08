import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ChangeEvent, type CSSProperties, type FormEvent, type MouseEvent as ReactMouseEvent, type ReactNode } from 'react'
import { AlertCircle, ArrowLeft, ArrowRight, BookOpen, CalendarDays, CheckCircle2, ChevronDown, Clock3, Copy, Cpu, Database, Download, ExternalLink, FileQuestion, FileStack, FileText, FlaskConical, Folder, FolderOpen, FolderPlus, Gauge, HardDrive, History, Info, Layers3, LoaderCircle, MoreHorizontal, PackageOpen, Pencil, RefreshCw, RotateCcw, Search, ShieldCheck, Star, Trash2, Upload, XCircle } from 'lucide-react'
import { MessageResponse } from '@/components/ai-elements/message-response'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card, CardContent, CardHeader } from '@/components/ui/card'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Tabs, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { Textarea } from '@/components/ui/textarea'
import { cn } from '@/lib/utils'
import type { KnowledgeDocumentSourceMetadata } from '@/features/conversations/model/types'
import { notify as toast } from '@/features/notifications'
import { previewKind } from '@/features/knowledge/knowledge-preview-model'
import { binaryValue, createKnowledgeCachedPreviewSource, type KnowledgeCachedPreviewOpener } from '@/features/knowledge/knowledge-preview-source'
import { createOperationState, reduceOperationEvent, type OperationState } from './operation-reducer'
import { defaultLocalKnowledgeGateway } from './tauri-gateway'
import { LocalFileTypeIcon } from './local-file-type-icon'
import type { KnowledgeChunkPreview, KnowledgeJob, KnowledgeJobStatus, KnowledgeOperationStage, KnowledgeRetrievalCase, KnowledgeRetrievalResult, KnowledgeSearchMode, LocalEmbeddingModel, LocalFileCategory, LocalKnowledgeBase, LocalKnowledgeBaseCreateRequest, LocalKnowledgeBaseDetail, LocalKnowledgeCatalogFile, LocalKnowledgeDocument, LocalKnowledgeFileSource, LocalKnowledgeFolder, LocalKnowledgeGateway, LocalKnowledgeImportFile, LocalKnowledgeStorageMigration, LocalKnowledgeStorageStatus, OperationAccepted, StorageMigrationStage, StorageMigrationStatus, VectorBackendHealth } from './model'
import { formatBytes, formatJobStage } from './model'
import '@/styles/local-knowledge.css'

const KnowledgePdfViewer = lazy(() => import('@/features/knowledge/knowledge-pdf-viewer'))
const KnowledgeDocxViewer = lazy(() => import('@/features/knowledge/knowledge-docx-viewer'))
const KnowledgePptxViewer = lazy(() => import('@/features/knowledge/knowledge-pptx-viewer'))
const KnowledgeSpreadsheetViewer = lazy(() => import('@/features/knowledge/knowledge-spreadsheet-viewer'))
const MAX_LOCAL_TEXT_PREVIEW_BYTES = 2 * 1024 * 1024
const LOCAL_FILE_PAGE_SIZE = 12
const KNOWLEDGE_JOB_PAGE_SIZE = 8
const RECENT_DOCUMENT_LIMIT = 8
const RECENT_DOCUMENT_STORAGE_KEY = 'fox.local-knowledge.recent-documents'
const LOCAL_KNOWLEDGE_ACTIVITY_STORAGE_KEY = 'fox.local-knowledge.activity-history'
const LOCAL_KNOWLEDGE_ACTIVITY_LIMIT = 80
const LOCAL_KNOWLEDGE_ACTIVITY_DISPLAY_LIMIT = 10
const LOCAL_KNOWLEDGE_MASCOT = '/mascot/fox/status/fox_give_flowers.png'
const FOX_GUIDE_KNOWLEDGE_BASE_ID = 'fox-user-guide'

export type LocalKnowledgeView = 'home' | 'files' | 'list' | 'detail' | 'documents' | 'import' | 'jobs' | 'models' | 'retrieval'

interface RecentLocalKnowledgeDocument {
  knowledgeBaseId: string
  knowledgeBaseName: string
  documentId: string
  name: string
  extension: string
  relativePath: string
  openedAt: string
}

type KnowledgeReadinessStage = 'documents' | 'chunks' | 'text' | 'vector'
type KnowledgeDetailSubview = 'documents' | 'retrieval' | 'jobs'
type KnowledgeDetailTab = 'detail' | KnowledgeDetailSubview
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
      title: wasDeleted ? '重新导入文档' : '导入文档',
      detail,
      occurredAt: new Date(now + index).toISOString(),
    }
  })
  saveLocalKnowledgeActivities([...imported.reverse(), ...current])
}

function knowledgeJobStageActivityTitle(job: KnowledgeJob, stage: KnowledgeOperationStage): string {
  if (job.type === 'delete') return stage === 'committing' ? '完成文档删除' : '删除文档'
  const titleByStage: Record<KnowledgeOperationStage, string> = {
    validating: job.type === 'import' ? '校验导入文件' : '校验文档',
    copying: '复制导入文件',
    hashing: '计算文档哈希',
    parsing: '解析文档',
    chunking: '文本分块',
    embedding: '向量化索引',
    vector_upsert: '写入向量索引',
    verifying: '校验索引',
    committing: job.type === 'rebuild'
      ? '完成索引重建'
      : job.type === 'import'
        ? '完成文档导入'
        : job.type === 'parse'
          ? '完成文档解析'
          : '完成索引更新',
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
}

export interface LocalKnowledgeNavigationProps extends LocalKnowledgePageProps {
  knowledgeBaseId?: string
  documentId?: string
  createRequest?: number
  onCreateRequestHandled?: () => void
  onBack?: () => void
  onNavigate?: (view: LocalKnowledgeView, knowledgeBaseId?: string, documentId?: string) => void
}

type EmbeddedLocalKnowledgeNavigationProps = LocalKnowledgeNavigationProps & { embedded?: boolean }

function formatDate(value: string | null | undefined): string {
  if (!value) return '尚未记录'
  return new Date(value).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })
}

function displayLocalPath(value: string): string {
  if (value.startsWith('\\\\?\\UNC\\')) return `\\\\${value.slice(8)}`
  if (value.startsWith('\\\\?\\')) return value.slice(4)
  return value
}

function formatTime(value: string | null | undefined): string {
  if (!value) return '--:--'
  return new Date(value).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
}

function baseStatusLabel(status: LocalKnowledgeBase['status']): string {
  return ({ empty: '待导入', indexing: '索引中', ready: '已就绪', error: '有错误', migrating: '迁移中' })[status]
}

function documentStatusLabel(status: LocalKnowledgeDocument['status']): string {
  return ({ queued: '排队中', parsing: '解析中', indexed: '已索引', error: '失败', stale: '待重建' })[status]
}

function jobStatusLabel(status: KnowledgeJobStatus, outcome: KnowledgeJob['outcome']): string {
  if (status === 'completed' && outcome === 'partial') return '部分完成'
  return ({ queued: '排队中', running: '处理中', paused: '已暂停', completed: '已完成', failed: '失败', cancelled: '已取消', interrupted: '已中断' })[status]
}

function jobStatusIcon(status: KnowledgeJobStatus) {
  if (status === 'completed') return <CheckCircle2 />
  if (status === 'failed' || status === 'interrupted') return <XCircle />
  if (status === 'cancelled') return <AlertCircle />
  if (status === 'queued' || status === 'paused') return <Clock3 />
  return <LoaderCircle className="animate-spin" />
}

function storageMigrationStatusLabel(status: StorageMigrationStatus): string {
  return ({ queued: '等待迁移', running: '迁移中', paused: '已暂停', completed: '迁移完成', failed: '迁移失败', cancelled: '已取消', interrupted: '已中断' })[status]
}

function storageMigrationStageLabel(stage: StorageMigrationStage | null): string {
  return ({
    validating: '校验目标目录',
    quiescing: '暂停知识库任务',
    flushing: '写入并刷新索引',
    closing_handles: '释放文件句柄',
    copying: '复制知识库文件',
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

function StorageLocationDialog({ gateway, open, onOpenChange }: { gateway: LocalKnowledgeGateway; open: boolean; onOpenChange: (open: boolean) => void }) {
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
          toast.success('本地知识库存储迁移已完成', { description: '请重启 Fox，使新的存储位置生效。', sourceType: 'knowledge_storage_migration', sourceId: migrationId })
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
          <DialogTitle>本地知识库存储位置</DialogTitle>
          <DialogDescription>选择新的根目录后，Fox 会自动迁移数据库、文件和本地索引。</DialogDescription>
        </DialogHeader>
        <div className="fox-local-kb-storage-body">
          {loading && <div className="fox-local-kb-storage-path is-loading"><LoaderCircle className="animate-spin" /><span>正在读取存储位置...</span></div>}
          {!loading && status && <>
            <label className="fox-local-kb-storage-label">当前位置</label>
            <div className="fox-local-kb-storage-path"><FolderOpen aria-hidden="true" /><code title={status.rootPath}>{status.rootPath}</code></div>
            {pendingPath && <><label className="fox-local-kb-storage-label">迁移目标</label><div className="fox-local-kb-storage-pending"><ArrowLeft aria-hidden="true" /><code title={pendingPath}>{pendingPath}</code></div></>}
            {migration && <div className="fox-local-kb-storage-progress" role="status" aria-live="polite"><div className="fox-local-kb-storage-progress-head"><span>{storageMigrationStatusLabel(migration.status)}</span><b>{migration.progress}%</b></div><div className="fox-local-kb-progress"><span style={{ width: `${Math.max(0, Math.min(100, migration.progress))}%` }} /></div><small>{storageMigrationStageLabel(migration.stage)}</small></div>}
            {completed && <p className="fox-local-kb-storage-success"><CheckCircle2 />迁移已完成，重启 Fox 后新存储位置才会生效。</p>}
            {(error || migrationError) && <p className="fox-local-kb-storage-error" role="alert"><AlertCircle />{migrationError ?? error}</p>}
          </>}
          {!loading && !status && error && <p className="fox-local-kb-storage-error" role="alert"><AlertCircle />{error}</p>}
        </div>
        <DialogFooter className="fox-local-kb-storage-actions">
          {!loading && !status && error ? <Button type="button" variant="outline" onClick={() => void loadStatus()}>重试</Button> : <>
            <Button type="button" variant="outline" onClick={() => void chooseDirectory()} disabled={busy}>{picking ? <LoaderCircle className="animate-spin" /> : <FolderOpen />}{picking ? '正在选择' : '选择文件夹'}</Button>
            <Button type="button" onClick={() => void confirmMigration()} disabled={!selectedPath || busy}>{starting ? <LoaderCircle className="animate-spin" /> : <CheckCircle2 />}{starting ? '正在提交' : '确认迁移'}</Button>
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
        {onBack && <Button aria-label="返回本地知识库" className="fox-local-kb-back" variant="ghost" size="icon" onClick={onBack}><ArrowLeft /></Button>}
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
  return <div className="fox-local-kb-loading"><LoaderCircle className="animate-spin" /><span>正在读取本地知识库...</span></div>
}

function LocalKnowledgeError({ message, onRetry }: { message: string; onRetry?: () => void }) {
  return <div className="fox-local-kb-error"><AlertCircle /><span>{message}</span>{onRetry && <Button size="sm" variant="outline" onClick={onRetry}>重试</Button>}</div>
}

function LocalKnowledgeEmpty({ title, description, action }: { title: string; description: string; action?: ReactNode }) {
  return <div className="fox-local-kb-empty"><Database /><div><strong>{title}</strong><span>{description}</span></div>{action}</div>
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
}: {
  gateway: LocalKnowledgeGateway
  open: boolean
  submitting: boolean
  error: string | null
  onOpenChange: (open: boolean) => void
  onSubmit: (request: LocalKnowledgeBaseCreateRequest) => Promise<void>
}) {
  const [name, setName] = useState('')
  const [description, setDescription] = useState('')
  const [models, setModels] = useState<LocalEmbeddingModel[]>([])
  const [embeddingModelId, setEmbeddingModelId] = useState('none')
  const [chunkSize, setChunkSize] = useState('512')
  const [chunkOverlap, setChunkOverlap] = useState('50')
  const [searchMode, setSearchMode] = useState<KnowledgeSearchMode>('keyword')
  const [validationError, setValidationError] = useState<string | null>(null)
  const [modelsLoading, setModelsLoading] = useState(false)
  const [modelsLoadError, setModelsLoadError] = useState(false)

  useEffect(() => {
    if (!open) {
      setName('')
      setDescription('')
      setEmbeddingModelId('none')
      setChunkSize('512')
      setChunkOverlap('50')
      setSearchMode('keyword')
      setValidationError(null)
      setModels([])
      setModelsLoadError(false)
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

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const normalizedName = name.trim()
    if (!normalizedName) {
      setValidationError('请输入知识库名称。')
      return
    }
    const normalizedChunkSize = Number(chunkSize)
    const normalizedChunkOverlap = Number(chunkOverlap)
    if (!Number.isInteger(normalizedChunkSize) || normalizedChunkSize < 128 || normalizedChunkSize > 4096) {
      setValidationError('分块大小需为 128–4096 的整数。')
      return
    }
    if (!Number.isInteger(normalizedChunkOverlap) || normalizedChunkOverlap < 0 || normalizedChunkOverlap >= normalizedChunkSize) {
      setValidationError('重叠长度需为非负整数，且必须小于分块大小。')
      return
    }
    if (searchMode !== 'keyword' && !models.some((model) => model.id === embeddingModelId && isSelectableEmbeddingModel(model))) {
      setValidationError('向量或混合检索需要选择一个已安装的向量模型。')
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
    })
  }

  const requestOpenChange = (nextOpen: boolean) => {
    if (submitting) return
    onOpenChange(nextOpen)
  }

  return (
    <Dialog open={open} onOpenChange={requestOpenChange}>
      <DialogContent className="fox-local-kb-create-dialog" showCloseButton={!submitting}>
        <DialogHeader>
          <DialogTitle>新建本地知识库</DialogTitle>
          <DialogDescription>创建后可以导入文件并建立本地索引；检测到默认向量模型时会自动选择混合检索。</DialogDescription>
        </DialogHeader>
        <form className="fox-local-kb-create-form" onSubmit={submit}>
          <div>
            <label htmlFor="local-knowledge-base-name">名称 <span>*</span></label>
            <Input id="local-knowledge-base-name" autoFocus placeholder="例如：产品资料库" value={name} onChange={(event) => { setName(event.target.value); setValidationError(null) }} disabled={submitting} />
          </div>
          <div>
            <label htmlFor="local-knowledge-base-description">描述 <small>（可选）</small></label>
            <Textarea id="local-knowledge-base-description" className="fox-local-kb-create-textarea" placeholder="简要说明这个知识库的用途" value={description} onChange={(event) => setDescription(event.target.value)} disabled={submitting} />
          </div>
          <fieldset className="fox-local-kb-index-settings">
            <legend>索引与检索</legend>
            <label>向量模型<Select value={embeddingModelId} onValueChange={(value) => { if (value !== 'none' && !models.some((model) => model.id === value && isSelectableEmbeddingModel(model))) return; setEmbeddingModelId(value); if (value === 'none') setSearchMode('keyword') }} disabled={submitting || modelsLoading}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="none">暂不使用向量模型</SelectItem>{models.map((model) => <SelectItem key={model.id} value={model.id} disabled={!isSelectableEmbeddingModel(model)}>{model.name} · {model.dimension} 维{isSelectableEmbeddingModel(model) ? '' : ` · ${modelStatusLabel(model)}`}</SelectItem>)}</SelectContent></Select></label>
            <label>检索模式<Select value={searchMode} onValueChange={(value) => setSearchMode(value as KnowledgeSearchMode)} disabled={submitting}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="keyword">关键词检索</SelectItem><SelectItem value="vector" disabled={embeddingModelId === 'none'}>向量检索</SelectItem><SelectItem value="hybrid" disabled={embeddingModelId === 'none'}>混合检索</SelectItem></SelectContent></Select></label>
            <label>分块大小<Input type="number" min={128} max={4096} step={1} value={chunkSize} onChange={(event) => setChunkSize(event.target.value)} disabled={submitting} /></label>
            <label>重叠长度<Input type="number" min={0} step={1} value={chunkOverlap} onChange={(event) => setChunkOverlap(event.target.value)} disabled={submitting} /></label>
            {modelsLoading && <p><LoaderCircle className="animate-spin" />正在读取向量模型...</p>}
            {!modelsLoading && modelsLoadError && <p><AlertCircle />向量模型列表暂不可用。可以先创建关键词知识库，稍后再编辑检索配置。</p>}
            {!modelsLoading && !modelsLoadError && models.length === 0 && <p><Cpu />还没有可用的向量模型。可先创建关键词知识库，再到“向量模型”页面安装。</p>}
            {!modelsLoading && models.length > 0 && !models.some(isSelectableEmbeddingModel) && <p><Info />已显示可安装模型；请先在“向量模型”页面完成安装与测试，之后即可选择。</p>}
            {models.some(isSelectableEmbeddingModel) && embeddingModelId !== 'none' && <p><Info />可用模型已选中。导入文档后，请在知识库“向量索引”步骤点击构建。</p>}
          </fieldset>
          {(validationError || error) && <p className="fox-local-kb-create-error" role="alert"><AlertCircle />{validationError ?? error}</p>}
          <DialogFooter className="fox-local-kb-create-actions">
            <Button type="button" variant="outline" onClick={() => requestOpenChange(false)} disabled={submitting}>取消</Button>
            <Button type="submit" disabled={submitting}>{submitting && <LoaderCircle className="animate-spin" />}创建知识库</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function EditKnowledgeBaseDialog({
  gateway,
  base,
  open,
  submitting,
  error,
  onOpenChange,
  onSubmit,
}: {
  gateway: LocalKnowledgeGateway
  base: LocalKnowledgeBaseDetail
  open: boolean
  submitting: boolean
  error: string | null
  onOpenChange: (open: boolean) => void
  onSubmit: (request: LocalKnowledgeBaseCreateRequest) => Promise<void>
}) {
  const [name, setName] = useState(base.name)
  const [description, setDescription] = useState(base.description)
  const [models, setModels] = useState<LocalEmbeddingModel[]>([])
  const [embeddingModelId, setEmbeddingModelId] = useState(base.configuredEmbeddingModelId ?? 'none')
  const [chunkSize, setChunkSize] = useState(String(base.chunkSize))
  const [chunkOverlap, setChunkOverlap] = useState(String(base.chunkOverlap))
  const [searchMode, setSearchMode] = useState<KnowledgeSearchMode>(base.searchMode)
  const [validationError, setValidationError] = useState<string | null>(null)
  const [dirty, setDirty] = useState(false)

  useEffect(() => {
    if (!open) return
    setName(base.name)
    setDescription(base.description)
    setEmbeddingModelId(base.configuredEmbeddingModelId ?? 'none')
    setChunkSize(String(base.chunkSize))
    setChunkOverlap(String(base.chunkOverlap))
    setSearchMode(base.searchMode)
    setValidationError(null)
    setDirty(false)
    void gateway.listEmbeddingModels().then(setModels).catch(() => setModels([]))
  }, [base, gateway, open])

  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const normalizedName = name.trim()
    if (!normalizedName) {
      setValidationError('请输入知识库名称。')
      return
    }
    const normalizedChunkSize = Number(chunkSize)
    const normalizedChunkOverlap = Number(chunkOverlap)
    if (!Number.isInteger(normalizedChunkSize) || normalizedChunkSize < 128 || normalizedChunkSize > 4096) {
      setValidationError('分块大小需为 128–4096 的整数。')
      return
    }
    if (!Number.isInteger(normalizedChunkOverlap) || normalizedChunkOverlap < 0 || normalizedChunkOverlap >= normalizedChunkSize) {
      setValidationError('重叠长度需为非负整数，且必须小于分块大小。')
      return
    }
    if (searchMode !== 'keyword' && !models.some((model) => model.id === embeddingModelId && isSelectableEmbeddingModel(model))) {
      setValidationError('向量或混合检索需要选择一个已安装的向量模型。')
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

  const requestOpenChange = (nextOpen: boolean) => {
    if (submitting) return
    if (!nextOpen && dirty && !window.confirm('放弃尚未保存的知识库修改？')) return
    onOpenChange(nextOpen)
  }

  return (
    <Dialog open={open} onOpenChange={requestOpenChange}>
      <DialogContent className="fox-local-kb-create-dialog" showCloseButton={!submitting}>
        <DialogHeader>
          <DialogTitle>编辑本地知识库</DialogTitle>
          <DialogDescription>修改基本信息和检索配置。模型或分块参数变化后会重建向量索引。</DialogDescription>
        </DialogHeader>
        <form className="fox-local-kb-create-form" onSubmit={submit}>
          <div>
            <label htmlFor="local-knowledge-base-edit-name">名称 <span>*</span></label>
            <Input id="local-knowledge-base-edit-name" autoFocus value={name} onChange={(event) => { setName(event.target.value); setValidationError(null); setDirty(true) }} disabled={submitting} />
          </div>
          <div>
            <label htmlFor="local-knowledge-base-edit-description">描述 <small>（可选）</small></label>
            <Textarea id="local-knowledge-base-edit-description" className="fox-local-kb-create-textarea" value={description} onChange={(event) => { setDescription(event.target.value); setDirty(true) }} disabled={submitting} />
          </div>
          <fieldset className="fox-local-kb-index-settings">
            <legend>索引与检索</legend>
            <label>向量模型<Select value={embeddingModelId} onValueChange={(value) => { if (value !== 'none' && !models.some((model) => model.id === value && isSelectableEmbeddingModel(model))) return; setEmbeddingModelId(value); if (value === 'none') setSearchMode('keyword'); setDirty(true) }} disabled={submitting}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="none">暂不使用向量模型</SelectItem>{models.map((model) => <SelectItem key={model.id} value={model.id} disabled={!isSelectableEmbeddingModel(model)}>{model.name} · {model.dimension} 维{isSelectableEmbeddingModel(model) ? '' : ` · ${modelStatusLabel(model)}`}</SelectItem>)}</SelectContent></Select></label>
            <label>检索模式<Select value={searchMode} onValueChange={(value) => { setSearchMode(value as KnowledgeSearchMode); setDirty(true) }} disabled={submitting}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="keyword">关键词检索</SelectItem><SelectItem value="vector" disabled={embeddingModelId === 'none'}>向量检索</SelectItem><SelectItem value="hybrid" disabled={embeddingModelId === 'none'}>混合检索</SelectItem></SelectContent></Select></label>
            <label>分块大小<Input type="number" min={128} max={4096} step={1} value={chunkSize} onChange={(event) => { setChunkSize(event.target.value); setDirty(true) }} disabled={submitting} /></label>
            <label>重叠长度<Input type="number" min={0} step={1} value={chunkOverlap} onChange={(event) => { setChunkOverlap(event.target.value); setDirty(true) }} disabled={submitting} /></label>
          </fieldset>
          {(validationError || error) && <p className="fox-local-kb-create-error" role="alert"><AlertCircle />{validationError ?? error}</p>}
          <DialogFooter className="fox-local-kb-create-actions">
            <Button type="button" variant="outline" onClick={() => requestOpenChange(false)} disabled={submitting}>取消</Button>
            <Button type="submit" disabled={submitting}>{submitting && <LoaderCircle className="animate-spin" />}保存修改</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function DeleteKnowledgeBaseDialog({
  base,
  open,
  submitting,
  error,
  onOpenChange,
  onConfirm,
}: {
  base: LocalKnowledgeBaseDetail
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
          <DialogTitle>删除本地知识库</DialogTitle>
          <DialogDescription>此操作会删除知识库中的全部文档和本地索引，无法撤销。</DialogDescription>
        </DialogHeader>
        <div className="fox-local-kb-delete-summary"><Trash2 /><div><strong>{base.name}</strong><span>{base.documentCount} 个文档将被永久删除</span></div></div>
        {error && <p className="fox-local-kb-create-error" role="alert"><AlertCircle />{error}</p>}
        <DialogFooter className="fox-local-kb-create-actions">
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)} disabled={submitting}>取消</Button>
          <Button type="button" variant="destructive" onClick={() => void onConfirm()} disabled={submitting}>{submitting ? <LoaderCircle className="animate-spin" /> : <Trash2 />}{submitting ? '正在删除' : '确认删除'}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

function KnowledgeStatusBadge({ status }: { status: LocalKnowledgeBase['status'] }) {
  return <Badge variant="outline" className={cn('fox-local-kb-status', `is-${status}`)}><i />{baseStatusLabel(status)}</Badge>
}

function countLabel(value: number | null): string {
  return value == null ? '暂无统计' : String(value)
}

function generationLabel(base: LocalKnowledgeBase): string {
  return base.vectorIndexReady && base.activeGeneration != null ? `G${base.activeGeneration}` : '未建立'
}

function searchModeLabel(base: LocalKnowledgeBase): string {
  if (!base.vectorIndexReady) return base.textIndexReady ? '仅关键词检索' : '关键词检索未就绪'
  if (base.searchMode === 'vector' && base.vectorIndexReady) return '向量检索'
  if (base.searchMode === 'hybrid' && base.vectorIndexReady) return '混合检索'
  return '关键词检索'
}

function KnowledgeBaseCard({ base, onOpen }: { base: LocalKnowledgeBase; onOpen: () => void }) {
  return (
    <Card className="fox-library-card fox-library-card--stacked-meta fox-shadcn-kb-card fox-shadcn-agent-card fox-shadcn-knowledge-card fox-local-kb-card-view" onClick={onOpen} onKeyDown={(event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); onOpen() } }} role="button" tabIndex={0} aria-label={`打开知识库${base.name}`}>
      <CardHeader className="fox-shadcn-kb-head">
        <span className="fox-shadcn-kb-icon fox-shadcn-agent-icon is-blue"><Database /></span>
        <span className="fox-shadcn-kb-heading"><span className="fox-agent-name-row"><strong>{base.name}</strong></span></span>
      </CardHeader>
      <CardContent className="fox-shadcn-kb-content">
        <p className="fox-agent-description" title={base.description}>{base.description || '本地知识库'}</p>
        <span className="fox-library-card-tags"><small>本地知识库</small><small>{base.documentCount} 文档</small><small>{countLabel(base.chunkCount)} 分块</small>{base.status !== 'ready' && <small>{baseStatusLabel(base.status)}</small>}</span>
      </CardContent>
    </Card>
  )
}

const localFileCategories: Array<{ value: LocalFileCategory; label: string }> = [
  { value: 'all', label: '全部' },
  { value: 'text', label: '文本' },
  { value: 'document', label: '文档' },
  { value: 'pdf', label: 'PDF' },
  { value: 'presentation', label: '演示文稿' },
  { value: 'spreadsheet', label: '表格' },
]

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
    if (!file || !metadata) throw new Error('没有选择需要预览的本地文件。')
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

  const handleFailure = useCallback((message: string) => setError(message), [])
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
      <DialogContent className="fox-local-files-preview-dialog">
        <DialogHeader>
          <DialogTitle>{file?.name ?? '本地文件预览'}</DialogTitle>
          <DialogDescription title={file?.absolutePath}>{file?.absolutePath}</DialogDescription>
        </DialogHeader>
        <div className="fox-local-files-preview-body">
          {loading && <div className="fox-file-preview-state is-loading"><LoaderCircle className="animate-spin" /><b>正在准备预览</b></div>}
          {!loading && !error && !unsupported && <Suspense fallback={<div className="fox-file-preview-state is-loading"><LoaderCircle className="animate-spin" /><b>正在加载查看器</b></div>}>{preview}</Suspense>}
          {!loading && (unsupported || error) && <div className="fox-file-preview-state"><FileQuestion /><b>{error ? '预览加载失败' : '暂不支持内置预览'}</b><p>{error ?? '当前文件格式暂不支持在 Fox 中直接预览。'}</p><Button onClick={() => void openWithSystem()}><ExternalLink />用系统应用打开</Button></div>}
        </div>
        <DialogFooter className="fox-local-files-preview-actions"><Button variant="outline" onClick={() => void revealInFolder()}><FolderOpen />打开所在文件夹</Button><Button variant="outline" onClick={() => void openWithSystem()}><ExternalLink />系统打开</Button></DialogFooter>
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

function LocalKnowledgeDocumentReader({ knowledgeBaseId, document, gateway, onBack, onDeleted, onJobAccepted, embedded = false }: { knowledgeBaseId: string; document: LocalKnowledgeDocument; gateway: LocalKnowledgeGateway; onBack: () => void; onDeleted: () => Promise<void>; onJobAccepted: () => void; embedded?: boolean }) {
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
      recordLocalKnowledgeActivity({ knowledgeBaseId, kind: 'document_deleted', title: '删除文档', detail: document.relativePath || document.name })
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
  const readerActions = <><Button variant="outline" onClick={() => void openChunks()}><Layers3 />分块预览</Button><Button variant="outline" onClick={() => void reparse()} disabled={actionBusy}><RefreshCw />重新解析</Button><Button variant="outline" onClick={() => void revealInFolder()}><FolderOpen />打开所在文件夹</Button><Button variant="outline" onClick={() => void openWithSystem()}><ExternalLink />系统打开</Button><Button variant="ghost" className="fox-local-kb-delete-trigger" onClick={() => setDeleteOpen(true)}><Trash2 />删除</Button></>

  return (
    <section className="fox-local-kb-document-reader">
      {embedded ? <div className="fox-local-kb-embedded-toolbar"><div><Button aria-label="返回文档列表" variant="ghost" size="icon" onClick={onBack}><ArrowLeft /></Button><span><h2 title={document.name}>{document.name}</h2><p>文档预览与索引操作</p></span></div><div>{readerActions}</div></div> : <LocalKnowledgeHeader title={document.name} description="" onBack={onBack} actions={readerActions} />}
      <div className="fox-local-kb-document-reader-meta"><LocalFileTypeIcon extension={document.extension} /><span title={document.relativePath}>{document.relativePath}</span><small>{formatBytes(document.sizeBytes)} · 更新于 {formatDate(document.updatedAt)}</small></div>
      <div className="fox-local-kb-document-reader-body">
        {loading && <div className="fox-file-preview-state is-loading"><LoaderCircle className="animate-spin" /><b>正在准备预览</b></div>}
        {!loading && !error && !unsupported && <Suspense fallback={<div className="fox-file-preview-state is-loading"><LoaderCircle className="animate-spin" /><b>正在加载查看器</b></div>}>{preview}</Suspense>}
        {!loading && (unsupported || error) && <div className="fox-file-preview-state"><FileQuestion /><b>{error ? '预览加载失败' : '暂不支持内置预览'}</b><p>{error ?? '当前文件格式暂不支持在 Fox 中直接预览。'}</p><Button onClick={() => void openWithSystem()}><ExternalLink />用系统应用打开</Button></div>}
      </div>
      <Dialog open={chunksOpen} onOpenChange={setChunksOpen}>
        <DialogContent className="fox-local-kb-chunks-dialog">
          <DialogHeader><DialogTitle>分块预览</DialogTitle><DialogDescription>检查解析后的文本边界、锚点和顺序。当前最多显示 100 个分块。</DialogDescription></DialogHeader>
          {chunksLoading && <LocalKnowledgeLoading />}
          {!chunksLoading && chunks.length === 0 && <LocalKnowledgeEmpty title="暂无分块" description="请先重新解析文档或重建索引。" />}
          {!chunksLoading && chunks.length > 0 && <div className="fox-local-kb-chunk-list">{chunks.map((chunk) => <article key={chunk.id}><header><strong>分块 {chunk.chunkIndex + 1}</strong><span>{chunk.anchor ?? '无锚点'}{chunk.startOffset != null && chunk.endOffset != null ? ` · ${chunk.startOffset}–${chunk.endOffset}` : ''}</span></header><p>{chunk.content}</p></article>)}</div>}
        </DialogContent>
      </Dialog>
      <Dialog open={deleteOpen} onOpenChange={(open) => { if (!actionBusy) setDeleteOpen(open) }}>
        <DialogContent className="fox-local-kb-create-dialog" showCloseButton={!actionBusy}>
          <DialogHeader><DialogTitle>删除文档</DialogTitle><DialogDescription>将删除文档副本、文本分块和当前向量索引记录。此操作无法撤销。</DialogDescription></DialogHeader>
          <div className="fox-local-kb-delete-summary"><Trash2 /><div><strong>{document.name}</strong><span>{document.relativePath}</span></div></div>
          <DialogFooter><Button variant="outline" onClick={() => setDeleteOpen(false)} disabled={actionBusy}>取消</Button><Button variant="destructive" onClick={() => void remove()} disabled={actionBusy}>{actionBusy ? <LoaderCircle className="animate-spin" /> : <Trash2 />}确认删除</Button></DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  )
}

function LocalFileSourceVisual({ aggregate = false }: { aggregate?: boolean }) {
  return <span className="fox-local-files-source-visual" aria-hidden="true">
    <span className="fox-local-files-source-peek">
      <LocalFileTypeIcon extension={aggregate ? 'md' : 'pdf'} />
      <LocalFileTypeIcon extension={aggregate ? 'xlsx' : 'docx'} />
      <LocalFileTypeIcon extension={aggregate ? 'pptx' : 'txt'} />
    </span>
    <span className="fox-local-files-source-folder" />
    <span className="fox-local-files-source-tray" />
  </span>
}

function LocalFileSourceCard({ source, selected, busy, onSelect, onEdit, onRescan, onRemove }: { source: LocalKnowledgeFileSource; selected: boolean; busy: boolean; onSelect: () => void; onEdit: () => void; onRescan: () => void; onRemove: () => void }) {
  const displayPath = displayLocalPath(source.rootPath)
  return (
    <article className={cn('fox-local-files-source', selected && 'is-selected', busy && 'is-busy')} title={displayPath}>
      <button type="button" className="fox-local-files-source-select" aria-pressed={selected} onClick={onSelect} disabled={busy}>
        <LocalFileSourceVisual />
        <span className="fox-local-files-source-main"><strong>{source.displayName}</strong><small>{source.fileCount} 个文件</small></span>
      </button>
      <DropdownMenu>
        <DropdownMenuTrigger asChild><Button variant="ghost" size="icon" className="fox-local-files-source-menu" aria-label={`管理 ${source.displayName}`} disabled={busy}><MoreHorizontal /></Button></DropdownMenuTrigger>
        <DropdownMenuContent align="end" sideOffset={5} className="fox-local-files-source-menu-content">
          <DropdownMenuItem onSelect={onEdit}><Pencil />编辑</DropdownMenuItem>
          <DropdownMenuItem onSelect={onRescan}><RefreshCw />刷新</DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant="destructive" onSelect={onRemove}><Trash2 />删除</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </article>
  )
}

function AllLocalFileSourcesCard({ selected, fileCount, onSelect }: { selected: boolean; fileCount: number; onSelect: () => void }) {
  return <button type="button" className={cn('fox-local-files-source', 'is-all', selected && 'is-selected')} aria-pressed={selected} onClick={onSelect}>
    <LocalFileSourceVisual aggregate />
    <span className="fox-local-files-source-main"><strong>全部文件</strong><small>{fileCount} 个文件</small></span>
  </button>
}

function LocalFileRow({ file, selected, onToggle, onPreview, onImport, onOpen, onReveal }: { file: LocalKnowledgeCatalogFile; selected: boolean; onToggle: () => void; onPreview: () => void; onImport: () => void; onOpen: () => void; onReveal: () => void }) {
  const invoke = (event: ReactMouseEvent, action: () => void) => {
    event.stopPropagation()
    action()
  }
  return (
    <div className={cn('fox-local-files-row', selected && 'is-selected')}>
      <input type="checkbox" checked={selected} onChange={onToggle} aria-label={`选择 ${file.name}`} />
      <button type="button" className="fox-local-files-file-trigger" onClick={onPreview}>
        <LocalFileTypeIcon extension={file.extension} />
        <span className="fox-local-files-file-main"><strong>{file.name}</strong><small title={displayLocalPath(file.absolutePath)}>{file.relativePath}</small></span>
      </button>
      <span className="fox-local-files-file-source"><b>{file.sourceName}</b><small>{file.extension.toUpperCase()}</small></span>
      <span className="fox-local-files-file-meta"><b>{formatBytes(file.sizeBytes)}</b><small>{formatDate(file.modifiedAt)}</small></span>
      <span className="fox-local-files-file-actions">
        <Button variant="ghost" size="icon" title="加入知识库" aria-label={`将 ${file.name} 加入知识库`} onClick={(event) => invoke(event, onImport)}><Database /></Button>
        <Button variant="ghost" size="icon" title="在 Fox 中打开" aria-label={`在 Fox 中打开 ${file.name}`} onClick={(event) => invoke(event, onOpen)}><BookOpen /></Button>
        <Button variant="ghost" size="icon" title="打开所在文件夹" aria-label={`打开 ${file.name} 所在文件夹`} onClick={(event) => invoke(event, onReveal)}><FolderOpen /></Button>
      </span>
    </div>
  )
}

export function LocalKnowledgeFilesPage({ gateway = defaultLocalKnowledgeGateway, className, onNavigate, onOperationAccepted }: LocalKnowledgeNavigationProps & { onOperationAccepted?: (accepted: OperationAccepted) => void }) {
  const [sources, setSources] = useState<LocalKnowledgeFileSource[]>([])
  const [bases, setBases] = useState<LocalKnowledgeBase[]>([])
  const [files, setFiles] = useState<LocalKnowledgeCatalogFile[]>([])
  const [query, setQuery] = useState('')
  const [category, setCategory] = useState<LocalFileCategory>('all')
  const [sourceId, setSourceId] = useState('all')
  const [sortBy, setSortBy] = useState<'modified' | 'name' | 'size'>('modified')
  const [editingSource, setEditingSource] = useState<LocalKnowledgeFileSource | null>(null)
  const [sourceNameDraft, setSourceNameDraft] = useState('')
  const [removingSource, setRemovingSource] = useState<LocalKnowledgeFileSource | null>(null)
  const [sourceDialogError, setSourceDialogError] = useState<string | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [targetBaseId, setTargetBaseId] = useState('')
  const [importOpen, setImportOpen] = useState(false)
  const [previewFile, setPreviewFile] = useState<LocalKnowledgeCatalogFile | null>(null)
  const [loading, setLoading] = useState(true)
  const [loadingMore, setLoadingMore] = useState(false)
  const [hasMore, setHasMore] = useState(false)
  const [picking, setPicking] = useState(false)
  const [importing, setImporting] = useState(false)
  const [busySourceId, setBusySourceId] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const loadMoreRef = useRef<HTMLDivElement | null>(null)
  const requestRevisionRef = useRef(0)

  const load = useCallback(async () => {
    const revision = ++requestRevisionRef.current
    setLoading(true)
    try {
      const [nextSources, nextBases, nextFiles] = await Promise.all([
        gateway.listFileSources(),
        gateway.listKnowledgeBases(),
        gateway.listLocalFiles({ sourceId: sourceId === 'all' ? undefined : sourceId, query: query.trim() || undefined, category, limit: LOCAL_FILE_PAGE_SIZE, offset: 0 }),
      ])
      if (revision !== requestRevisionRef.current) return
      setSources(nextSources)
      setSourceId((current) => current === 'all' || nextSources.some((source) => source.id === current) ? current : 'all')
      setBases(nextBases)
      setFiles(nextFiles)
      setHasMore(nextFiles.length === LOCAL_FILE_PAGE_SIZE)
      setSelected((current) => new Set([...current].filter((id) => nextFiles.some((file) => file.id === id))))
      setTargetBaseId((current) => current && nextBases.some((base) => base.id === current) ? current : nextBases[0]?.id ?? '')
      setError(null)
    } catch (cause) {
      if (revision === requestRevisionRef.current) setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      if (revision === requestRevisionRef.current) setLoading(false)
    }
  }, [category, gateway, query, sourceId])

  useEffect(() => { void load() }, [load])

  const loadMore = useCallback(async () => {
    if (loading || loadingMore || !hasMore) return
    const revision = requestRevisionRef.current
    const offset = files.length
    setLoadingMore(true)
    try {
      const nextFiles = await gateway.listLocalFiles({ sourceId: sourceId === 'all' ? undefined : sourceId, query: query.trim() || undefined, category, limit: LOCAL_FILE_PAGE_SIZE, offset })
      if (revision !== requestRevisionRef.current) return
      setFiles((current) => [...current, ...nextFiles.filter((file) => !current.some((item) => item.id === file.id))])
      setHasMore(nextFiles.length === LOCAL_FILE_PAGE_SIZE)
    } catch (cause) {
      if (revision === requestRevisionRef.current) setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      if (revision === requestRevisionRef.current) setLoadingMore(false)
    }
  }, [category, files.length, gateway, hasMore, loading, loadingMore, query, sourceId])

  useEffect(() => {
    const sentinel = loadMoreRef.current
    if (!sentinel || !hasMore) return
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) void loadMore()
    }, { rootMargin: '120px 0px' })
    observer.observe(sentinel)
    return () => observer.disconnect()
  }, [hasMore, loadMore])

  const addSource = async () => {
    setPicking(true)
    setError(null)
    try {
      const path = await gateway.pickFileSourceFolder()
      if (!path) return
      await gateway.addFileSource(path)
      await load()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setPicking(false)
    }
  }

  const rescanSource = async (sourceId: string) => {
    setBusySourceId(sourceId)
    setError(null)
    try {
      await gateway.rescanFileSource(sourceId)
      await load()
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusySourceId(null)
    }
  }

  const openSourceEditor = (source: LocalKnowledgeFileSource) => {
    setEditingSource(source)
    setSourceNameDraft(source.displayName)
    setSourceDialogError(null)
  }

  const updateSource = async () => {
    if (!editingSource || !sourceNameDraft.trim()) return
    setBusySourceId(editingSource.id)
    setSourceDialogError(null)
    try {
      await gateway.updateFileSource(editingSource.id, sourceNameDraft)
      setEditingSource(null)
      await load()
    } catch (cause) {
      setSourceDialogError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusySourceId(null)
    }
  }

  const removeSource = async () => {
    if (!removingSource) return
    const source = removingSource
    setBusySourceId(source.id)
    setSourceDialogError(null)
    try {
      await gateway.removeFileSource(source.id)
      setRemovingSource(null)
      if (sourceId === source.id) setSourceId('all')
      else await load()
    } catch (cause) {
      setSourceDialogError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setBusySourceId(null)
    }
  }

  const selectedFiles = files.filter((file) => selected.has(file.id))
  const visibleFiles = useMemo(() => [...files].sort((left, right) => {
    if (sortBy === 'name') return left.name.localeCompare(right.name, 'zh-CN')
    if (sortBy === 'size') return right.sizeBytes - left.sizeBytes
    return (right.modifiedAt ?? '').localeCompare(left.modifiedAt ?? '')
  }), [files, sortBy])
  const visibleFilesSelected = visibleFiles.length > 0 && visibleFiles.every((file) => selected.has(file.id))
  const totalFileCount = sourceId === 'all'
    ? sources.reduce((total, source) => total + source.fileCount, 0)
    : sources.find((source) => source.id === sourceId)?.fileCount ?? files.length
  const allSourceFileCount = sources.reduce((total, source) => total + source.fileCount, 0)
  const toggleFile = (fileId: string) => {
    setSelected((current) => {
      const next = new Set(current)
      if (next.has(fileId)) next.delete(fileId)
      else next.add(fileId)
      return next
    })
  }
  const toggleVisibleFiles = () => {
    setSelected((current) => {
      const next = new Set(current)
      if (visibleFilesSelected) visibleFiles.forEach((file) => next.delete(file.id))
      else visibleFiles.forEach((file) => next.add(file.id))
      return next
    })
  }

  const startImport = async () => {
    if (!targetBaseId || selectedFiles.length === 0) return
    setImporting(true)
    setError(null)
    try {
      const accepted = await gateway.startImport({
        knowledgeBaseId: targetBaseId,
        files: selectedFiles.map((file) => ({
          name: file.name,
          sourcePath: file.absolutePath,
          relativePath: file.relativePath,
          mimeType: file.mimeType,
          sizeBytes: file.sizeBytes,
        })),
      })
      recordImportedDocuments(targetBaseId, selectedFiles.map((file) => ({ name: file.name, relativePath: file.relativePath })))
      onOperationAccepted?.(accepted)
      setImportOpen(false)
      setSelected(new Set())
      onNavigate?.('jobs', targetBaseId)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setImporting(false)
    }
  }

  const openImport = () => {
    if (bases.length === 0) {
      setError('请先创建一个本地知识库，再把文件加入知识库。')
      return
    }
    setImportOpen(true)
  }

  const openImportFor = (file: LocalKnowledgeCatalogFile) => {
    if (bases.length === 0) {
      setError('请先创建一个本地知识库，再把文件加入知识库。')
      return
    }
    setSelected(new Set([file.id]))
    setImportOpen(true)
  }

  const revealLocalFile = async (file: LocalKnowledgeCatalogFile) => {
    try {
      await gateway.revealLocalFile(file.id)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    }
  }

  return (
    <div className={cn('fox-local-kb-page', className)}>
      <div className="fox-local-kb-content">
        <header className="fox-local-files-header">
          <div className="fox-local-files-heading"><h1>本地文件</h1><span>{totalFileCount} 个文件</span></div>
          <div className="fox-local-files-header-actions">
            <label className="fox-local-kb-search"><Search /><Input aria-label="搜索本地文件" placeholder="搜索文件名或路径..." value={query} onChange={(event) => setQuery(event.target.value)} /></label>
            <Button onClick={() => void addSource()} disabled={picking}>{picking ? <LoaderCircle className="animate-spin" /> : <FolderPlus />}{picking ? '正在扫描' : '添加文件夹'}</Button>
          </div>
        </header>
        {sources.length > 0 && <div className="fox-local-files-sources" aria-label="本地文件夹">
          <AllLocalFileSourcesCard selected={sourceId === 'all'} fileCount={allSourceFileCount} onSelect={() => setSourceId('all')} />
          {sources.map((source) => <LocalFileSourceCard key={source.id} source={source} selected={sourceId === source.id} busy={busySourceId === source.id} onSelect={() => setSourceId(source.id)} onEdit={() => openSourceEditor(source)} onRescan={() => void rescanSource(source.id)} onRemove={() => { setRemovingSource(source); setSourceDialogError(null) }} />)}
        </div>}
        <div className="fox-local-files-toolbar">
          <Tabs value={category} onValueChange={(value) => setCategory(value as LocalFileCategory)} className="fox-local-files-categories">
            <TabsList aria-label="本地文件类型">{localFileCategories.map((item) => <TabsTrigger key={item.value} value={item.value}>{item.label}</TabsTrigger>)}</TabsList>
          </Tabs>
          <Select value={sortBy} onValueChange={(value) => setSortBy(value as 'modified' | 'name' | 'size')}>
            <SelectTrigger className="fox-local-files-sort-select" aria-label="本地文件排序"><SelectValue /></SelectTrigger>
            <SelectContent><SelectItem value="modified">修改时间</SelectItem><SelectItem value="name">文件名</SelectItem><SelectItem value="size">文件大小</SelectItem></SelectContent>
          </Select>
        </div>
        {selected.size > 0 && <div className="fox-local-files-selection"><span>已选择 {selected.size} 个文件 · {formatBytes(selectedFiles.reduce((total, file) => total + file.sizeBytes, 0))}</span><Button variant="ghost" onClick={() => setSelected(new Set())}>取消选择</Button><Button onClick={openImport}><Database />加入知识库</Button></div>}
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        {!loading && !error && sources.length === 0 && <LocalKnowledgeEmpty title="还没有本地文件来源" description="添加一个文件夹后，Fox 只会登记其中受支持的文档，不会移动原文件。" action={<Button onClick={() => void addSource()}><FolderPlus />添加文件夹</Button>} />}
        {!loading && !error && sources.length > 0 && files.length === 0 && <LocalKnowledgeEmpty title="没有匹配的文件" description="当前目录中没有受支持的文件，或没有匹配当前筛选条件。" />}
        {!loading && !error && files.length > 0 && <div className="fox-local-files-list"><div className="fox-local-files-list-head"><input type="checkbox" checked={visibleFilesSelected} onChange={toggleVisibleFiles} aria-label="选择当前已加载的全部文件" /><span>文件</span><span>来源 / 类型</span><span>大小 / 修改时间</span><span>操作</span></div>{visibleFiles.map((file) => <LocalFileRow key={file.id} file={file} selected={selected.has(file.id)} onToggle={() => toggleFile(file.id)} onPreview={() => setPreviewFile(file)} onImport={() => openImportFor(file)} onOpen={() => setPreviewFile(file)} onReveal={() => void revealLocalFile(file)} />)}<div ref={loadMoreRef} className="fox-local-files-load-more" aria-live="polite">{loadingMore && <><LoaderCircle className="animate-spin" />正在加载更多文件...</>}</div></div>}
      </div>
      <Dialog open={importOpen} onOpenChange={(open) => { if (!importing) setImportOpen(open) }}>
        <DialogContent className="fox-local-files-import-dialog" showCloseButton={!importing}>
          <DialogHeader><DialogTitle>加入本地知识库</DialogTitle><DialogDescription>选中的 {selectedFiles.length} 个文件会复制到目标知识库，原文件保持不变。</DialogDescription></DialogHeader>
          <div className="fox-local-files-base-options">{bases.map((base) => <label key={base.id} className={targetBaseId === base.id ? 'is-selected' : undefined}><input type="radio" name="target-local-knowledge-base" value={base.id} checked={targetBaseId === base.id} onChange={() => setTargetBaseId(base.id)} /><Database /><span><strong>{base.name}</strong><small>{base.documentCount} 个文档 · {base.description || '暂无描述'}</small></span></label>)}</div>
          <DialogFooter><Button variant="outline" onClick={() => setImportOpen(false)} disabled={importing}>取消</Button><Button onClick={() => void startImport()} disabled={importing || !targetBaseId}>{importing ? <LoaderCircle className="animate-spin" /> : <Upload />}{importing ? '正在加入' : '确认加入'}</Button></DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog open={editingSource != null} onOpenChange={(open) => { if (!open && !busySourceId) setEditingSource(null) }}>
        <DialogContent className="fox-local-kb-create-dialog" showCloseButton={!busySourceId}>
          <DialogHeader><DialogTitle>编辑文件夹</DialogTitle><DialogDescription>修改文件夹在 Fox 中显示的名称，不会更改磁盘上的文件夹名称。</DialogDescription></DialogHeader>
          <label className="fox-local-kb-field"><span>显示名称</span><Input value={sourceNameDraft} maxLength={80} autoFocus onChange={(event) => setSourceNameDraft(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); void updateSource() } }} /></label>
          {sourceDialogError && <LocalKnowledgeError message={sourceDialogError} />}
          <DialogFooter><Button variant="outline" onClick={() => setEditingSource(null)} disabled={busySourceId != null}>取消</Button><Button onClick={() => void updateSource()} disabled={busySourceId != null || !sourceNameDraft.trim()}>{busySourceId ? <LoaderCircle className="animate-spin" /> : <Pencil />}保存</Button></DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog open={removingSource != null} onOpenChange={(open) => { if (!open && !busySourceId) setRemovingSource(null) }}>
        <DialogContent className="fox-local-kb-create-dialog" showCloseButton={!busySourceId}>
          <DialogHeader><DialogTitle>删除文件夹来源</DialogTitle><DialogDescription>Fox 只会停止管理这个文件夹并移除目录记录，不会删除原文件。</DialogDescription></DialogHeader>
          {removingSource && <div className="fox-local-kb-delete-summary"><FolderOpen /><div><strong>{removingSource.displayName}</strong><span>{displayLocalPath(removingSource.rootPath)}</span></div></div>}
          {sourceDialogError && <LocalKnowledgeError message={sourceDialogError} />}
          <DialogFooter><Button variant="outline" onClick={() => setRemovingSource(null)} disabled={busySourceId != null}>取消</Button><Button variant="destructive" onClick={() => void removeSource()} disabled={busySourceId != null}>{busySourceId ? <LoaderCircle className="animate-spin" /> : <Trash2 />}确认删除</Button></DialogFooter>
        </DialogContent>
      </Dialog>
      <LocalFilePreviewDialog file={previewFile} gateway={gateway} open={previewFile != null} onOpenChange={(open) => { if (!open) setPreviewFile(null) }} />
    </div>
  )
}

export function LocalKnowledgeHomePage({ gateway = defaultLocalKnowledgeGateway, className, onNavigate }: LocalKnowledgePageProps & { onNavigate?: (view: LocalKnowledgeView, knowledgeBaseId?: string, documentId?: string) => void }) {
  const [bases, setBases] = useState<LocalKnowledgeBase[]>([])
  const [jobs, setJobs] = useState<KnowledgeJob[]>([])
  const [recent] = useState(readRecentDocuments)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = async () => {
    setLoading(true)
    try {
      const [nextBases, nextJobs] = await Promise.all([
        gateway.listKnowledgeBases(),
        gateway.listJobs().catch(() => []),
      ])
      setBases(nextBases)
      setJobs(nextJobs)
      setError(null)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setLoading(false)
    }
  }

  useEffect(() => { void load() }, [gateway])

  const availableBaseIds = useMemo(() => new Set(bases.map((base) => base.id)), [bases])
  const visibleRecent = recent.filter((item) => availableBaseIds.has(item.knowledgeBaseId)).slice(0, RECENT_DOCUMENT_LIMIT)
  const activeJobs = jobs.filter((job) => ['queued', 'running', 'paused'].includes(job.status)).length
  const guide = bases.find((base) => base.name === 'Fox 使用指南')

  return (
    <div className={cn('fox-local-kb-page fox-local-kb-home-page', className)}>
      <div className="fox-local-kb-content fox-local-kb-home-content">
        <section className="fox-local-kb-home-hero">
          <div className="fox-local-kb-home-greeting">
            <img src={LOCAL_KNOWLEDGE_MASCOT} alt="" />
            <div className="fox-local-kb-home-greeting-copy">
              <h1>今天想一起读点什么？</h1>
              <div className="fox-local-kb-home-actions">
                <button type="button" onClick={() => onNavigate?.('files')}>整理本地文件</button>
                <button type="button" onClick={() => onNavigate?.('list')}>本地知识库</button>
                <button type="button" onClick={() => onNavigate?.('jobs')}>任务中心{activeJobs > 0 && <small>{activeJobs}</small>}</button>
                {guide && <button type="button" onClick={() => onNavigate?.('documents', guide.id)}>查看使用指南</button>}
              </div>
            </div>
          </div>
        </section>

        {error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}

        <section className="fox-local-kb-home-recent">
          <div className="fox-local-kb-home-section-head"><div><History /><h2>最近访问</h2></div></div>
          {!loading && visibleRecent.length > 0 && <div className="fox-local-kb-recent-list">
            <div className="fox-local-kb-recent-list-head"><span>文件</span><span>所属知识库</span><span>最近打开</span></div>
            {visibleRecent.map((item) => <button type="button" key={`${item.knowledgeBaseId}:${item.documentId}`} onClick={() => onNavigate?.('documents', item.knowledgeBaseId, item.documentId)}><span><LocalFileTypeIcon extension={item.extension} /><strong>{item.name}</strong></span><span>{item.knowledgeBaseName}</span><span>{formatDate(item.openedAt)}</span></button>)}
          </div>}
          {!loading && visibleRecent.length === 0 && <div className="fox-local-kb-recent-empty"><FileText /><span><strong>最近还没有打开过文件</strong><small>阅读过的文档会出现在这里，最多保留 8 个。</small></span>{guide && <Button variant="outline" onClick={() => onNavigate?.('documents', guide.id)}>打开 Fox 使用指南</Button>}</div>}
          {loading && <div className="fox-local-kb-recent-loading"><LoaderCircle className="animate-spin" />正在整理最近访问...</div>}
        </section>
      </div>
    </div>
  )
}

export function LocalKnowledgeListPage({ gateway = defaultLocalKnowledgeGateway, className, onNavigate, createRequest, onCreateRequestHandled }: LocalKnowledgePageProps & { onNavigate?: (view: LocalKnowledgeView, knowledgeBaseId?: string) => void; createRequest?: number; onCreateRequestHandled?: () => void }) {
  const [items, setItems] = useState<LocalKnowledgeBase[]>([])
  const [query, setQuery] = useState('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [createOpen, setCreateOpen] = useState(false)
  const [storageOpen, setStorageOpen] = useState(false)
  const [creating, setCreating] = useState(false)
  const [createError, setCreateError] = useState<string | null>(null)

  const load = async () => {
    setLoading(true)
    try { setItems(await gateway.listKnowledgeBases()); setError(null) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }

  useEffect(() => { void load() }, [gateway])

  const openCreateDialog = () => {
    setCreateError(null)
    setCreateOpen(true)
  }

  useEffect(() => {
    if (createRequest == null || createRequest <= 0) return
    openCreateDialog()
    onCreateRequestHandled?.()
  }, [createRequest, onCreateRequestHandled])

  const createKnowledgeBase = async (request: LocalKnowledgeBaseCreateRequest) => {
    setCreating(true)
    setCreateError(null)
    try {
      const created = await gateway.createKnowledgeBase(request)
      setCreateOpen(false)
      await load()
      onNavigate?.('detail', created.id)
    } catch (cause) {
      setCreateError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setCreating(false)
    }
  }

  const filteredItems = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase()
    return items.filter((item) => !normalizedQuery || [item.name, item.description, item.storagePath].some((value) => value.toLocaleLowerCase().includes(normalizedQuery)))
  }, [items, query])

  return (
    <div className={cn('fox-local-kb-page', className)}>
      <div className="fox-local-kb-content">
        <LocalKnowledgeHeader title="本地知识库" description="" actions={<><Button variant="outline" onClick={() => setStorageOpen(true)}><HardDrive />存储位置</Button><Button onClick={openCreateDialog}><Database />新建知识库</Button></>} />
        <div className="fox-local-kb-toolbar">
          <label className="fox-local-kb-search"><Search /><Input aria-label="搜索本地知识库" placeholder="搜索知识库..." value={query} onChange={(event) => setQuery(event.target.value)} /></label>
          <span className="fox-local-kb-result-count">{filteredItems.length} 个知识库</span>
          <Button variant="outline" size="sm" onClick={() => void load()} disabled={loading}><RefreshCw className={loading ? 'animate-spin' : undefined} />刷新</Button>
        </div>
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        {!loading && !error && items.length === 0 && <LocalKnowledgeEmpty title="还没有本地知识库" description="创建一个知识库后，就可以导入文件并建立本地索引。" action={<Button onClick={openCreateDialog}>创建知识库</Button>} />}
        {!loading && !error && items.length > 0 && filteredItems.length === 0 && <LocalKnowledgeEmpty title="没有匹配的知识库" description="换一个名称、描述或存储位置关键词试试。" />}
        {!loading && !error && filteredItems.length > 0 && <div className="fox-local-kb-grid fox-shadcn-entity-grid fox-knowledge-grid">{filteredItems.map((base) => <KnowledgeBaseCard key={base.id} base={base} onOpen={() => onNavigate?.('detail', base.id)} />)}</div>}
      </div>
      <StorageLocationDialog gateway={gateway} open={storageOpen} onOpenChange={setStorageOpen} />
      <CreateKnowledgeBaseDialog gateway={gateway} open={createOpen} submitting={creating} error={createError} onOpenChange={(open) => { setCreateOpen(open); if (open) setCreateError(null) }} onSubmit={createKnowledgeBase} />
    </div>
  )
}

export function LocalKnowledgeDetailPage({ gateway = defaultLocalKnowledgeGateway, className, knowledgeBaseId, onBack, onNavigate, onOperationAccepted }: LocalKnowledgeNavigationProps & { onOperationAccepted?: (accepted: OperationAccepted) => void }) {
  const [base, setBase] = useState<LocalKnowledgeBaseDetail | null>(null)
  const [configuredEmbeddingModel, setConfiguredEmbeddingModel] = useState<LocalEmbeddingModel | null>(null)
  const [defaultEmbeddingModel, setDefaultEmbeddingModel] = useState<LocalEmbeddingModel | null>(null)
  const [documents, setDocuments] = useState<LocalKnowledgeDocument[]>([])
  const [jobs, setJobs] = useState<KnowledgeJob[]>([])
  const [localActivities, setLocalActivities] = useState<LocalKnowledgeActivity[]>([])
  const [selectedStage, setSelectedStage] = useState<KnowledgeReadinessStage>('text')
  const [activeSubview, setActiveSubview] = useState<KnowledgeDetailSubview | null>(null)
  const [activeDocumentId, setActiveDocumentId] = useState<string | undefined>()
  const [timelineScrollProgress, setTimelineScrollProgress] = useState(0)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [editOpen, setEditOpen] = useState(false)
  const [deleteOpen, setDeleteOpen] = useState(false)
  const [saving, setSaving] = useState(false)
  const [deleting, setDeleting] = useState(false)
  const [mutationError, setMutationError] = useState<string | null>(null)
  const activityTimelineRef = useRef<HTMLDivElement | null>(null)
  const hadActiveJobsRef = useRef(false)

  const selectDetailTab = (view: KnowledgeDetailTab) => {
    setActiveDocumentId(undefined)
    setActiveSubview(view === 'detail' ? null : view)
  }

  const leaveSubviewOrDetail = () => {
    if (activeSubview) {
      setActiveDocumentId(undefined)
      setActiveSubview(null)
      return
    }
    onBack?.()
  }

  const navigateWithinSubview = (view: LocalKnowledgeView, nextKnowledgeBaseId?: string, documentId?: string) => {
    if (view === 'documents') {
      setActiveSubview('documents')
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
    setLoading(true)
    try {
      const [nextBase, documentPage, nextJobs, embeddingModels] = await Promise.all([
        gateway.getKnowledgeBase(knowledgeBaseId),
        gateway.listDocuments(knowledgeBaseId).catch(() => null),
        gateway.listJobs(knowledgeBaseId).catch(() => null),
        gateway.listEmbeddingModels().catch(() => [] as LocalEmbeddingModel[]),
      ])
      setBase(nextBase)
      setConfiguredEmbeddingModel(nextBase.configuredEmbeddingModelId ? embeddingModels.find((model) => model.id === nextBase.configuredEmbeddingModelId) ?? null : null)
      setDefaultEmbeddingModel(embeddingModels.find((model) => model.isDefault && model.status === 'ready' && model.integrityStatus === 'verified' && model.loadReady) ?? null)
      setDocuments(documentPage?.items ?? [])
      const resolvedJobs = nextJobs ?? nextBase.recentJobs
      hadActiveJobsRef.current = resolvedJobs.some((job) => job.status === 'queued' || job.status === 'running' || job.status === 'paused')
      setJobs(resolvedJobs)
      setLocalActivities(mergeKnowledgeJobStageActivities(knowledgeBaseId, resolvedJobs).filter((item) => item.knowledgeBaseId === knowledgeBaseId))
      setError(null)
    }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }

  useEffect(() => { void load() }, [gateway, knowledgeBaseId])

  useEffect(() => {
    setActiveSubview(null)
    setActiveDocumentId(undefined)
  }, [knowledgeBaseId])

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
        setLocalActivities(mergeKnowledgeJobStageActivities(knowledgeBaseId, nextJobs).filter((item) => item.knowledgeBaseId === knowledgeBaseId))
        const hasActiveJobs = nextJobs.some((job) => job.status === 'queued' || job.status === 'running' || job.status === 'paused')
        const shouldRefreshBase = hasActiveJobs || hadActiveJobsRef.current
        hadActiveJobsRef.current = hasActiveJobs
        if (shouldRefreshBase) {
          const [nextBase, documentPage] = await Promise.all([
            gateway.getKnowledgeBase(knowledgeBaseId),
            gateway.listDocuments(knowledgeBaseId).catch(() => null),
          ])
          if (!disposed) {
            setBase(nextBase)
            if (documentPage) setDocuments(documentPage.items)
          }
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
      const observedJob = operationStateForJob(job, observedState)
      setJobs((current) => current.map((item) => item.id === job.id ? operationStateForJob(item, observedState) : item))
      setLocalActivities(mergeKnowledgeJobStageActivities(knowledgeBaseId, [observedJob]).filter((item) => item.knowledgeBaseId === knowledgeBaseId))
    }))
    return () => { disposed = true; for (const stop of unlisten) stop() }
  }, [activeJobSubscriptionKey, gateway, knowledgeBaseId])

  const activityEvents = useMemo<KnowledgeActivityEvent[]>(() => {
    const recordedEvents = localActivities.map((activity): KnowledgeActivityEvent => ({
      id: `local-${activity.id}`,
      title: activity.title,
      detail: activity.detail,
      occurredAt: activity.occurredAt,
      progress: activity.progress ?? 100,
      status: activity.status ?? 'complete',
    }))
    return recordedEvents
      .sort((left, right) => right.occurredAt.localeCompare(left.occurredAt))
      .slice(0, LOCAL_KNOWLEDGE_ACTIVITY_DISPLAY_LIMIT)
      .sort((left, right) => left.occurredAt.localeCompare(right.occurredAt))
  }, [localActivities])

  const updateTimelineScrollProgress = useCallback(() => {
    const timeline = activityTimelineRef.current
    if (!timeline) return
    const available = timeline.scrollWidth - timeline.clientWidth
    const nextProgress = available <= 0 ? 100 : Math.round((timeline.scrollLeft / available) * 100)
    setTimelineScrollProgress((current) => current === nextProgress ? current : nextProgress)
  }, [])

  const scrollActivityTimeline = (offset: number) => {
    activityTimelineRef.current?.scrollBy({ left: offset, behavior: 'smooth' })
  }

  const seekActivityTimeline = (progress: number) => {
    const timeline = activityTimelineRef.current
    if (!timeline) return
    const available = timeline.scrollWidth - timeline.clientWidth
    timeline.scrollLeft = available * (progress / 100)
    setTimelineScrollProgress(progress)
  }

  useEffect(() => {
    const frame = window.requestAnimationFrame(() => {
      const timeline = activityTimelineRef.current
      if (!timeline) return
      timeline.scrollLeft = timeline.scrollWidth
      updateTimelineScrollProgress()
    })
    return () => window.cancelAnimationFrame(frame)
  }, [activityEvents.length, updateTimelineScrollProgress])

  const latestJob = jobs[0] ?? base?.recentJobs[0] ?? null
  const configuredEmbeddingModelReady = configuredEmbeddingModel?.status === 'ready'
    && configuredEmbeddingModel.integrityStatus === 'verified'
    && configuredEmbeddingModel.loadReady
    ? configuredEmbeddingModel
    : null
  const configuredBindingUnavailable = Boolean(base?.configuredEmbeddingModelId && !base.embeddingModel && !configuredEmbeddingModelReady)
  const effectiveEmbeddingModel = base?.embeddingModel
    ?? configuredEmbeddingModelReady
    ?? (!base?.configuredEmbeddingModelId ? defaultEmbeddingModel : null)
  const displayedEmbeddingModel = base?.embeddingModel
    ?? configuredEmbeddingModel
    ?? (!base?.configuredEmbeddingModelId ? defaultEmbeddingModel : null)
  const indexedDocuments = documents.filter((document) => document.status === 'indexed').length
  const processingDocuments = documents.filter((document) => document.status === 'queued' || document.status === 'parsing').length
  const latestDocumentUpdate = documents.reduce<string | null>((latest, document) => !latest || document.updatedAt > latest ? document.updatedAt : latest, null)
  const stageDetail = base ? selectedStage === 'documents' ? {
    tone: processingDocuments > 0 ? 'is-active' : 'is-success',
    icon: processingDocuments > 0 ? <LoaderCircle className="animate-spin" /> : <CheckCircle2 />,
    title: processingDocuments > 0 ? '文档正在处理中' : '文档已同步',
    description: processingDocuments > 0 ? '新导入的文档正在解析，状态会随任务进度自动更新。' : '文档已写入本地知识库，可按文件夹浏览和管理。',
    metrics: [{ label: '已索引', value: `${indexedDocuments}` }, { label: '处理中', value: `${processingDocuments}` }, { label: '最后更新', value: formatDate(latestDocumentUpdate) }],
    action: <Button variant="outline" size="sm" onClick={() => navigateWithinSubview('documents', base.id)}>查看文档</Button>,
  } : selectedStage === 'chunks' ? {
    tone: (base.chunkCount ?? 0) > 0 ? 'is-success' : 'is-warning',
    icon: (base.chunkCount ?? 0) > 0 ? <CheckCircle2 /> : <AlertCircle />,
    title: (base.chunkCount ?? 0) > 0 ? '文本分块已生成' : '文本分块待生成',
    description: '解析后的文档按当前分块策略拆分，供文本索引与向量索引使用。',
    metrics: [{ label: '分块总数', value: countLabel(base.chunkCount) }, { label: '分块大小', value: `${base.chunkSize}` }, { label: '重叠长度', value: `${base.chunkOverlap}` }],
    action: base.id !== FOX_GUIDE_KNOWLEDGE_BASE_ID ? <Button variant="outline" size="sm" onClick={() => { setMutationError(null); setEditOpen(true) }}>编辑配置</Button> : <Button variant="outline" size="sm" onClick={() => navigateWithinSubview('documents', base.id)}>查看文档</Button>,
  } : selectedStage === 'text' ? {
    tone: base.textIndexReady ? 'is-success' : 'is-warning',
    icon: base.textIndexReady ? <CheckCircle2 /> : <AlertCircle />,
    title: base.textIndexReady ? '文本索引已就绪' : '文本索引待构建',
    description: base.textIndexReady ? '全文倒排索引已完成，支持高质量关键词检索。' : '完成文本索引后即可使用关键词检索。',
    metrics: [{ label: '索引方式', value: '全文倒排索引' }, { label: '索引条目', value: countLabel(base.chunkCount) }, { label: '最后更新', value: formatDate(latestJob?.updatedAt) }],
    action: <Button variant="outline" size="sm" onClick={() => navigateWithinSubview('retrieval', base.id)}><Search />检索测试</Button>,
  } : {
    tone: base.vectorIndexReady ? 'is-success' : configuredBindingUnavailable ? 'is-warning' : 'is-info',
    icon: base.vectorIndexReady ? <CheckCircle2 /> : configuredBindingUnavailable ? <AlertCircle /> : <Info />,
    title: base.vectorIndexReady ? '向量索引已就绪' : configuredBindingUnavailable ? '已绑定的向量模型不可用' : effectiveEmbeddingModel ? '向量索引待构建' : '向量索引待配置',
    description: base.vectorIndexReady
      ? '语义与混合检索已启用。'
      : configuredBindingUnavailable
        ? '当前绑定模型未安装完整或没有通过隔离测试，请前往向量模型页修复或重新测试。'
      : base.configuredEmbeddingModelId
        ? '模型已经绑定，完成一次向量索引构建后即可使用语义与混合检索。'
        : effectiveEmbeddingModel
          ? `将自动采用默认模型“${effectiveEmbeddingModel.name}”，构建完成后会绑定到当前知识库。`
          : (base.fallbackReason ?? '请先安装向量模型并设为默认。'),
    metrics: [{ label: '向量模型', value: displayedEmbeddingModel ? `${displayedEmbeddingModel.name}${configuredBindingUnavailable ? '（不可用）' : base.configuredEmbeddingModelId ? '' : '（默认）'}` : (base.configuredEmbeddingModelId ?? '未配置') }, { label: '向量数量', value: countLabel(base.vectorCount) }, { label: '索引代次', value: base.vectorIndexReady ? generationLabel(base) : '—' }],
    action: base.vectorIndexReady
      ? <Button variant="outline" size="sm" onClick={() => onNavigate?.('models', base.id)}>管理模型</Button>
      : configuredBindingUnavailable
        ? <Button variant="outline" size="sm" onClick={() => onNavigate?.('models', base.id)}>修复并测试模型</Button>
      : effectiveEmbeddingModel
        ? <Button variant="outline" size="sm" onClick={() => void rebuildIndex()} disabled={base.documentCount === 0}><RefreshCw />{base.documentCount === 0 ? '请先导入文档' : '构建向量索引'}</Button>
        : <Button variant="outline" size="sm" onClick={() => onNavigate?.('models', base.id)}>安装并设置默认模型</Button>,
  } : null

  const updateKnowledgeBase = async (request: LocalKnowledgeBaseCreateRequest) => {
    if (!knowledgeBaseId) return
    setSaving(true)
    setMutationError(null)
    try {
      await gateway.updateKnowledgeBase(knowledgeBaseId, request)
      setEditOpen(false)
      await load()
    } catch (cause) {
      setMutationError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSaving(false)
    }
  }

  const deleteKnowledgeBase = async () => {
    if (!knowledgeBaseId) return
    setDeleting(true)
    setMutationError(null)
    try {
      await gateway.deleteKnowledgeBase(knowledgeBaseId)
      setDeleteOpen(false)
      onNavigate?.('list')
    } catch (cause) {
      setMutationError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setDeleting(false)
    }
  }

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

  if (!knowledgeBaseId) return <div className={cn('fox-local-kb-page', className)}><div className="fox-local-kb-content"><LocalKnowledgeEmpty title="没有选择知识库" description="请从本地知识库列表进入一个知识库。" action={<Button onClick={onBack}>返回列表</Button>} /></div></div>

  return (
    <div className={cn('fox-local-kb-page fox-local-kb-detail-page', className)}>
      <div className="fox-local-kb-content fox-local-kb-detail-content">
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        {!loading && base && <>
          <header className="fox-local-kb-detail-header">
            <div className="fox-local-kb-detail-heading">
              <Button aria-label={activeSubview ? '返回知识库详情主页' : '返回本地知识库'} className="fox-local-kb-back" variant="ghost" size="icon" onClick={leaveSubviewOrDetail}><ArrowLeft /></Button>
              <h1 title={base.name}>{base.name}</h1>
              <KnowledgeStatusBadge status={base.status} />
              <span className="fox-local-kb-detail-path" title={base.storagePath}><FolderOpen />{base.storagePath}</span>
            </div>
            <div className="fox-local-kb-detail-actions">
              <Tabs value={activeSubview ?? 'detail'} onValueChange={(value) => selectDetailTab(value as KnowledgeDetailTab)} className="fox-local-kb-detail-switcher">
                <TabsList aria-label="切换知识库详情视图">
                  <TabsTrigger value="detail">详情</TabsTrigger>
                  <TabsTrigger value="documents">文档管理</TabsTrigger>
                  <TabsTrigger value="retrieval">检索测试</TabsTrigger>
                  <TabsTrigger value="jobs">任务状态</TabsTrigger>
                </TabsList>
              </Tabs>
              {base.id !== FOX_GUIDE_KNOWLEDGE_BASE_ID && <>
                <Button variant="outline" onClick={() => { setMutationError(null); setEditOpen(true) }}><Pencil />编辑</Button>
                <Button variant="outline" className="fox-local-kb-delete-trigger" onClick={() => { setMutationError(null); setDeleteOpen(true) }}><Trash2 />删除</Button>
              </>}
            </div>
          </header>
          {mutationError && <p className="fox-local-kb-create-error" role="alert"><AlertCircle />{mutationError}</p>}

          {activeSubview ? <div className="fox-local-kb-detail-inline-view" key={activeSubview}>
            {activeSubview === 'documents' && <LocalKnowledgeDocumentsPage embedded gateway={gateway} knowledgeBaseId={base.id} documentId={activeDocumentId} onBack={() => { if (activeDocumentId) setActiveDocumentId(undefined); else setActiveSubview(null) }} onNavigate={navigateWithinSubview} onOperationAccepted={onOperationAccepted} />}
            {activeSubview === 'retrieval' && <LocalKnowledgeRetrievalPage embedded gateway={gateway} knowledgeBaseId={base.id} onBack={() => setActiveSubview(null)} onNavigate={navigateWithinSubview} />}
            {activeSubview === 'jobs' && <LocalKnowledgeJobsPage embedded gateway={gateway} knowledgeBaseId={base.id} onBack={() => setActiveSubview(null)} onNavigate={navigateWithinSubview} />}
          </div> : <>
          <section className="fox-local-kb-readiness" aria-labelledby="knowledge-readiness-title">
            <h2 className="fox-sr-only" id="knowledge-readiness-title">知识库索引就绪状态</h2>
            <ol className="fox-local-kb-readiness-flow">
              <li className={cn(base.documentCount > 0 && 'is-ready', selectedStage === 'documents' && 'is-selected')}><button type="button" aria-pressed={selectedStage === 'documents'} onClick={() => setSelectedStage('documents')}><span><FileText />{base.documentCount > 0 && <CheckCircle2 />}</span><p><strong>文档</strong><small>{base.documentCount} 个文档</small></p></button></li>
              <li className={cn((base.chunkCount ?? 0) > 0 && 'is-ready', selectedStage === 'chunks' && 'is-selected')}><button type="button" aria-pressed={selectedStage === 'chunks'} onClick={() => setSelectedStage('chunks')}><span><Layers3 />{(base.chunkCount ?? 0) > 0 && <CheckCircle2 />}</span><p><strong>文本分块</strong><small>{countLabel(base.chunkCount)} 个分块</small></p></button></li>
              <li className={cn(base.textIndexReady && 'is-ready', selectedStage === 'text' && 'is-selected')}><button type="button" aria-pressed={selectedStage === 'text'} onClick={() => setSelectedStage('text')}><span><Search />{base.textIndexReady && <CheckCircle2 />}</span><p><strong>文本索引</strong><small>{base.textIndexReady ? '已就绪' : '待构建'}</small></p></button></li>
              <li className={cn(base.vectorIndexReady && 'is-ready', selectedStage === 'vector' && 'is-selected')}><button type="button" aria-pressed={selectedStage === 'vector'} onClick={() => setSelectedStage('vector')}><span><PackageOpen />{base.vectorIndexReady && <CheckCircle2 />}</span><p><strong>向量索引</strong><small>{base.vectorIndexReady ? '已就绪' : configuredBindingUnavailable ? '模型异常' : effectiveEmbeddingModel ? '待构建' : '待配置'}</small></p></button></li>
            </ol>
            {stageDetail && <div className={cn('fox-local-kb-stage-detail', stageDetail.tone)}><span>{stageDetail.icon}</span><p><strong>{stageDetail.title}</strong><small>{stageDetail.description}</small></p><dl>{stageDetail.metrics.map((metric) => <div key={metric.label}><dt>{metric.label}</dt><dd title={metric.value}>{metric.value}</dd></div>)}</dl>{stageDetail.action}</div>}
          </section>

          <div className="fox-local-kb-detail-columns">
            <section className="fox-local-kb-section fox-local-kb-activity">
              <div className="fox-local-kb-section-head"><div><h2>运行与活动</h2><p>最近任务、操作历史与文档更新。</p></div><Button variant="ghost" size="sm" onClick={() => navigateWithinSubview('jobs', base.id)}>查看任务<ArrowRight /></Button></div>
              {latestJob ? <>
                <div className="fox-local-kb-activity-job">
                  <span>{jobStatusIcon(latestJob.status)}</span>
                  <div><strong>最新任务：{formatJobStage(latestJob.stage)}</strong><small>{jobStatusLabel(latestJob.status, latestJob.outcome)} · {latestJob.progress}%</small></div>
                  <time dateTime={latestJob.updatedAt}>{formatDate(latestJob.updatedAt)}</time>
                  <div className="fox-local-kb-activity-progress" aria-label={`任务进度 ${latestJob.progress}%`}><span style={{ width: `${latestJob.progress}%` }} /></div>
                </div>
              </> : <LocalKnowledgeEmpty title="还没有运行记录" description="导入文档或重建索引后，任务进度会显示在这里。" />}

              <div className="fox-local-kb-operation-history">
                <div className="fox-local-kb-operation-history-head"><h3>操作历史</h3><span>最多显示最近 10 条，任务处理中会自动更新</span></div>
                {activityEvents.length > 0 ? <>
                  <div className="fox-local-kb-operation-history-body">
                    <Button type="button" variant="outline" size="icon" aria-label="查看更早操作" onClick={() => scrollActivityTimeline(-320)}><ArrowLeft /></Button>
                    <div ref={activityTimelineRef} className="fox-local-kb-operation-history-viewport" tabIndex={0} onScroll={updateTimelineScrollProgress} aria-label="知识库操作历史时间线">
                      <ol className="fox-local-kb-operation-history-track" style={{ '--fox-local-activity-events': Math.max(activityEvents.length, 1) } as CSSProperties}>{activityEvents.map((event, index) => <li className={cn(index % 2 === 0 ? 'is-above' : 'is-below', `is-${event.status}`)} key={event.id}><article><time dateTime={event.occurredAt}>{formatTime(event.occurredAt)}</time><strong>{event.title}</strong><small title={event.detail}>{event.detail}</small></article><span>{event.status === 'active' ? <LoaderCircle className="animate-spin" /> : event.status === 'warning' ? <AlertCircle /> : <CheckCircle2 />}</span></li>)}</ol>
                    </div>
                    <Button type="button" variant="outline" size="icon" aria-label="查看更新操作" onClick={() => scrollActivityTimeline(320)}><ArrowRight /></Button>
                  </div>
                  <input className="fox-local-kb-operation-history-range" type="range" min="0" max="100" value={timelineScrollProgress} onInput={(event) => seekActivityTimeline(Number(event.currentTarget.value))} aria-label="操作历史滚动位置" />
                </> : <p className="fox-local-kb-operation-history-empty">导入、删除、解析、分块、文本索引与向量化任务会按时间显示在这里。</p>}
              </div>

              <div className="fox-local-kb-recent-documents">
                <div><h3>最近文档</h3><Button variant="ghost" size="sm" onClick={() => navigateWithinSubview('documents', base.id)}>查看全部 {base.documentCount} 个文档<ArrowRight /></Button></div>
                {documents.length > 0 ? <ul>{documents.slice(0, 3).map((document) => <li key={document.id}><button type="button" onClick={() => navigateWithinSubview('documents', base.id, document.id)}><LocalFileTypeIcon extension={document.extension} /><span title={document.name}>{document.name}</span><small>{formatBytes(document.sizeBytes)}</small><time dateTime={document.updatedAt}>{formatTime(document.updatedAt)}</time></button></li>)}</ul> : <p>还没有文档，导入后会显示最近更新。</p>}
              </div>
            </section>

            <aside className="fox-local-kb-section fox-local-kb-current-config">
              <div className="fox-local-kb-section-head"><div><h2>当前配置</h2><p>索引能力与本地运行信息。</p></div>{effectiveEmbeddingModel && base.documentCount > 0 && <Button variant="ghost" size="sm" onClick={() => void rebuildIndex()}><RefreshCw />{base.vectorIndexReady ? '重建索引' : '构建索引'}</Button>}</div>
              <dl className="fox-local-kb-config-list">
                <div><dt><Cpu />向量模型</dt><dd>{displayedEmbeddingModel ? `${displayedEmbeddingModel.name} · ${displayedEmbeddingModel.dimension} 维${configuredBindingUnavailable ? '（不可用，需重新测试）' : base.configuredEmbeddingModelId ? '' : '（默认，构建时应用）'}` : (base.configuredEmbeddingModelId ?? '未配置')}{!effectiveEmbeddingModel && <Button variant="ghost" size="sm" onClick={() => onNavigate?.('models', base.id)}>{configuredBindingUnavailable ? '修复模型' : '安装模型'}</Button>}</dd></div>
                <div><dt><Layers3 />分块大小</dt><dd>{base.chunkSize}</dd></div>
                <div><dt><FileStack />重叠长度</dt><dd>{base.chunkOverlap}</dd></div>
                <div><dt><Search />检索模式</dt><dd>{searchModeLabel(base)}</dd></div>
              </dl>
              <dl className="fox-local-kb-runtime-list">
                <div><dt><Database />解析器版本</dt><dd>{base.parserVersion ?? '暂无统计'}</dd></div>
                <div><dt><HardDrive />可用空间</dt><dd>{base.storage.freeBytes == null ? '暂无统计' : formatBytes(base.storage.freeBytes)}</dd></div>
                <div><dt><ShieldCheck />本地权限</dt><dd>{base.writable ? '可编辑' : '只读'}</dd></div>
              </dl>
            </aside>
          </div>
          </>}
        </>}
      </div>
      {base && base.id !== FOX_GUIDE_KNOWLEDGE_BASE_ID && <><EditKnowledgeBaseDialog gateway={gateway} base={base} open={editOpen} submitting={saving} error={editOpen ? mutationError : null} onOpenChange={(open) => { setEditOpen(open); if (open) setMutationError(null) }} onSubmit={updateKnowledgeBase} /><DeleteKnowledgeBaseDialog base={base} open={deleteOpen} submitting={deleting} error={deleteOpen ? mutationError : null} onOpenChange={(open) => { setDeleteOpen(open); if (open) setMutationError(null) }} onConfirm={deleteKnowledgeBase} /></>}
    </div>
  )
}

export function LocalKnowledgeDocumentsPage({ gateway = defaultLocalKnowledgeGateway, className, knowledgeBaseId, documentId, onBack, onNavigate, embedded = false, initialImportOpen = false, onOperationAccepted }: EmbeddedLocalKnowledgeNavigationProps & { initialImportOpen?: boolean; onOperationAccepted?: (accepted: OperationAccepted) => void }) {
  const [base, setBase] = useState<LocalKnowledgeBase | null>(null)
  const [documents, setDocuments] = useState<LocalKnowledgeDocument[]>([])
  const [folders, setFolders] = useState<LocalKnowledgeFolder[]>([])
  const [currentFolder, setCurrentFolder] = useState('')
  const [createOpen, setCreateOpen] = useState(false)
  const [folderName, setFolderName] = useState('')
  const [creating, setCreating] = useState(false)
  const [folderError, setFolderError] = useState<string | null>(null)
  const [importOpen, setImportOpen] = useState(false)
  const [importFiles, setImportFiles] = useState<LocalKnowledgeImportFile[]>([])
  const [targetFolder, setTargetFolder] = useState('')
  const [picking, setPicking] = useState(false)
  const [submitting, setSubmitting] = useState(false)
  const [importError, setImportError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const load = async () => {
    if (!knowledgeBaseId) return
    setLoading(true)
    try {
      const [baseDetail, page, folderItems] = await Promise.all([gateway.getKnowledgeBase(knowledgeBaseId), gateway.listDocuments(knowledgeBaseId), gateway.listFolders(knowledgeBaseId)])
      setBase(baseDetail); setDocuments(page.items); setFolders(folderItems); setError(null)
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLoading(false) }
  }

  useEffect(() => { void load() }, [gateway, knowledgeBaseId])
  useEffect(() => {
    if (!initialImportOpen || !knowledgeBaseId) return
    setTargetFolder('')
    setImportFiles([])
    setImportError(null)
    setImportOpen(true)
  }, [initialImportOpen, knowledgeBaseId])
  const selectedDocument = documentId ? documents.find((document) => document.id === documentId) ?? null : null

  useEffect(() => {
    if (base && selectedDocument) rememberRecentDocument(base, selectedDocument)
  }, [base, selectedDocument])

  const parentPath = (value: string) => {
    const normalized = value.replace(/\\/g, '/')
    const index = normalized.lastIndexOf('/')
    return index < 0 ? '' : normalized.slice(0, index)
  }
  const childFolders = folders.filter((folder) => parentPath(folder.relativePath) === currentFolder)
  const childDocuments = documents.filter((document) => parentPath(document.relativePath) === currentFolder)
  const breadcrumbs = currentFolder ? currentFolder.split('/').filter(Boolean) : []

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

  const openImportDialog = () => {
    setTargetFolder(currentFolder)
    setImportFiles([])
    setImportError(null)
    setImportOpen(true)
  }

  const closeImportDialog = () => {
    if (submitting) return
    setImportOpen(false)
    setImportFiles([])
    setImportError(null)
  }

  const chooseImportFiles = (event: ChangeEvent<HTMLInputElement>) => {
    const selected = Array.from(event.target.files ?? []).map((file) => ({
      name: file.name,
      sourcePath: (file as File & { path?: string }).path,
      relativePath: file.name,
      mimeType: file.type,
      sizeBytes: file.size,
    }))
    setImportFiles(selected)
    setImportError(null)
    event.target.value = ''
  }

  const chooseHostImportFiles = async () => {
    if (!gateway.pickImportFiles) return
    setPicking(true)
    try {
      setImportFiles(await gateway.pickImportFiles())
      setImportError(null)
    } catch (cause) {
      setImportError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setPicking(false)
    }
  }

  const chooseHostImportFolder = async () => {
    if (!gateway.pickImportFolder) return
    setPicking(true)
    try {
      const selected = await gateway.pickImportFolder()
      setImportFiles(selected)
      setImportError(selected.length ? null : '所选文件夹中没有支持导入的文件。')
    } catch (cause) {
      setImportError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setPicking(false)
    }
  }

  const startImportFromDialog = async () => {
    if (!knowledgeBaseId || importFiles.length === 0) return
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
      setImportFiles([])
      onNavigate?.('jobs', knowledgeBaseId)
    } catch (cause) {
      setImportError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setSubmitting(false)
    }
  }

  const browserActions = <><Button variant="outline" size="sm" onClick={() => { setFolderError(null); setCreateOpen(true) }} disabled={!knowledgeBaseId}><FolderPlus />新建文件夹</Button><Button size="sm" onClick={openImportDialog} disabled={!knowledgeBaseId}><Upload />导入文件或文件夹</Button></>

  return (
    <div className={cn('fox-local-kb-page', embedded && 'fox-local-kb-embedded-page', className)}>
      <div className={cn('fox-local-kb-content fox-local-kb-reader-page', embedded && 'fox-local-kb-embedded-content')}>
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        {!loading && !error && !selectedDocument && <section className="fox-local-kb-browser">
          {!embedded && <LocalKnowledgeHeader title={base?.name ?? '本地知识库'} description={currentFolder ? `当前文件夹：${currentFolder}` : '按文件夹整理和浏览知识库文档。'} onBack={onBack} actions={browserActions} />}
          <div className="fox-local-kb-browser-toolbar">
            <nav className="fox-local-kb-breadcrumbs" aria-label="文件夹路径"><button type="button" className={currentFolder ? '' : 'is-current'} onClick={() => setCurrentFolder('')}><Database />根目录</button>{breadcrumbs.map((part, index) => { const path = breadcrumbs.slice(0, index + 1).join('/'); return <span key={path}><ArrowRight /><button type="button" className={path === currentFolder ? 'is-current' : ''} onClick={() => setCurrentFolder(path)}>{part}</button></span> })}</nav>
            {embedded && <div className="fox-local-kb-browser-actions">{browserActions}</div>}
          </div>
          {childFolders.length === 0 && childDocuments.length === 0 ? <LocalKnowledgeEmpty title={folders.length || documents.length ? '这个文件夹是空的' : '还没有文档'} description={folders.length || documents.length ? '可以在此创建子文件夹，或导入文件和整个文件夹。' : '选择本地文件或文件夹导入，Fox 会保留目录结构并在后台建立索引。'} action={<Button onClick={openImportDialog}>开始导入</Button>} /> : <div className="fox-local-kb-browser-list">
            <div className="fox-local-kb-browser-list-head" role="row"><span aria-hidden="true" /><span>名称</span><span>类型</span><span>大小 / 内容</span><span>状态</span><span aria-hidden="true" /></div>
            {childFolders.map((folder) => <button type="button" className="fox-local-kb-folder-row" key={folder.relativePath} onClick={() => setCurrentFolder(folder.relativePath)}><span><FolderOpen /></span><strong title={folder.name}>{folder.name}</strong><small>文件夹</small><small>{folder.documentCount} 个文档</small><span className="fox-local-kb-browser-status">—</span><ArrowRight /></button>)}
            {childDocuments.map((document) => <button type="button" className="fox-local-kb-document-row" key={document.id} onClick={() => onNavigate?.('documents', knowledgeBaseId, document.id)}><LocalFileTypeIcon extension={document.extension} /><strong title={document.name}>{document.name}</strong><small>{document.extension?.toUpperCase() || '文件'}</small><small>{formatBytes(document.sizeBytes)} · {document.chunkCount} 个分块</small><Badge variant="outline">{document.status === 'indexed' ? '已索引' : document.status === 'error' ? '有错误' : '处理中'}</Badge><ArrowRight /></button>)}
          </div>}
        </section>}
        {!loading && !error && selectedDocument && knowledgeBaseId && <LocalKnowledgeDocumentReader embedded={embedded} knowledgeBaseId={knowledgeBaseId} document={selectedDocument} gateway={gateway} onBack={() => onNavigate?.('documents', knowledgeBaseId)} onDeleted={load} onJobAccepted={() => onNavigate?.('jobs', knowledgeBaseId)} />}
      </div>
      <Dialog open={createOpen} onOpenChange={(open) => { setCreateOpen(open); if (!open) { setFolderName(''); setFolderError(null) } }}>
        <DialogContent className="fox-local-kb-folder-dialog"><DialogHeader><DialogTitle>新建文件夹</DialogTitle><DialogDescription>{currentFolder ? `在「${currentFolder}」中创建子文件夹。` : '在知识库根目录创建文件夹。'}</DialogDescription></DialogHeader><label className="fox-local-kb-folder-field"><span>文件夹名称</span><Input autoFocus value={folderName} onChange={(event) => setFolderName(event.target.value)} placeholder="例如：产品资料" onKeyDown={(event) => { if (event.key === 'Enter') void createFolder() }} /></label>{folderError && <p className="fox-local-kb-form-error" role="alert"><AlertCircle />{folderError}</p>}<DialogFooter><Button variant="outline" onClick={() => setCreateOpen(false)} disabled={creating}>取消</Button><Button onClick={() => void createFolder()} disabled={creating || !folderName.trim()}>{creating ? <LoaderCircle className="animate-spin" /> : <FolderPlus />}{creating ? '正在创建' : '创建文件夹'}</Button></DialogFooter></DialogContent>
      </Dialog>
      <Dialog open={importOpen} onOpenChange={(open) => { if (open) setImportOpen(true); else closeImportDialog() }}>
        <DialogContent className="fox-local-kb-import-dialog" showCloseButton={!submitting}>
          <DialogHeader><DialogTitle>导入文档</DialogTitle><DialogDescription>{base ? `导入到「${base.name}」，文件只会写入本地知识库。` : '选择要加入本地知识库的文件。'}</DialogDescription></DialogHeader>
          <div className="fox-local-kb-import-dialog-body">
            <div className="fox-local-kb-import-dropzone"><Upload /><strong>选择要导入的文件或文件夹</strong><small>支持 PDF、DOCX、PPTX、XLSX、Markdown、TXT；导入文件夹时会保留原有目录结构。</small><div className="fox-local-kb-import-pickers">{gateway.pickImportFiles ? <Button type="button" variant="outline" onClick={() => void chooseHostImportFiles()} disabled={picking}>{picking ? <LoaderCircle className="animate-spin" /> : <FileText />}选择文件</Button> : <label className="fox-local-kb-file-picker"><span>选择文件</span><input type="file" multiple onChange={chooseImportFiles} /></label>}{gateway.pickImportFolder && <Button type="button" variant="outline" onClick={() => void chooseHostImportFolder()} disabled={picking}>{picking ? <LoaderCircle className="animate-spin" /> : <FolderOpen />}选择文件夹</Button>}</div></div>
            <label className="fox-local-kb-import-target"><span>导入到</span><Select value={targetFolder || '__root__'} onValueChange={(value) => setTargetFolder(value === '__root__' ? '' : value)}><SelectTrigger><SelectValue /></SelectTrigger><SelectContent><SelectItem value="__root__">根目录</SelectItem>{folders.map((folder) => <SelectItem key={folder.relativePath} value={folder.relativePath}>{folder.relativePath}</SelectItem>)}</SelectContent></Select></label>
            {importFiles.length > 0 && <div className="fox-local-kb-selected-files"><div className="fox-local-kb-section-head"><div><h2>待导入文件</h2><p>{importFiles.length} 个文件 · {formatBytes(importFiles.reduce((total, file) => total + file.sizeBytes, 0))}</p></div><Button variant="ghost" size="sm" onClick={() => setImportFiles([])}>清空</Button></div><div className="fox-local-kb-selected-file-list">{importFiles.map((file) => <div className="fox-local-kb-selected-file" key={`${file.name}-${file.sourcePath ?? file.sizeBytes}`}><FileText /><span title={file.relativePath ?? file.name}>{file.relativePath ?? file.name}</span><small>{formatBytes(file.sizeBytes)}</small></div>)}</div></div>}
            {importError && <p className="fox-local-kb-form-error" role="alert"><AlertCircle />{importError}</p>}
          </div>
          <DialogFooter><Button variant="outline" onClick={closeImportDialog} disabled={submitting}>取消</Button><Button onClick={() => void startImportFromDialog()} disabled={!knowledgeBaseId || importFiles.length === 0 || submitting}>{submitting ? <LoaderCircle className="animate-spin" /> : <Upload />}{submitting ? '正在导入' : '开始导入'}</Button></DialogFooter>
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
        <LocalKnowledgeHeader title="导入本地文档" description={base ? `导入到「${base.name}」，文件只会写入本地知识库。` : '选择要加入本地知识库的文件。'} onBack={onBack} />
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
    { label: '解析', stages: ['parsing'] },
    { label: '提交', stages: ['committing'] },
  ],
  parse: [
    { label: '校验', stages: ['validating'] },
    { label: '解析', stages: ['parsing'] },
    { label: '分块', stages: ['chunking'] },
    { label: '提交', stages: ['committing'] },
  ],
  index: [
    { label: '校验', stages: ['validating'] },
    { label: '向量化', stages: ['embedding'] },
    { label: '写入索引', stages: ['vector_upsert'] },
    { label: '索引校验', stages: ['verifying'] },
    { label: '提交', stages: ['committing'] },
  ],
  rebuild: [
    { label: '校验', stages: ['validating'] },
    { label: '向量化', stages: ['embedding'] },
    { label: '写入索引', stages: ['vector_upsert'] },
    { label: '索引校验', stages: ['verifying'] },
    { label: '提交', stages: ['committing'] },
  ],
  delete: [
    { label: '校验', stages: ['validating'] },
    { label: '提交删除', stages: ['committing'] },
  ],
}

function knowledgeJobBucket(job: KnowledgeJob): Exclude<KnowledgeJobFilter, 'all'> {
  if (job.status === 'queued' || job.status === 'running' || job.status === 'paused') return 'active'
  if (job.status === 'failed' || job.status === 'interrupted' || (job.status === 'completed' && job.outcome === 'partial')) return 'attention'
  return 'completed'
}

function knowledgeJobTypeLabel(type: KnowledgeJob['type']): string {
  return ({ import: '文档导入', parse: '文档解析', index: '索引构建', delete: '文档删除', rebuild: '索引重建' })[type]
}

function knowledgeJobTitle(job: KnowledgeJob, knowledgeBaseName: string): string {
  const count = job.totalItems > 0 ? job.totalItems : Math.max(job.completedItems + job.failedItems, 1)
  if (job.type === 'import') return `导入 ${count} 个文档到 ${knowledgeBaseName}`
  if (job.type === 'parse') return `解析 ${count} 个文档`
  if (job.type === 'delete') return `删除 ${count} 个文档`
  return `${knowledgeJobTypeLabel(job.type)} ${count} 项`
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
  const [collapsedJobDays, setCollapsedJobDays] = useState<Set<string>>(() => new Set())
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
      setExpandedJobIds((current) => {
        const retained = new Set(Array.from(current).filter((jobId) => nextJobs.some((job) => job.id === jobId)))
        if (retained.size > 0) return retained
        const firstPriorityJob = nextJobs.find((job) => knowledgeJobBucket(job) === 'active' || knowledgeJobBucket(job) === 'attention')
        return new Set(firstPriorityJob ? [firstPriorityJob.id] : [])
      })
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
  const groupedJobs = useMemo(() => {
    const groups = new Map<string, { label: string; date: string; jobs: KnowledgeJob[] }>()
    for (const job of displayedJobs) {
      const dateGroup = knowledgeJobDateGroup(job.updatedAt)
      const group = groups.get(dateGroup.date) ?? { ...dateGroup, jobs: [] }
      group.jobs.push(job)
      groups.set(dateGroup.date, group)
    }
    return Array.from(groups.values())
  }, [displayedJobs])
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
  const jobFilters = <div className="fox-local-kb-job-filters">
    <div className="fox-local-kb-job-selects">
      {!knowledgeBaseId && <Select value={baseFilter} onValueChange={setBaseFilter}><SelectTrigger aria-label="筛选知识库" className="fox-local-kb-job-base-select"><SelectValue placeholder="全部知识库" /></SelectTrigger><SelectContent><SelectItem value="all">全部知识库</SelectItem>{knowledgeBases.map((base) => <SelectItem key={base.id} value={base.id}>{base.name}</SelectItem>)}</SelectContent></Select>}
      <Select value={typeFilter} onValueChange={(value) => setTypeFilter(value as KnowledgeJobTypeFilter)}><SelectTrigger aria-label="筛选任务类型" className="fox-local-kb-job-type-select"><SelectValue placeholder="全部任务类型" /></SelectTrigger><SelectContent><SelectItem value="all">全部任务类型</SelectItem><SelectItem value="import">文档导入</SelectItem><SelectItem value="parse">文档解析</SelectItem><SelectItem value="index">索引构建</SelectItem><SelectItem value="delete">文档删除</SelectItem><SelectItem value="rebuild">索引重建</SelectItem></SelectContent></Select>
      <Select value={dateFilter} onValueChange={(value) => setDateFilter(value as KnowledgeJobDateFilter)}><SelectTrigger aria-label="筛选任务时间" className="fox-local-kb-job-date-select"><CalendarDays /><SelectValue /></SelectTrigger><SelectContent><SelectItem value="all">全部时间</SelectItem><SelectItem value="today">今天</SelectItem><SelectItem value="week">近 7 天</SelectItem></SelectContent></Select>
    </div>
    <Tabs value={statusFilter} onValueChange={(value) => setStatusFilter(value as KnowledgeJobFilter)} className="fox-local-kb-job-filter-tabs">
      <TabsList aria-label="筛选任务状态"><TabsTrigger value="all">全部</TabsTrigger><TabsTrigger value="active">进行中</TabsTrigger><TabsTrigger value="attention">需处理</TabsTrigger><TabsTrigger value="completed">已完成</TabsTrigger></TabsList>
    </Tabs>
    <Button variant="ghost" size="icon" className="fox-local-kb-job-refresh" onClick={() => void load()} disabled={loading} aria-label="刷新任务" title="刷新任务"><RefreshCw className={loading ? 'animate-spin' : undefined} /></Button>
  </div>
  return (
    <div className={cn('fox-local-kb-page', embedded && 'fox-local-kb-embedded-page', className)}>
      <div className={cn('fox-local-kb-content fox-local-kb-jobs-content', embedded && 'fox-local-kb-embedded-content')}>
        {!embedded && <LocalKnowledgeHeader title={knowledgeBaseId ? '任务状态' : '任务中心'} description="" onBack={knowledgeBaseId ? onBack : undefined} />}
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        {!loading && !error && visibleJobs.length === 0 && <section className="fox-local-kb-job-board">{jobFilters}<LocalKnowledgeEmpty title="没有任务记录" description="导入文档或重建文本索引后，任务进度会显示在这里。" action={knowledgeBaseId ? <Button onClick={() => onNavigate?.('documents', knowledgeBaseId)}>前往文档管理</Button> : <Button onClick={() => onNavigate?.('list')}>查看知识库</Button>} /></section>}
        {!loading && !error && visibleJobs.length > 0 && <section className="fox-local-kb-job-board">
          {jobFilters}
          <div className="fox-local-kb-job-timeline" aria-live="polite">
            {groupedJobs.length === 0
              ? <LocalKnowledgeEmpty title="没有符合条件的任务" description="调整状态、知识库、任务类型或时间范围后再试。" />
              : <>{groupedJobs.map((group) => {
                const collapsed = collapsedJobDays.has(group.date)
                return <section className={cn('fox-local-kb-job-day', collapsed && 'is-collapsed')} key={group.date}>
                  <header>
                    <strong>{group.label}</strong>
                    <i />
                    {group.label !== group.date && <span>{group.date}</span>}
                    <button
                      type="button"
                      className="fox-local-kb-job-day-toggle"
                      aria-expanded={!collapsed}
                      aria-label={`${collapsed ? '展开' : '折叠'}${group.label}的任务`}
                      onClick={() => setCollapsedJobDays((current) => {
                        const next = new Set(current)
                        if (next.has(group.date)) next.delete(group.date)
                        else next.add(group.date)
                        return next
                      })}
                    >
                      <ChevronDown />
                    </button>
                  </header>
                  {!collapsed && <div className="fox-local-kb-job-day-list">{group.jobs.map((job) => <JobTimelineItem key={job.id} job={job} knowledgeBaseName={baseNameById.get(job.knowledgeBaseId) ?? job.knowledgeBaseId} expanded={expandedJobIds.has(job.id)} onToggle={() => setExpandedJobIds((current) => { const next = new Set(current); if (next.has(job.id)) next.delete(job.id); else next.add(job.id); return next })} onCancel={() => void cancel(job)} onRetry={() => void retry(job)} />)}</div>}
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
  return <article className={cn('fox-local-kb-job-timeline-item', `is-${bucket}`, expanded && 'is-expanded', partial && 'is-partial')}>
    <div className="fox-local-kb-job-rail"><span>{jobStatusIcon(job.status)}</span><time>{formatTime(job.updatedAt)}</time></div>
    <div className="fox-local-kb-job-panel">
      <button type="button" className="fox-local-kb-job-panel-head" aria-expanded={expanded} onClick={onToggle}>
        <span className="fox-local-kb-job-panel-title"><strong>{knowledgeJobTitle(job, knowledgeBaseName)}</strong><small>{knowledgeJobTypeLabel(job.type)} · {knowledgeBaseName}</small></span>
        {bucket === 'active' && <b>{job.progress}%</b>}
        <Badge variant="secondary" className={cn('fox-local-kb-job-status', `is-${bucket}`, partial && 'is-partial')}>{jobStatusLabel(job.status, job.outcome)}</Badge>
        <ChevronDown />
      </button>
      {expanded && <div className="fox-local-kb-job-panel-body">
        <div className="fox-local-kb-job-stage-scroll"><ol className="fox-local-kb-job-stages">{stageSteps.map((step, index) => {
          const state = job.status === 'completed' && !partial
            ? 'complete'
            : index < currentStageIndex
              ? 'complete'
              : index === currentStageIndex
                ? bucket === 'attention' ? 'warning' : 'active'
                : 'pending'
          return <li className={`is-${state}`} key={step.label}><span>{state === 'complete' ? <CheckCircle2 /> : index + 1}</span><strong>{step.label}</strong><small>{state === 'complete' ? '已完成' : state === 'active' ? '进行中' : state === 'warning' ? '需处理' : '未开始'}</small></li>
        })}</ol></div>
        {bucket === 'active' && <div className="fox-local-kb-job-progress-detail"><div><span>当前阶段：{formatJobStage(job.stage)}</span><strong>{job.progress}%</strong></div><i><b style={{ width: `${Math.max(0, Math.min(100, job.progress))}%` }} /></i></div>}
        {job.error && <p className="fox-local-kb-job-error"><AlertCircle /><span><strong>任务执行失败</strong>{job.error.message}</span></p>}
        <footer className="fox-local-kb-job-detail-footer"><span>已完成 {completedItems}/{totalItems} 项{job.failedItems > 0 ? ` · ${job.failedItems} 项失败` : ''} · 更新于 {formatDate(job.updatedAt)}</span><div>{canCancel && <Button variant="outline" size="sm" onClick={onCancel}>取消</Button>}{canRetry && <Button variant="outline" size="sm" onClick={onRetry}><RotateCcw />重试</Button>}</div></footer>
      </div>}
    </div>
  </article>
}

function modelStatusLabel(model: LocalEmbeddingModel): string {
  if (model.status === 'installing') return '下载中'
  if (model.status === 'ready' && model.loadReady) return '可使用'
  if (model.status === 'ready') return '待加载测试'
  if (model.status === 'error') return '安装失败'
  return '未安装'
}

export function LocalKnowledgeModelsPage({ gateway = defaultLocalKnowledgeGateway, className, onBack }: LocalKnowledgeNavigationProps) {
  const [models, setModels] = useState<LocalEmbeddingModel[]>([])
  const [health, setHealth] = useState<VectorBackendHealth | null>(null)
  const [filesModel, setFilesModel] = useState<LocalEmbeddingModel | null>(null)
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
        toast.success(`${model.name} 隔离测试通过`, {
          ...context,
          description: `${result.message}\n维度：${result.dimension}\n耗时：${result.elapsedMs} ms`,
        })
      } else {
        toast.error(`${model.name} 隔离测试失败`, {
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
      toast.error(`${model.name} 隔离测试未能完成`, {
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

  return (
    <div className={cn('fox-local-kb-page', className)}>
      <div className="fox-local-kb-content fox-local-models-page">
        <LocalKnowledgeHeader title="向量模型" description="" onBack={onBack} actions={<Button variant="outline" onClick={() => void importPackage()} disabled={busyId != null}><PackageOpen />导入模型包</Button>} />
        {health && <div className={cn('fox-local-model-health', health.readWriteVerified ? 'is-ready' : 'is-warning')}><Gauge /><div><strong>{health.backend}</strong><span>{health.message}</span></div><Badge variant="secondary">{health.fallbackActive ? '兼容模式' : '原生模式'}</Badge></div>}
        <div className="fox-local-model-health is-guide"><Info /><div><strong>使用方法</strong><span>安装模型 → 测试 → 设为默认；然后进入知识库“向量索引”，点击“构建向量索引”。默认模型不会替代已经绑定到知识库的模型。</span></div><Badge variant="secondary">3 步</Badge></div>
        {loading && <LocalKnowledgeLoading />}
        {!loading && error && <LocalKnowledgeError message={error} onRetry={() => void load()} />}
        {!loading && models.length > 0 && <div className="fox-local-model-list">
          {models.map((model) => {
            const installing = model.status === 'installing' && model.download
            const busy = busyId === model.id
            return <Card key={`${model.id}-${model.version}`} className={cn('fox-library-card fox-library-card--stacked-meta fox-shadcn-kb-card fox-shadcn-agent-card fox-local-model-card', installing && 'is-installing')}>
              <CardHeader className="fox-shadcn-kb-head fox-local-model-card-head">
                <span className="fox-shadcn-kb-icon fox-shadcn-agent-icon fox-local-model-icon"><Cpu /></span>
                <span className="fox-shadcn-kb-heading">
                  <span className="fox-agent-name-row"><strong>{model.name}</strong></span>
                </span>
              </CardHeader>
              <div className="fox-local-model-primary-action">
                {model.status === 'not_installed' && <Button onClick={() => void run(model.id, () => gateway.startEmbeddingModelInstall(model.id), '模型下载任务已创建。')} disabled={busy}>安装</Button>}
                {installing && model.download && <Button variant="outline" onClick={() => void run(model.id, () => gateway.cancelEmbeddingModelDownload(model.download!.id), '下载已取消，可随时继续。')} disabled={busy}>取消</Button>}
                {(model.status === 'error' || model.download?.status === 'cancelled' || model.download?.status === 'failed') && model.download && <Button onClick={() => void run(model.id, () => gateway.retryEmbeddingModelDownload(model.download!.id), '已从断点继续下载。')} disabled={busy}><RotateCcw />继续</Button>}
                {model.status === 'ready' && <DropdownMenu>
                  <DropdownMenuTrigger asChild><Button className="fox-local-model-more" variant="ghost" size="icon" aria-label={`管理 ${model.name}`} disabled={busy}><MoreHorizontal /></Button></DropdownMenuTrigger>
                  <DropdownMenuContent className="fox-local-model-menu" align="end" sideOffset={5}>
                    <DropdownMenuItem onSelect={() => void testModel(model)}><FlaskConical />隔离测试</DropdownMenuItem>
                    <DropdownMenuItem disabled={model.isDefault || !model.loadReady} onSelect={() => void run(model.id, () => gateway.setDefaultEmbeddingModel(model.id), '已设为默认向量模型；未绑定模型的知识库将在构建向量索引时自动采用它。')}><Star />设为默认</DropdownMenuItem>
                    <DropdownMenuItem onSelect={() => setFilesModel(model)}><FileStack />文件</DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem variant="destructive" onSelect={() => void run(model.id, () => gateway.deleteEmbeddingModel(model.id), '模型已删除。')}><Trash2 />删除</DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>}
              </div>
              <CardContent className="fox-shadcn-kb-content fox-local-model-card-content">
                <p className="fox-agent-description">{model.languages.join(' / ') || '语言未声明'} · {model.license} · {formatBytes(model.sizeBytes)}</p>
                <span className="fox-library-card-tags"><small>向量模型</small><small>{model.dimension} 维</small>{model.recommended && <small>推荐</small>}{model.isDefault && <small>默认</small>}<small>{modelStatusLabel(model)}</small></span>
                {installing && <div className="fox-local-model-download" role="status" aria-live="polite"><div><span>{model.download?.currentFile ? `正在下载 ${model.download.currentFile}` : '准备下载'}</span><b>{model.download?.progress ?? 0}%</b></div><div className="fox-local-kb-progress"><span style={{ width: `${model.download?.progress ?? 0}%` }} /></div><small>{formatBytes(model.download?.downloadedBytes ?? 0)} / {formatBytes(model.download?.totalBytes ?? model.sizeBytes)}</small></div>}
              </CardContent>
            </Card>
          })}
        </div>}
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
      toast.error('知识库检索失败', {
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
      const text = surrounding.length ? surrounding.map((chunk, position) => `${position === index - start ? '当前分块' : '相邻分块'}\n${chunk.content}`).join('\n\n') : item.content
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

  const retrievalActions = <>
    <Button variant="outline" size="sm" onClick={() => void exportCases()} disabled={cases.length === 0}><Copy />导出测试集</Button>
    <Button variant="outline" size="sm" onClick={() => void buildVectorIndex()}><RefreshCw />{base?.vectorIndexReady ? '重建向量索引' : '构建向量索引'}</Button>
  </>

  if (!knowledgeBaseId) return <div className={cn('fox-local-kb-page', className)}><div className="fox-local-kb-content"><LocalKnowledgeEmpty title="没有选择知识库" description="请先进入一个本地知识库。" action={<Button onClick={onBack}>返回</Button>} /></div></div>

  return <div className={cn('fox-local-kb-page', embedded && 'fox-local-kb-embedded-page', className)}><div className={cn('fox-local-kb-content fox-local-retrieval-page', embedded && 'fox-local-kb-embedded-content')}>
    {!embedded && <LocalKnowledgeHeader
      title="检索测试"
      description=""
      onBack={onBack}
    />}
    {loading && <LocalKnowledgeLoading />}
    {!loading && error && <LocalKnowledgeError message={error} />}
    {!loading && base && <>
      {!base.vectorIndexReady && <div className="fox-local-model-health is-warning"><Info /><div><strong>向量检索还差一步</strong><span>{base.configuredEmbeddingModelId ? '当前知识库已经选好模型，请先构建向量索引。' : '点击构建后会自动采用已设置的默认模型，并绑定到当前知识库。'}</span></div><Button variant="outline" size="sm" onClick={() => void buildVectorIndex()} disabled={base.documentCount === 0}>{base.documentCount === 0 ? '请先导入文档' : '立即构建'}</Button></div>}
      <Card className="fox-local-retrieval-query"><div className="fox-local-retrieval-query-row"><Input value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') void search() }} placeholder="输入要验证的问题" aria-label="检索测试问题" /><Button onClick={() => void search()} disabled={running || !query.trim()}>{running ? <LoaderCircle className="animate-spin" /> : <Search />}开始检索</Button></div><div className="fox-local-retrieval-modes" role="group" aria-label="检索方式">{(['keyword', 'vector', 'hybrid'] as KnowledgeSearchMode[]).map((value) => <Button key={value} type="button" size="sm" variant={mode === value ? 'default' : 'outline'} disabled={value !== 'keyword' && !base.vectorIndexReady} onClick={() => setMode(value)}>{value === 'keyword' ? '关键词' : value === 'vector' ? '向量' : '混合'}</Button>)}<span>{base.vectorIndexReady ? `向量代次 ${base.activeGeneration ?? '—'}` : base.fallbackReason}</span><div className="fox-local-retrieval-actions">{retrievalActions}</div></div></Card>
      {result && <section className="fox-local-retrieval-results"><div className="fox-local-kb-section-head"><div><h2>检索结果</h2><p>{result.mode === 'hybrid' ? `${result.fusionVersion} · ` : ''}关键词 {result.timings.keywordMs} ms · 向量 {result.timings.vectorMs} ms · 总计 {result.timings.totalMs} ms</p></div><Button variant="outline" size="sm" onClick={() => void saveCase()}>保存问题</Button></div>{result.items.length === 0 ? <LocalKnowledgeEmpty title="没有命中内容" description="尝试调整问题、检索方式或重建索引。" /> : <div className="fox-local-retrieval-result-list">{result.items.map((item, index) => <Card key={item.chunkId} className="fox-local-retrieval-result"><div><b>{index + 1}</b><div><strong>{item.documentName}</strong><small>{item.relativePath}{item.anchor ? ` · ${item.anchor}` : ''}</small></div><Badge variant="secondary">{item.score.toFixed(4)}</Badge></div><p>{item.content}</p>{contexts[item.chunkId] && <div className={`fox-local-retrieval-context ${contexts[item.chunkId].error ? 'is-error' : ''}`}>{contexts[item.chunkId].loading ? <><LoaderCircle className="animate-spin" />正在读取相邻分块</> : <pre>{contexts[item.chunkId].error ?? contexts[item.chunkId].text}</pre>}</div>}<footer><span>{item.keywordScore != null ? `关键词 ${item.keywordScore.toFixed(3)}` : ''}{item.vectorScore != null ? ` · 向量 ${item.vectorScore.toFixed(3)}` : ''}</span><div><Button variant="ghost" size="sm" onClick={() => void toggleContext(item)}>{contexts[item.chunkId] ? '收起上下文' : '展开上下文'}</Button><Button variant="ghost" size="sm" onClick={() => void copyCitation(item)}><Copy />复制引用</Button><Button variant="ghost" size="sm" onClick={() => onNavigate?.('documents', knowledgeBaseId, item.documentId)}><FileText />定位文档</Button><Button variant="ghost" size="sm" onClick={() => void openOriginal(item)}><ExternalLink />打开原文件</Button></div></footer></Card>)}</div>}</section>}
      {comparison && <section className="fox-local-retrieval-comparison"><div className="fox-local-kb-section-head"><div><h2>检索模式对比</h2><p>同一问题、相同 Top 10，对比关键词、向量与混合检索；未标注答案时仍显示与混合检索 Top 3 的重合度。</p></div></div><div>{comparison.map((item) => <Card key={item.mode}><strong>{item.mode === 'keyword' ? '关键词' : item.mode === 'vector' ? '向量' : '混合'}</strong><span><b>{item.result.timings.totalMs} ms</b><small>总耗时</small></span><span><b>{item.result.items.length}</b><small>命中数</small></span><span><b>{item.expectedCount ? `${item.hitCount}/${item.expectedCount}` : '未标注'}</b><small>预期文档</small></span><span><b>{item.hybridTop3Overlap == null ? '基准' : `${item.hybridTop3Overlap}/${item.hybridTop3Count}`}</b><small>混合 Top 3 重合</small></span><p>{item.result.items[0]?.documentName ?? '无结果'}</p></Card>)}</div></section>}
      <section className="fox-local-retrieval-cases"><div className="fox-local-kb-section-head"><div><h2>回归问题</h2><p>最多保留 20 条问题，可重复对比关键词、向量与混合检索策略。</p></div></div>{cases.length === 0 ? <p className="fox-local-retrieval-empty">还没有保存测试问题。</p> : cases.map((item) => <div key={item.id}><button type="button" onClick={() => { setQuery(item.question); void search(item.question) }}>{item.question}</button><span><Button variant="outline" size="sm" disabled={!base.vectorIndexReady || comparisonRunning} onClick={() => void compareCase(item)}>{comparisonRunning ? <LoaderCircle className="animate-spin" /> : <FlaskConical />}三模式对比</Button><Button variant="ghost" size="sm" onClick={() => void gateway.deleteRetrievalCase(item.id).then(() => setCases((current) => current.filter((entry) => entry.id !== item.id)))}><Trash2 />删除</Button></span></div>)}</section>
    </>}
  </div></div>
}

export function LocalKnowledgeWorkspace({ view = 'list', knowledgeBaseId, documentId, gateway = defaultLocalKnowledgeGateway, onNavigate, onBack, createRequest, onCreateRequestHandled, onOperationAccepted }: LocalKnowledgeNavigationProps & { view?: LocalKnowledgeView; createRequest?: number; onOperationAccepted?: (accepted: OperationAccepted) => void }) {
  const navigate = onNavigate
  switch (view) {
    case 'home': return <LocalKnowledgeHomePage gateway={gateway} onNavigate={navigate} />
    case 'files': return <LocalKnowledgeFilesPage gateway={gateway} onNavigate={navigate} onOperationAccepted={onOperationAccepted} />
    case 'detail': return <LocalKnowledgeDetailPage gateway={gateway} knowledgeBaseId={knowledgeBaseId} onBack={onBack} onNavigate={navigate} onOperationAccepted={onOperationAccepted} />
    case 'documents': return <LocalKnowledgeDocumentsPage gateway={gateway} knowledgeBaseId={knowledgeBaseId} documentId={documentId} onBack={onBack} onNavigate={navigate} onOperationAccepted={onOperationAccepted} />
    case 'import': return <LocalKnowledgeDocumentsPage gateway={gateway} knowledgeBaseId={knowledgeBaseId} onBack={onBack} onNavigate={navigate} initialImportOpen onOperationAccepted={onOperationAccepted} />
    case 'jobs': return <LocalKnowledgeJobsPage gateway={gateway} knowledgeBaseId={knowledgeBaseId} onBack={onBack} onNavigate={navigate} />
    case 'models': return <LocalKnowledgeModelsPage gateway={gateway} onBack={onBack} />
    case 'retrieval': return <LocalKnowledgeRetrievalPage gateway={gateway} knowledgeBaseId={knowledgeBaseId} onBack={onBack} onNavigate={navigate} />
    default: return <LocalKnowledgeListPage gateway={gateway} onNavigate={navigate} createRequest={createRequest} onCreateRequestHandled={onCreateRequestHandled} />
  }
}
