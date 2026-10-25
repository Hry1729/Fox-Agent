import { useEffect, useMemo, useState } from 'react'
import { Activity, Check, ChevronDown, ChevronRight, Copy, Download, HardDrive, Info, LoaderCircle, RotateCcw, Trash2, Zap } from 'lucide-react'
import { getVersion } from '@tauri-apps/api/app'
import { notify as toast } from '@/features/notifications'
import { cn } from '@/lib/utils'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Card } from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { confirmDialog } from '@/components/ui/confirm-dialog'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import type { SettingsSectionAnchor } from '@/features/workspace/management-navigation'
import { desktopClient, desktopRuntimeAvailable } from '@/features/conversations/api/desktop-client'
import type { KnowledgePreviewCacheStatistics, ObservabilityStatistics, UsageStatistics } from '@/features/conversations/model/types'
import { defaultLocalKnowledgeGateway } from '@/features/local-knowledge/tauri-gateway'
import { defaultPluginGateway } from '@/features/plugins/tauri-gateway'
import { SettingsLoadError } from './settings-load-error'
import { SettingsGroup, SettingsPageLayout, SettingsRow, settingsSectionId, useScrollToSettingsSection } from './settings-layout'
import { formatStorageSize } from './settings-shared'

type AppDiagnosticCheck = {
  id: string
  label: string
  status: 'ok' | 'warning' | 'error' | 'offline'
  summary: string
  route?: Parameters<NavigateWorkspace>[0]
  actionLabel?: string
}

function diagnosticStatusLabel(status: AppDiagnosticCheck['status']) {
  return ({ ok: '正常', warning: '需注意', error: '异常', offline: '离线' })[status]
}

const PREVIEW_CACHE_LIMITS = [250 * 1024 * 1024, 500 * 1024 * 1024, 1024 * 1024 * 1024, 2 * 1024 * 1024 * 1024, 5 * 1024 * 1024 * 1024]

