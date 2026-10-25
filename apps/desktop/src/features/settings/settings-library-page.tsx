import { useCallback, useEffect, useState } from 'react'
import { AlertCircle, ChevronRight, Database, FolderOpen, LoaderCircle, LogIn, Server, Shield } from 'lucide-react'
import { notify as toast } from '@/features/notifications'
import { cn } from '@/lib/utils'
import { Avatar, AvatarFallback, AvatarImage } from '@/components/ui/avatar'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { WorkspacePage } from '@/features/workspace/page-shell'
import type { NavigateWorkspace } from '@/features/workspace/types'
import { StorageLocationDialog } from '@/features/local-knowledge/local-knowledge-pages'
import { defaultLocalKnowledgeGateway } from '@/features/local-knowledge/tauri-gateway'
import type { LocalKnowledgeBase, LocalKnowledgeGateway, LocalKnowledgeStorageStatus } from '@/features/local-knowledge/model'
import { YUXI_LOGIN_RETURN_KEY } from '@/features/knowledge/knowledge-navigation'
import { useYuxiService } from './use-yuxi-service'
import { useYuxiUser } from './use-yuxi-user'
import { SettingsGroup, SettingsPageLayout, SettingsRow } from './settings-layout'
import { connectionTypeLabel, knowledgeServiceName, serviceStatusLabel } from './settings-shared'

function storageSummary(status: LocalKnowledgeStorageStatus) {
  if (status.degradedReason) return status.restartRequired ? '新位置已记录，重启 Fox 后生效' : '当前位置不可写，资料库暂时只读或不可用'
  if (status.restartRequired) return `已迁移到 ${status.pendingRootPath ?? '新位置'}，重启 Fox 后生效`
  if (status.migrationId) return '正在迁移到新位置…'
  return `数据库 v${status.schemaVersion} · ${status.writable ? '目录可写' : '目录不可写'}`
}

/** 资料库服务 connection, edited in the 480px Sheet (same test / save calls as the 连接资料库 page). */
function YuxiServiceSheet({ open, yuxi, onOpenChange }: { open: boolean; yuxi: ReturnType<typeof useYuxiService>; onOpenChange: (open: boolean) => void }) {
  const [name, setName] = useState('本机资料库')
  const [baseUrl, setBaseUrl] = useState('http://127.0.0.1:5050')
  const [token, setToken] = useState('')
  useEffect(() => {
    if (!open) return
    setToken('')
    if (yuxi.service) { setName(yuxi.service.name); setBaseUrl(yuxi.service.baseUrl) }
  }, [open, yuxi.service])
  const test = async () => {
    const result = await yuxi.test(baseUrl, token || undefined)
    if (result) toast.success(result.authenticated ? '连接成功，访问凭证已验证' : `资料库服务连接成功 · ${result.version ?? '版本未知'}`)
  }
  const save = async () => {
    const saved = await yuxi.save({ name, baseUrl, accessToken: token || undefined })
    if (!saved) return
    await yuxi.test(saved.baseUrl)
    setToken('')
    toast.success('资料库服务配置已保存')
    onOpenChange(false)
  }
  const result = yuxi.testResult
  return <Sheet open={open} onOpenChange={onOpenChange}>
    <SheetContent side="right" className="fox-settings-sheet">
      <SheetHeader className="fox-settings-sheet-head">
        <span className="fox-settings-provider-mark is-large"><Server /></span>
        <div className="fox-settings-sheet-title"><SheetTitle><span>{yuxi.service ? knowledgeServiceName(yuxi.service.name) : '连接资料库服务'}</span></SheetTitle><SheetDescription>远程专家、资料库和知识图谱能力的来源；支持本机、局域网和 HTTPS 地址。</SheetDescription></div>
      </SheetHeader>
      <div className="fox-settings-sheet-body">
        <section className="fox-settings-sheet-section">
          <label className="fox-settings-field"><span>服务名称</span><Input value={name} onChange={(event) => setName(event.target.value)} /></label>
          <label className="fox-settings-field"><span>API 地址</span><Input value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} placeholder="http://127.0.0.1:5050" /><small className="fox-settings-field-help">远程示例：https://knowledge.example.com，Fox 会检测 /api/system/health。</small></label>
          <label className="fox-settings-field"><span>访问令牌</span><Input type="password" value={token} onChange={(event) => setToken(event.target.value)} placeholder={yuxi.service?.credentialConfigured ? '已保存；留空保留原值' : '可选：API Key 或登录 Token'} /></label>
          <div className="fox-settings-test-row">
            <Button type="button" variant="outline" size="sm" disabled={yuxi.testing || !baseUrl.trim()} onClick={() => void test()}>{yuxi.testing && <LoaderCircle className="animate-spin" />}测试连接</Button>
            {result ? <span className="fox-settings-test-result"><i className="fox-settings-dot is-ok" aria-hidden="true" /><b>{result.authenticated ? '已验证' : '已连通'}</b><span className="is-separator">·</span><span>{result.version ?? '未知版本'} · {result.latencyMs} ms · {connectionTypeLabel(result.connectionType)}</span></span>
              : yuxi.error ? <span className="fox-settings-test-result is-error"><i className="fox-settings-dot is-error" aria-hidden="true" /><b>连接失败</b><span title={yuxi.error}>{yuxi.error}</span></span> : null}
          </div>
          <p className="fox-settings-sheet-hint"><Shield />Token 使用系统凭证库保存，不写入 Fox 数据库。</p>
        </section>
      </div>
      <div className="fox-settings-sheet-foot">
        <span className="is-grow" />
        <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>取消</Button>
        <Button type="button" disabled={yuxi.saving || !name.trim() || !baseUrl.trim()} onClick={() => void save()}>{yuxi.saving && <LoaderCircle className="animate-spin" />}保存</Button>
      </div>
    </SheetContent>
  </Sheet>
}

