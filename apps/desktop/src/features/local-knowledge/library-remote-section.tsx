import FolderComponent from '@/components/ui/folder-component'
import { Cloud, CloudOff, LoaderCircle, RefreshCw, WifiOff } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useYuxiService } from '@/features/settings/use-yuxi-service'
import { useYuxiUser } from '@/features/settings/use-yuxi-user'
import { useKnowledgeBases } from '@/features/knowledge/use-knowledge'
import { YUXI_LOGIN_RETURN_KEY, YUXI_SERVICE_RETURN_KEY } from '@/features/knowledge/knowledge-navigation'
import type { NavigateWorkspace } from '@/features/workspace/types'
import { Progress } from '@/components/ui/progress'

export function LibraryRemoteSection({ query, navigate }: { query: string; navigate?: NavigateWorkspace }) {
  const yuxi = useYuxiService()
  const user = useYuxiUser(Boolean(yuxi.service?.credentialConfigured))
  const resource = useKnowledgeBases(Boolean(user.user))
  const needle = query.trim().toLocaleLowerCase()
  const items = resource.items.filter((item) => !needle || `${item.name} ${item.description}`.toLocaleLowerCase().includes(needle))
  const configured = Boolean(yuxi.service?.baseUrl)
  const error = yuxi.error || user.error || resource.error
  const busy = yuxi.loading || user.loading || resource.loading
  const connect = () => {
    window.sessionStorage.setItem(configured ? YUXI_LOGIN_RETURN_KEY : YUXI_SERVICE_RETURN_KEY, 'local-knowledge')
    navigate?.(configured ? 'login' : 'service')
  }
  const refresh = () => { void yuxi.refresh(); void user.refresh(); void resource.refresh() }
  return <section className="fox-library-source-section" aria-label="远程资料库">
    <h2 className="fox-library-group-title"><Cloud />远程 · Yuxi{user.user && <small>{resource.items.length}</small>}</h2>
    {busy && <div className="fox-library-remote-state" role="status"><LoaderCircle className="animate-spin" /><span>正在连接 Yuxi</span></div>}
    {!busy && (error || !user.user) && <div className={`fox-library-remote-state ${error ? 'is-error' : ''}`} role={error ? 'alert' : 'status'}>{error ? <WifiOff /> : <CloudOff />}<div><b>{error ? '连不上 Yuxi' : configured ? '登录 Yuxi 后查看远程资料库' : '还没有连接 Yuxi'}</b><p>{error ? `${error}。本机资料库不受影响。` : '连接后可在这里访问有权限的远程资料库。'}</p></div>{error && <Button variant="ghost" onClick={() => { window.sessionStorage.setItem(YUXI_SERVICE_RETURN_KEY, 'local-knowledge'); navigate?.('service') }}>连接设置</Button>}<Button variant="outline" onClick={error ? refresh : connect}>{error && <RefreshCw />}{error ? '重试' : configured ? '登录' : '连接'}</Button></div>}
    {!busy && !error && user.user && <div className="fox-library-card-grid">{items.map((item) => {
      const progress = item.fileCount > 0 ? Math.min(100, Math.round(item.processedCount / item.fileCount * 100)) : 0
      const processing = item.fileCount > 0 && item.processedCount < item.fileCount
      return <button type="button" className="fox-library-base-card" key={item.id} onClick={() => navigate?.('knowledge-detail', item.id)}><span className="fox-kb-cover" aria-hidden="true"><FolderComponent color="theme" size="xs" /></span><div className="fox-library-card-heading"><b>{item.name}</b><span className="fox-library-source-label"><Cloud />远程</span></div><p>{item.description?.trim() || '无简介'}</p><div className="fox-library-card-meta">{processing ? <><span>已处理 {item.processedCount} / {item.fileCount}</span><Progress value={progress} aria-label={`${item.name} 已处理比例`} /><span>{progress}%</span></> : <><span>{item.fileCount} 份资料</span>{item.status === 'error' && <span>处理失败</span>}</>}</div></button>
    })}{!items.length && <p className="fox-library-inline-empty">{needle ? '没有匹配的远程资料库' : '还没有可访问的远程资料库'}</p>}</div>}
  </section>
}