/** Full self-check: same ten probes and summaries as before (Runtime, 模型, 资料库, 向量, 插件, 路径). */
async function collectDiagnostics() {
  const checks: AppDiagnosticCheck[] = []
  let runtime: Awaited<ReturnType<typeof desktopClient.runtimeDiagnostics>> | null = null
  const [runtimeResult, providersResult, storageResult, basesResult, jobsResult, modelsResult, vectorResult, mcpResult, pluginsResult, projectsResult] = await Promise.allSettled([
    desktopClient.runtimeDiagnostics(),
    desktopClient.listModelProviders(),
    defaultLocalKnowledgeGateway.getStorageStatus(),
    defaultLocalKnowledgeGateway.listKnowledgeBases(),
    defaultLocalKnowledgeGateway.listJobs(),
    defaultLocalKnowledgeGateway.listEmbeddingModels(),
    defaultLocalKnowledgeGateway.getVectorBackendHealth(),
    desktopClient.listMcpServers(),
    defaultPluginGateway.installationsList(),
    desktopClient.listManagedProjects(),
  ])
  if (runtimeResult.status === 'fulfilled') {
    runtime = runtimeResult.value
    checks.push({ id: 'runtime', label: 'Runtime Host', status: runtimeResult.value.state === 'ready' ? 'ok' : 'warning', summary: `${runtimeResult.value.runtime} · 协议 v${runtimeResult.value.protocolVersion} · ${runtimeResult.value.state}` })
  } else checks.push({ id: 'runtime', label: 'Runtime Host', status: 'error', summary: '无法读取 Runtime 诊断信息' })
  if (providersResult.status === 'fulfilled') {
    const enabled = providersResult.value.filter((item) => item.enabled)
    const healthy = enabled.filter((item) => item.lastStatus === 'connected')
    checks.push({ id: 'models', label: '模型连接', status: enabled.length === 0 ? 'warning' : healthy.length === enabled.length ? 'ok' : 'warning', summary: enabled.length === 0 ? '尚未启用模型供应商' : `${healthy.length} / ${enabled.length} 个已启用供应商连接正常`, route: 'settings-models', actionLabel: '查看模型' })
  } else checks.push({ id: 'models', label: '模型连接', status: 'error', summary: '模型服务状态读取失败', route: 'settings-models', actionLabel: '检查配置' })
  if (storageResult.status === 'fulfilled' && storageResult.value.degradedReason) {
    // Startup could not open the storage for writing: name the path and link to the storage-location entry.
    checks.push({ id: 'knowledge-storage', label: '本机资料库', status: 'error', summary: `${storageResult.value.degradedReason}${storageResult.value.restartRequired ? ' · 新位置重启后生效' : ' · 资料库暂时只读或不可用'}`, route: 'settings-yuxi', actionLabel: '修改存储位置' })
  } else if (storageResult.status === 'fulfilled') {
    checks.push({ id: 'knowledge-storage', label: '本机资料库', status: storageResult.value.writable && !storageResult.value.restartRequired ? 'ok' : 'warning', summary: `数据库 v${storageResult.value.schemaVersion} · ${storageResult.value.writable ? '目录可写' : '目录不可写'}${storageResult.value.restartRequired ? ' · 等待重启完成迁移' : ''}`, route: 'settings-yuxi', actionLabel: '查看存储' })
  } else checks.push({ id: 'knowledge-storage', label: '本机资料库', status: 'error', summary: '无法读取数据库与目录权限', route: 'settings-yuxi', actionLabel: '查看存储' })
  if (basesResult.status === 'fulfilled' && jobsResult.status === 'fulfilled') {
    const activeJobs = jobsResult.value.filter((item) => ['queued', 'running', 'paused'].includes(item.status))
    const failedJobs = jobsResult.value.filter((item) => item.status === 'failed')
    checks.push({ id: 'knowledge-jobs', label: '知识任务队列', status: failedJobs.length ? 'warning' : 'ok', summary: `${basesResult.value.length} 个资料库 · ${activeJobs.length} 个活动任务 · ${failedJobs.length} 个失败任务`, route: 'local-knowledge-jobs', actionLabel: '查看进度' })
  } else checks.push({ id: 'knowledge-jobs', label: '知识任务队列', status: 'offline', summary: '知识任务队列不可用', route: 'local-knowledge-jobs', actionLabel: '查看进度' })
  if (modelsResult.status === 'fulfilled') {
    const readyModels = modelsResult.value.filter((item) => item.status === 'ready' && item.integrityStatus === 'verified' && item.loadReady)
    const damagedModels = modelsResult.value.filter((item) => item.status === 'error' || item.integrityStatus === 'failed')
    checks.push({ id: 'embedding', label: '向量模型', status: damagedModels.length ? 'error' : readyModels.length ? 'ok' : 'warning', summary: `${readyModels.length} 个可加载模型${damagedModels.length ? ` · ${damagedModels.length} 个完整性异常` : ''}`, route: 'local-knowledge-models', actionLabel: readyModels.length ? '管理模型' : '安装模型' })
  } else checks.push({ id: 'embedding', label: '向量模型', status: 'error', summary: '模型清单或 ONNX 运行状态读取失败', route: 'local-knowledge-models', actionLabel: '检查模型' })
  if (vectorResult.status === 'fulfilled') {
    checks.push({ id: 'zvec', label: 'Zvec 向量后端', status: vectorResult.value.readWriteVerified ? 'ok' : vectorResult.value.fallbackActive ? 'warning' : 'error', summary: `${vectorResult.value.backend} · ${vectorResult.value.message}${vectorResult.value.fallbackActive ? ' · 已启用降级存储' : ''}`, route: 'local-knowledge-models', actionLabel: '查看向量模型' })
  } else checks.push({ id: 'zvec', label: 'Zvec 向量后端', status: 'error', summary: '读写自检未完成', route: 'local-knowledge-models', actionLabel: '重新检查' })
  if (mcpResult.status === 'fulfilled') {
    const enabled = mcpResult.value.filter((item) => item.enabled)
    const failed = enabled.filter((item) => item.status === 'unavailable' || Boolean(item.lastError))
    checks.push({ id: 'mcp', label: '连接器', status: failed.length ? 'warning' : 'ok', summary: `${enabled.length} 个已启用 · ${failed.length} 个最近失败`, route: 'settings-extensions', actionLabel: '查看插件' })
  } else checks.push({ id: 'mcp', label: '连接器', status: 'error', summary: '插件连接状态读取失败', route: 'settings-extensions', actionLabel: '检查插件' })
  if (pluginsResult.status === 'fulfilled') {
    const failed = pluginsResult.value.items.filter((item) => !item.compatible || Boolean(item.lastError))
    checks.push({ id: 'plugins', label: '插件', status: failed.length ? 'warning' : 'ok', summary: `${pluginsResult.value.total} 个已安装 · ${failed.length} 个兼容性或运行异常`, route: 'plugins', actionLabel: '查看插件' })
  } else checks.push({ id: 'plugins', label: '插件', status: 'error', summary: '已安装插件状态读取失败', route: 'plugins', actionLabel: '查看插件' })
  if (projectsResult.status === 'fulfilled') {
    const missing = projectsResult.value.filter((item) => item.archivedAt === null && !item.pathExists)
    checks.push({ id: 'system-paths', label: '系统与项目路径', status: missing.length ? 'warning' : 'ok', summary: `${projectsResult.value.length} 个项目 · ${missing.length} 个路径失效`, route: 'settings-projects', actionLabel: '管理项目' })
  } else checks.push({ id: 'system-paths', label: '系统与项目路径', status: 'error', summary: '系统路径能力读取失败', route: 'settings-projects', actionLabel: '检查项目' })
  return { checks, runtime }
}