export function YuxiSettingsPage({ sidebarCollapsed, onSidebar, navigate, gateway = defaultLocalKnowledgeGateway }: { sidebarCollapsed: boolean; onSidebar: () => void; navigate: NavigateWorkspace; gateway?: LocalKnowledgeGateway }) {
  const yuxi = useYuxiService()
  const user = useYuxiUser(Boolean(yuxi.service?.credentialConfigured))
  const [storage, setStorage] = useState<LocalKnowledgeStorageStatus | null>(null)
  const [storageError, setStorageError] = useState<string | null>(null)
  const [storageOpen, setStorageOpen] = useState(false)
  const [libraries, setLibraries] = useState<LocalKnowledgeBase[]>([])
  const [librariesLoading, setLibrariesLoading] = useState(true)
  const [librariesError, setLibrariesError] = useState<string | null>(null)
  const [serviceOpen, setServiceOpen] = useState(false)
  const loadStorage = useCallback(async () => {
    try { setStorage(await gateway.getStorageStatus()); setStorageError(null) }
    catch (cause) { setStorageError(cause instanceof Error ? cause.message : String(cause)) }
  }, [gateway])
  const loadLibraries = useCallback(async () => {
    setLibrariesLoading(true)
    try { setLibraries(await gateway.listKnowledgeBases()); setLibrariesError(null) }
    catch (cause) { setLibrariesError(cause instanceof Error ? cause.message : String(cause)) }
    finally { setLibrariesLoading(false) }
  }, [gateway])
  useEffect(() => { void loadLibraries() }, [loadLibraries])
  // Re-read after the storage dialog closes: a migration or a degraded reset changes the status.
  useEffect(() => { if (!storageOpen) void loadStorage() }, [storageOpen, loadStorage])
  const openLogin = () => {
    window.sessionStorage.setItem(YUXI_LOGIN_RETURN_KEY, 'settings-yuxi')
    navigate('login')
  }
  const service = yuxi.service
  const serviceTone = service?.lastStatus === 'connected' ? 'is-ok' : service?.lastStatus === 'unavailable' ? 'is-error' : 'is-empty'
  return <WorkspacePage title="资料库" subtitle="存储位置与资料库服务" sidebarCollapsed={sidebarCollapsed} onSidebar={onSidebar}>
    <SettingsPageLayout title="资料库" description="管理本机资料库的存储位置、资料库服务连接和账户。">
      {storage?.degradedReason && <div className="fox-settings-inline-alert is-error" role="alert" data-testid="fox-settings-storage-degraded">
        <AlertCircle />
        <span><b>{storage.degradedReason}</b><small>{storage.restartRequired ? '新位置已记录，重启 Fox 后生效。' : '资料库暂时只读或不可用。选择一个可写的文件夹，或恢复默认位置。'}</small></span>
        <Button type="button" variant="outline" size="sm" onClick={() => setStorageOpen(true)}>修改存储位置</Button>
      </div>}

      <SettingsGroup title="本机存储">
        <SettingsRow className="fox-settings-path-row" title="存储位置" description={storage ? <><code title={storage.rootPath}>{storage.rootPath}</code><span>{storageSummary(storage)}</span></> : storageError ? `暂时无法读取存储位置：${storageError}` : '正在读取存储位置…'}>
          <Button type="button" variant="outline" size="sm" disabled={!storage && !storageError} onClick={() => setStorageOpen(true)}><FolderOpen />修改位置</Button>
        </SettingsRow>
        <SettingsRow title="向量模型" description="本机检索使用的嵌入模型、完整性校验和向量后端">
          <Button type="button" variant="ghost" size="sm" onClick={() => navigate('local-knowledge-models')}>管理<ChevronRight /></Button>
        </SettingsRow>
        <SettingsRow title="任务队列" description="导入、解析和建索引任务的进度与失败记录">
          <Button type="button" variant="ghost" size="sm" onClick={() => navigate('local-knowledge-jobs')}>查看<ChevronRight /></Button>
        </SettingsRow>
      </SettingsGroup>

      <SettingsGroup title="本机资料库" aside={<>{libraries.length > 0 && <span>{libraries.length} 个</span>}<Button type="button" variant="ghost" size="sm" onClick={() => navigate('knowledge')}>打开资料库<ChevronRight /></Button></>}>
        {librariesLoading && libraries.length === 0 && <p className="fox-settings-empty"><LoaderCircle className="animate-spin" /> 正在读取资料库…</p>}
        {!librariesLoading && librariesError && <p className="fox-settings-error" role="alert">暂时无法读取资料库：{librariesError}</p>}
        {!librariesLoading && !librariesError && libraries.length === 0 && <p className="fox-settings-empty">还没有本机资料库，可在资料库页面新建。</p>}
        <div className="fox-settings-providers">
          {libraries.map((library) => <div className="fox-settings-provider" key={library.id}>
            <button type="button" className="fox-settings-provider-main" onClick={() => navigate('local-knowledge-settings', library.id)} aria-label={`编辑 ${library.name}`}>
              <span className="fox-settings-provider-mark"><Database /></span>
              <span className="fox-settings-provider-text"><span className="fox-settings-provider-name"><b>{library.name}</b></span><small title={library.description}>{library.description || (library.vectorIndexReady ? '全文与向量检索' : library.textIndexReady ? '全文检索' : '索引未就绪')}</small></span>
              <span className="fox-settings-provider-status"><span>{library.documentCount} 个文件</span><ChevronRight className="fox-settings-provider-chevron" /></span>
            </button>
          </div>)}
        </div>
      </SettingsGroup>

      <SettingsGroup title="资料库服务" aside={service ? connectionTypeLabel(service.connectionType) : undefined}>
        <div className="fox-settings-providers">
          <div className="fox-settings-provider">
            <button type="button" className="fox-settings-provider-main" onClick={() => setServiceOpen(true)} aria-haspopup="dialog" aria-label={service ? '编辑资料库服务' : '连接资料库服务'}>
              <span className="fox-settings-provider-mark"><Server /></span>
              <span className="fox-settings-provider-text"><span className="fox-settings-provider-name"><b>{service ? knowledgeServiceName(service.name) : '未连接资料库服务'}</b></span><small title={service?.baseUrl}>{service?.baseUrl ?? '连接后可使用远程专家、资料库和知识图谱'}</small></span>
              <span className="fox-settings-provider-status"><i className={cn('fox-settings-dot', serviceTone)} aria-hidden="true" /><b>{service ? serviceStatusLabel(service.lastStatus) : '未设置'}</b>{service?.lastLatencyMs != null && <><span className="is-separator">·</span><span>{service.lastLatencyMs} ms</span></>}</span>
            </button>
            {service && <Button type="button" variant="ghost" size="sm" className="fox-settings-provider-action" disabled={yuxi.testing} onClick={() => void yuxi.test().then((result) => { if (result) toast.success('资料库服务连接正常') })}>{yuxi.testing && <LoaderCircle className="animate-spin" />}测试</Button>}
          </div>
        </div>
      </SettingsGroup>

      <SettingsGroup title="账户">
        {user.user
          ? <div className="fox-settings-account-row">
            <Avatar>{user.user.avatar && <AvatarImage src={user.user.avatar} alt="" />}<AvatarFallback>{user.user.username.charAt(0).toUpperCase()}</AvatarFallback></Avatar>
            <span><b>{user.user.username}</b><small>{[user.user.uid, user.user.departmentName, user.user.role].filter(Boolean).join(' · ')}</small></span>
            <Badge variant="secondary" className="fox-settings-pill">已登录</Badge>
          </div>
          : <SettingsRow title="资料库账户" description={service ? '登录后同步你有权访问的远程专家与资料库；Fox 与浏览器的登录态相互独立' : '登录前需要先连接资料库服务'}>
            <Button type="button" variant="outline" size="sm" onClick={openLogin}><LogIn />登录账户</Button>
          </SettingsRow>}
      </SettingsGroup>
    </SettingsPageLayout>
    <StorageLocationDialog gateway={gateway} open={storageOpen} onOpenChange={setStorageOpen} />
    <YuxiServiceSheet open={serviceOpen} yuxi={yuxi} onOpenChange={setServiceOpen} />
  </WorkspacePage>
}