function DiagnosticsGroup({ id, navigate, busy, error, onExport }: { id?: string; navigate: NavigateWorkspace; busy: boolean; error: string | null; onExport: () => void }) {
  const [runtime, setRuntime] = useState<Awaited<ReturnType<typeof desktopClient.runtimeDiagnostics>> | null>(null)
  const [checks, setChecks] = useState<AppDiagnosticCheck[]>([])
  const [loading, setLoading] = useState(false)
  const [showOk, setShowOk] = useState(false)
  const run = async () => {
    setLoading(true)
    try {
      const result = await collectDiagnostics()
      setChecks(result.checks)
      if (result.runtime) setRuntime(result.runtime)
    } finally { setLoading(false) }
  }
  useEffect(() => { void run() }, [])
  const copySummary = async () => {
    const lines = ['Fox 脱敏诊断摘要', `生成时间：${new Date().toLocaleString('zh-CN')}`, ...checks.map((item) => `[${diagnosticStatusLabel(item.status)}] ${item.label}：${item.summary}`)]
    try { await navigator.clipboard.writeText(lines.join('\n')); toast.success('脱敏诊断摘要已复制') }
    catch { toast.error('无法访问剪贴板') }
  }
  const problems = checks.filter((item) => item.status !== 'ok')
  const okChecks = checks.filter((item) => item.status === 'ok')
  const summaryTone = problems.some((item) => item.status === 'error') ? 'is-error' : problems.length ? 'is-warning' : 'is-ok'
  return <SettingsGroup id={id} title="诊断" aside={<Button type="button" variant="ghost" size="sm" disabled={loading || busy} onClick={() => void run()}><RotateCcw className={loading ? 'animate-spin' : undefined} />重新诊断</Button>}>
    <SettingsRow title={<span className="fox-settings-diagnostic-title" aria-live="polite">{loading && checks.length === 0 ? <><LoaderCircle className="animate-spin" />正在逐项自检</> : <><i className={cn('fox-settings-dot', summaryTone)} aria-hidden="true" />{problems.length ? `${problems.length} 项需要处理` : '全部正常'}</>}</span>} description={runtime ? `Runtime ${runtime.runtime} · 协议 v${runtime.protocolVersion} · ${runtime.sessionFileCount} 个 Session · 恢复尝试 ${runtime.recoveryAttempts ?? 0} 次` : '检查 Runtime、模型、资料库、向量、插件与系统路径；摘要不含凭证、正文或文件内容'}>
      <div className="fox-settings-inline-actions">
        <Button type="button" variant="outline" size="sm" disabled={!checks.length} onClick={() => void copySummary()}><Copy />复制摘要</Button>
        <Button type="button" variant="outline" size="sm" disabled={busy} onClick={onExport}><Download />导出诊断包</Button>
      </div>
    </SettingsRow>
    {problems.map((item) => <SettingsRow key={item.id} className={cn('fox-settings-check', `is-${item.status}`)} title={<span className="fox-settings-diagnostic-title"><i className={cn('fox-settings-dot', item.status === 'error' ? 'is-error' : 'is-warning')} aria-hidden="true" />{item.label}<small>{diagnosticStatusLabel(item.status)}</small></span>} description={item.summary}>
      {item.route && <Button type="button" variant="ghost" size="sm" onClick={() => navigate(item.route!)}>{item.actionLabel ?? '查看'}<ChevronRight /></Button>}
    </SettingsRow>)}
    {error && <p className="fox-settings-error" role="alert">操作没有完成：{error}</p>}
    {okChecks.length > 0 && <>
      <button type="button" className="fox-settings-disclosure fox-settings-ok-toggle" aria-expanded={showOk} onClick={() => setShowOk((open) => !open)}><span><b>{okChecks.length} 项正常</b></span><ChevronDown /></button>
      {showOk && okChecks.map((item) => <SettingsRow key={item.id} className="fox-settings-check is-ok" title={<span className="fox-settings-diagnostic-title"><i className="fox-settings-dot is-ok" aria-hidden="true" />{item.label}</span>} description={item.summary} />)}
    </>}
  </SettingsGroup>
}

function PreviewCacheGroup({ busy, setBusy, setError }: { busy: string | null; setBusy: (key: string | null) => void; setError: (message: string | null) => void }) {
  const [cache, setCache] = useState<KnowledgePreviewCacheStatistics | null>(null)
  const refresh = async () => {
    try { setCache(await desktopClient.knowledgePreviewCacheStatistics()) }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
  }
  useEffect(() => { void refresh() }, []) // eslint-disable-line react-hooks/exhaustive-deps
  const clear = async () => {
    const confirmed = await confirmDialog({ title: '清理预览缓存', description: '清理所有未使用的资料库预览缓存？正在打开的文件会保留。', confirmLabel: '清理', destructive: true })
    if (!confirmed) return
    setBusy('preview-cache'); setError(null)
    try {
      const result = await desktopClient.clearKnowledgePreviewCache()
      toast.success(`${result.message}，释放 ${formatStorageSize(result.bytes)}`)
      await refresh()
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setBusy(null) }
  }
  const updateLimit = async (value: string) => {
    const limitBytes = Number(value)
    if (!Number.isFinite(limitBytes)) return
    setBusy('preview-cache-limit'); setError(null)
    try {
      setCache(await desktopClient.setKnowledgePreviewCacheLimit(limitBytes))
      toast.success(`预览缓存上限已调整为 ${formatStorageSize(limitBytes)}`)
    } catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setBusy(null) }
  }
  const ratio = cache?.limitBytes ? Math.min(1, cache.totalBytes / cache.limitBytes) : 0
  const clearable = Math.max(0, (cache?.totalBytes ?? 0) - (cache?.activeBytes ?? 0))
  const limitOptions = cache && !PREVIEW_CACHE_LIMITS.includes(cache.limitBytes) ? [...PREVIEW_CACHE_LIMITS, cache.limitBytes].sort((a, b) => a - b) : PREVIEW_CACHE_LIMITS
  return <SettingsGroup title="预览缓存" aside={<Button type="button" variant="ghost" size="sm" disabled={busy != null} onClick={() => void refresh()}><RotateCcw />刷新用量</Button>}>
    <SettingsRow title="已用空间" description={cache ? `${formatStorageSize(cache.totalBytes)} / ${formatStorageSize(cache.limitBytes)} · 可清理 ${formatStorageSize(clearable)} · ${cache.activeFiles} 个文件使用中` : '正在读取预览缓存…'}>
      <div className="fox-settings-meter-control">
        <span className="fox-settings-meter" role="meter" aria-label="预览缓存占用" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(ratio * 100)}><i style={{ width: `${ratio * 100}%` }} /></span>
        <Button type="button" variant="outline" size="sm" disabled={busy != null || !cache || clearable === 0} onClick={() => void clear()}>{busy === 'preview-cache' ? <LoaderCircle className="animate-spin" /> : <Trash2 />}清理未使用缓存</Button>
      </div>
    </SettingsRow>
    <SettingsRow title="缓存上限" description="达到上限后按最近使用时间自动清理；调小会立即淘汰未使用的旧文件，正在打开的预览会保留">
      <Select value={cache ? String(cache.limitBytes) : undefined} disabled={busy != null || !cache} onValueChange={(value) => void updateLimit(value)}>
        <SelectTrigger className="fox-settings-compact-select" aria-label="缓存上限"><SelectValue placeholder="选择上限" /></SelectTrigger>
        <SelectContent>{limitOptions.map((bytes) => <SelectItem key={bytes} value={String(bytes)}>{formatStorageSize(bytes)}</SelectItem>)}</SelectContent>
      </Select>
    </SettingsRow>
  </SettingsGroup>
}

/** The running build's version from Tauri; '' while reading, null when it cannot be read. */
function useAppVersion() {
  const [version, setVersion] = useState<string | null>(desktopRuntimeAvailable ? '' : null)
  useEffect(() => {
    if (!desktopRuntimeAvailable) return
    let cancelled = false
    void getVersion().then((value) => { if (!cancelled) setVersion(typeof value === 'string' && value ? value : null) }).catch(() => { if (!cancelled) setVersion(null) })
    return () => { cancelled = true }
  }, [])
  return version
}

export function DataAboutSettingsPage({ sidebarCollapsed, onSidebar, navigate, section }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; section?: SettingsSectionAnchor }) {
  useScrollToSettingsSection(section)
  const version = useAppVersion()
  const [busy, setBusy] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [lastPath, setLastPath] = useState<string | null>(null)
  const [restorePath, setRestorePath] = useState('')
  const [retentionDays, setRetentionDays] = useState('30')
  const run = async (key: string, operation: () => Promise<{ message: string; path: string | null; restartRequired: boolean }>) => {
    setBusy(key); setError(null)
    try { const result = await operation(); setLastPath(result.path); toast.success(result.message); if (result.restartRequired) toast('请关闭并重新启动 Fox 以应用恢复') }
    catch (cause) { setError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setBusy(null) }
  }
  const restore = async () => {
    const path = restorePath.trim()
    if (!path) return
    const confirmed = await confirmDialog({ title: '恢复数据', description: '恢复会在下次启动时替换当前 Fox 核心数据，是否继续？', confirmLabel: '继续恢复', destructive: true })
    if (confirmed) void run('restore', () => desktopClient.restoreBackup(path))
  }
  const cleanup = async () => {
    const days = Math.max(1, Number(retentionDays) || 30)
    const confirmed = await confirmDialog({ title: '清理数据', description: '开始清理 Fox 内部的孤立文件？', confirmLabel: '开始清理', destructive: true })
    if (confirmed) void run('cleanup', () => desktopClient.cleanupData(days))
  }
  return <WorkspacePage title="数据与关于" subtitle="使用情况、维护与版本" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout title="数据与关于" description="查看使用情况、检查和维护本机数据，以及 Fox 的版本信息。">
      <UsageGroup />
        <DiagnosticsGroup id={settingsSectionId('maintenance')} navigate={navigate} busy={busy != null} error={error} onExport={() => void run('diagnostics', desktopClient.exportDiagnostics)} />
        <PreviewCacheGroup busy={busy} setBusy={setBusy} setError={setError} />
        <SettingsGroup title="备份与恢复" aside="备份不包含 API Key、Token 或系统凭证库内容">
          <SettingsRow title="备份 Fox" description="包含 SQLite、附件、技能与 Runtime Session；Session 仅尽力跨版本恢复">
            <Button type="button" variant="outline" size="sm" disabled={busy != null} onClick={() => void run('backup', desktopClient.createBackup)}>{busy === 'backup' ? <LoaderCircle className="animate-spin" /> : <HardDrive />}创建备份</Button>
          </SettingsRow>
          <SettingsRow title="恢复数据" description="先校验清单、哈希、路径与数据库完整性，下次启动 Fox 时应用" align="start">
            <div className="fox-settings-inline-form">
              <Input aria-label="备份位置" value={restorePath} onChange={(event) => setRestorePath(event.target.value)} placeholder="D:\Backups\fox-backup-….foxbackup" />
              <Button type="button" variant="outline" size="sm" disabled={busy != null || !restorePath.trim()} onClick={() => void restore()}>{busy === 'restore' ? <LoaderCircle className="animate-spin" /> : <RotateCcw />}验证并安排恢复</Button>
            </div>
          </SettingsRow>
          <SettingsRow title="清理数据" description="清理超过保留天数的孤立 Session、孤立附件和失败记录，不触碰项目目录中的已导出文件" align="start">
            <div className="fox-settings-inline-form">
              <label className="fox-settings-days"><Input aria-label="孤立 Session 保留天数" inputMode="numeric" value={retentionDays} onChange={(event) => setRetentionDays(event.target.value)} /><span>天</span></label>
              <Button type="button" variant="outline" size="sm" disabled={busy != null} onClick={() => void cleanup()}>{busy === 'cleanup' ? <LoaderCircle className="animate-spin" /> : <Trash2 />}开始清理</Button>
            </div>
          </SettingsRow>
          {lastPath && <p className="fox-settings-result" role="status"><Check /><span><b>操作完成</b><small title={lastPath}>{lastPath}</small></span></p>}
        </SettingsGroup>
      <SettingsGroup id={settingsSectionId('about')} title="关于">
        <div className="fox-settings-about">
          <img src="/mascot/fox/status/fox_sayhi.png" alt="" />
          <span><b>Fox</b><small>轻量、高效、可连接资料库服务的桌面专家工作台。</small></span>
        </div>
        <SettingsRow title="版本" description="当前运行的 Fox 桌面版本">
          <span className="fox-settings-value">{version === '' ? '读取中' : version ?? '无法读取'}</span>
        </SettingsRow>
        <SettingsRow title="技术栈" description="Tauri 2、React、shadcn/ui、AI Elements 与 Fox Runtime" />
        <SettingsRow title="许可" description="Fox 自身许可将在正式发布前确定；第三方依赖遵循各自许可证" />
      </SettingsGroup>
    </SettingsPageLayout>
  </WorkspacePage>
}

function formatUsageNumber(value: number) {
  return new Intl.NumberFormat('zh-CN', { notation: value >= 10_000 ? 'compact' : 'standard', maximumFractionDigits: 1 }).format(value)
}

function formatDuration(value: number | null) {
  if (value === null) return '进行中'
  if (value < 1000) return `${value} ms`
  return `${(value / 1000).toFixed(value < 10_000 ? 1 : 0)} s`
}

/** 使用情况: same queries, heatmap, Trace and offline evaluation as before, inside the unified page. */
function UsageGroup() {
  const [statistics, setStatistics] = useState<UsageStatistics | null>(null)
  const [observability, setObservability] = useState<ObservabilityStatistics | null>(null)
  const [loading, setLoading] = useState(true)
  const [evaluationLoading, setEvaluationLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const refresh = () => {
    setLoading(true)
    setError(null)
    void Promise.all([desktopClient.usageStatistics(), desktopClient.observabilityStatistics()])
      .then(([usage, traces]) => { setStatistics(usage); setObservability(traces) })
      .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
      .finally(() => setLoading(false))
  }
  const runEvaluation = () => {
    setEvaluationLoading(true)
    setError(null)
    void desktopClient.runOfflineEvaluation()
      .then((result) => {
        toast.success(`离线评测完成：${result.passed}/${result.total}`)
        refresh()
      })
      .catch((cause) => setError(cause instanceof Error ? cause.message : String(cause)))
      .finally(() => setEvaluationLoading(false))
  }
  useEffect(refresh, [])
  const activityRecord = useMemo(() => {
    const days = statistics?.days ?? []
    const leadingDays = days.length ? (new Date(`${days[0].date}T00:00:00`).getDay() + 6) % 7 : 0
    const months = days.reduce<string[]>((labels, day, index) => {
      const month = `${Number(day.date.slice(5, 7))}月`
      if (index === 0 || day.date.endsWith('-01')) labels.push(month)
      return labels
    }, [])
    return {
      days: [...Array.from({ length: leadingDays }, () => null), ...days],
      months,
      maxActivity: Math.max(1, ...days.map((day) => day.runCount + day.conversationCount)),
    }
  }, [statistics])
  const maxAgentRuns = Math.max(1, ...(statistics?.agents.map((agent) => agent.runCount) ?? [1]))
  const cacheEligibleTokens = (statistics?.inputTokens ?? 0) + (statistics?.cacheReadTokens ?? 0)
  const cacheHitRate = cacheEligibleTokens ? Math.round((statistics?.cacheReadTokens ?? 0) / cacheEligibleTokens * 100) : 0
  const latestEvaluation = observability?.evaluationHistory[0] ?? null
  const operationLabels: Record<string, string> = { plan: '规划', inference: '模型', execute_tool: '工具', host_prepare: '准备', finalize: '收尾', ui_render: '界面' }
  return <SettingsGroup id={settingsSectionId('usage')} title="使用情况" className="fox-settings-usage-group" aside={<Button type="button" variant="ghost" size="sm" onClick={refresh} disabled={loading}><RotateCcw className={loading ? 'animate-spin' : ''} />刷新</Button>}>
    <p className="fox-settings-group-lead">这台设备上的使用、分段延迟、Trace 覆盖与固定离线回归；不包含费用估算。</p>
    {error && <SettingsLoadError message="暂时无法读取用量数据" detail={error} onRetry={refresh} />}
    {!statistics && loading ? <Card className="fox-settings-section fox-usage-loading"><LoaderCircle className="animate-spin" /><span>正在汇总使用记录</span></Card> : statistics && <>
      <Card className="fox-agent-work-card fox-usage-record-card">
        <div className="fox-agent-work-head"><h2>使用记录</h2></div>
        <div className="fox-agent-work-content">
          <div className="fox-agent-work-metrics">
            {[['对话数', statistics.conversationCount], ['完成任务', statistics.completedRunCount], ['活跃天数', statistics.activeDayCount], ['Token 总量', formatUsageNumber(statistics.totalTokens)]].map(([label, value]) => <div key={label}><strong>{value}</strong><span>{label} <Info /></span></div>)}
          </div>
          <div className="fox-agent-activity-panel" aria-label="最近一年活动热力图">
            <div className="fox-agent-activity-months">{activityRecord.months.map((month, index) => <span key={`${month}-${index}`}>{month}</span>)}</div>
            <div className="fox-agent-activity-chart">
              <div className="fox-agent-activity-weekdays"><span>周一</span><span>周三</span><span>周五</span><span>周日</span></div>
              <div className="fox-agent-activity-grid">{activityRecord.days.map((day, index) => {
                if (!day) return <i key={`empty-${index}`} className="is-empty" aria-hidden="true" />
                const activity = day.runCount + day.conversationCount
                const level = activity === 0 ? 0 : Math.max(1, Math.ceil(activity / activityRecord.maxActivity * 4))
                return <i key={day.date} className={`is-level-${level}`} title={`${day.date} · ${day.conversationCount} 个对话 · ${day.runCount} 次运行 · ${formatUsageNumber(day.totalTokens)} tokens`} />
              })}</div>
            </div>
            <div className="fox-agent-activity-legend"><span>少</span>{[0, 1, 2, 3, 4].map((level) => <i key={level} className={`is-level-${level}`} />)}<span>多</span></div>
          </div>
        </div>
      </Card>
      <div className="fox-usage-columns">
        <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>专家使用</h2></div></div>{statistics.agents.length ? <div className="fox-usage-agents">{statistics.agents.map((agent, index) => <div key={agent.agentId}><span className="fox-usage-rank">{index + 1}</span><div><header><b>{agent.agentName}</b><small>{agent.runCount} 次运行</small></header><span><i style={{ width: `${agent.runCount / maxAgentRuns * 100}%` }} /></span><small>{agent.conversationCount} 个对话 · {formatUsageNumber(agent.totalTokens)} tokens</small></div></div>)}</div> : <p className="fox-settings-empty-copy">还没有专家使用记录。</p>}</Card>
        <Card className="fox-settings-section"><div className="fox-settings-section-head"><div><h2>Token 构成</h2><p>Prompt 与工具目录共享同一供应商缓存口径。</p></div><Badge variant="secondary">命中 {cacheHitRate}%</Badge></div><div className="fox-token-summary"><div><span>输入</span><strong>{formatUsageNumber(statistics.inputTokens)}</strong><i style={{ width: `${statistics.totalTokens ? statistics.inputTokens / statistics.totalTokens * 100 : 0}%` }} /></div><div><span>输出</span><strong>{formatUsageNumber(statistics.outputTokens)}</strong><i style={{ width: `${statistics.totalTokens ? statistics.outputTokens / statistics.totalTokens * 100 : 0}%` }} /></div><div><span>缓存读取</span><strong>{formatUsageNumber(statistics.cacheReadTokens)}</strong><i style={{ width: `${statistics.totalTokens ? statistics.cacheReadTokens / statistics.totalTokens * 100 : 0}%` }} /></div><div><span>缓存写入</span><strong>{formatUsageNumber(statistics.cacheWriteTokens)}</strong><i style={{ width: `${statistics.totalTokens ? statistics.cacheWriteTokens / statistics.totalTokens * 100 : 0}%` }} /></div></div><p className="fox-usage-note"><Activity />命中率 = 缓存读取 /（普通输入 + 缓存读取）；统计取每次运行最后一次 usage，避免流式重复累计。</p></Card>
      </div>
      {observability && <>
        <Card className="fox-settings-section fox-observability-card">
          <div className="fox-settings-section-head"><div><h2>运行可观测性</h2><p>Fox Trace schema v{observability.traceSchemaVersion} · 每次运行以 invoke_agent 为根 Span。</p></div><Badge variant={observability.traceCoveragePercent === 100 ? 'secondary' : 'outline'}>{observability.traceCoveragePercent}% Trace 覆盖</Badge></div>
          <div className={`fox-observability-metrics ${observability.latencyMetrics.length > 4 && observability.latencyMetrics.length % 3 === 0 ? 'is-balanced-three' : ''}`}>
            {observability.latencyMetrics.length ? observability.latencyMetrics.map((metric) => <div key={metric.operation}><span>{operationLabels[metric.operation] ?? metric.operation}</span><strong>P50 {formatDuration(metric.p50Ms)}</strong><small>P95 {formatDuration(metric.p95Ms)} · {metric.sampleCount} 样本</small></div>) : <p className="fox-settings-empty-copy">完成一次本机运行后会显示规划、模型、工具和界面分段耗时。</p>}
          </div>
          {observability.recentRuns.length > 0 && <div className="fox-trace-run-list">{observability.recentRuns.slice(0, 8).map((run) => <div key={run.runId}><span><b>{run.model || '默认模型'}</b><small title={run.traceId}>{run.traceId.slice(0, 12)}… · {run.spanCount} spans</small></span><span><small>规划 {formatDuration(run.planningDurationMs)} · 模型 {formatDuration(run.modelDurationMs)} · 工具 {formatDuration(run.toolDurationMs)}</small><b>{formatDuration(run.totalDurationMs)}</b></span></div>)}</div>}
        </Card>
        <Card className="fox-settings-section fox-evaluation-card">
          <div className="fox-settings-section-head"><div><h2>Agent 固定回归集</h2><p>SWE-bench、BFCL、AgentDojo 风格的本机契约评测；结果不冒充官方榜单成绩。</p></div><Button variant="outline" size="sm" onClick={runEvaluation} disabled={evaluationLoading || loading}>{evaluationLoading ? <LoaderCircle className="animate-spin" /> : <Zap />}运行离线评测</Button></div>
          {latestEvaluation ? <><div className="fox-evaluation-summary"><strong>{latestEvaluation.passed}/{latestEvaluation.total}</strong><span>{latestEvaluation.failed ? `${latestEvaluation.failed} 项失败` : '全部通过'} · {latestEvaluation.suites} 个套件 · {formatDuration(latestEvaluation.durationMs)}</span><code title={latestEvaluation.reportHash}>{latestEvaluation.reportHash.slice(0, 12)}</code></div><div className="fox-evaluation-suites">{latestEvaluation.suiteResults.map((suite) => <div key={suite.name}><span>{suite.name}</span><b className={suite.failed ? 'is-failed' : ''}>{suite.passed}/{suite.total}</b></div>)}</div>{observability.evaluationHistory.length > 1 && <p className="fox-usage-note"><Activity />已保留最近 {observability.evaluationHistory.length} 次结果，可观察固定数据集通过率趋势。</p>}</> : <p className="fox-settings-empty-copy">尚无本机评测记录。运行时只读取内置固定夹具，不调用真实模型、网络或项目文件。</p>}
        </Card>
      </>}
    </>}
  </SettingsGroup>
}

