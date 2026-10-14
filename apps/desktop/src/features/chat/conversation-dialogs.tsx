import { filterKnowledgePickerItems, localKnowledgePickerState } from './knowledge-picker-state'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Tabs, TabsContent, TabsList, TabsTrigger } from '@/components/ui/tabs'
import { type LocalKnowledgeBaseDto, knowledgeReferenceFromLegacyBinding, knowledgeReferenceKey } from '@/features/conversations/api/desktop-client'
import { type KnowledgeBaseRecord, type KnowledgeBindingRecord, type KnowledgeReference } from '@/features/conversations/model/types'
import { AlertTriangle, CircleHelp, LoaderCircle, Search } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'

type KnowledgePickerItem = {
  reference: KnowledgeReference
  name: string
  description: string
  source: 'local' | 'remote'
  status?: string | null
  unavailable?: boolean
}

export function KnowledgeBindingDialog({ open, remoteItems, localItems, localLoading, remoteLoading, remoteError, onRetry, bindings, references, busy, error, onOpenChange, onConfirm }: {
  open: boolean
  remoteItems: KnowledgeBaseRecord[]
  localItems: LocalKnowledgeBaseDto[]
  localLoading: boolean
  remoteLoading: boolean
  remoteError?: string | null
  onRetry: () => void
  bindings: KnowledgeBindingRecord[]
  references?: KnowledgeReference[]
  busy: boolean
  error?: string | null
  onOpenChange: (open: boolean) => void
  onConfirm: (references: KnowledgeReference[], names: Record<string, string>) => void
}) {
  const [selected, setSelected] = useState<string[]>([])
  const [source, setSource] = useState<'local' | 'remote'>('local')
  const [query, setQuery] = useState('')
  const configuredReferences = Array.isArray(references)
    ? references
    : bindings.filter((item) => item.enabled).map(knowledgeReferenceFromLegacyBinding)
  const wasOpen = useRef(false)

  useEffect(() => {
    if (open && !wasOpen.current) setSelected(configuredReferences.map(knowledgeReferenceKey))
    wasOpen.current = open
  }, [bindings, open, references])

  useEffect(() => {
    if (!open) return
    setSource('local')
    setQuery('')
  }, [open])

  const pickerItems: KnowledgePickerItem[] = [
    ...remoteItems.map((item) => ({
      reference: {
        source: 'remote' as const,
        providerKey: 'yuxi-primary',
        connectionId: 'yuxi-primary',
        id: item.id,
      },
      name: item.name,
      description: item.description || `${item.fileCount} 个文件`,
      source: 'remote' as const,
      status: item.status,
    })),
    ...localItems.map((item) => ({
      reference: {
        source: 'local' as const,
        providerKey: 'local' as const,
        id: item.id,
      },
      name: item.name,
      description: [item.description || `${item.documentCount} 个文档`, localKnowledgePickerState(item).reason].filter(Boolean).join(' · '),
      source: 'local' as const,
      status: localKnowledgePickerState(item).status,
      unavailable: !localKnowledgePickerState(item).available,
    })),
  ]
  const knownReferenceKeys = new Set(pickerItems.map((item) => knowledgeReferenceKey(item.reference)))
  pickerItems.push(...configuredReferences
    .filter((reference) => !knownReferenceKeys.has(knowledgeReferenceKey(reference)))
    .map((reference) => ({
      reference,
      name: reference.id,
      description: '当前绑定但暂时无法从目录读取',
      source: reference.source,
      status: '不可用',
      unavailable: true,
    })))
  const itemByKey = new Map(pickerItems.map((item) => [knowledgeReferenceKey(item.reference), item]))
  const search = query.trim().toLocaleLowerCase()
  const visibleItems = filterKnowledgePickerItems(pickerItems, source, query)
  const loading = source === 'local' ? localLoading : remoteLoading
  const toggle = (reference: KnowledgeReference) => {
    const key = knowledgeReferenceKey(reference)
    setSelected((current) => current.includes(key)
      ? current.filter((item) => item !== key)
      : [...current, key])
  }
  const confirm = () => {
    const names: Record<string, string> = {}
    const selectedReferences = selected.map((key) => {
      const item = itemByKey.get(key)
      if (item) names[key] = item.name
      return item?.reference
    }).filter((reference): reference is KnowledgeReference => Boolean(reference))
    onConfirm(selectedReferences, names)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="fox-knowledge-binding-dialog">
        <DialogHeader>
          <DialogTitle>选择资料库</DialogTitle>
          <DialogDescription>仅所选资料库可用于当前对话。</DialogDescription>
        </DialogHeader>
        <Tabs value={source} onValueChange={(value) => setSource(value as 'local' | 'remote')} className="fox-knowledge-picker-tabs">
          <TabsList aria-label="资料库来源" className="fox-knowledge-picker-sources">
            <TabsTrigger value="local">本机 · {localItems.length}</TabsTrigger>
            <TabsTrigger value="remote">远程 · {remoteLoading ? '读取中' : remoteError ? '未连接' : remoteItems.length}{remoteError && <AlertTriangle aria-hidden="true" className="fox-knowledge-connection-warning" />}</TabsTrigger>
          </TabsList>
          <label className="fox-knowledge-picker-search">
            <Search aria-hidden="true" />
            <Input aria-label="搜索资料库" placeholder="搜索资料库" value={query} onChange={(event) => setQuery(event.target.value)} />
          </label>
          <TabsContent value={source} className="fox-knowledge-picker-panel">
            {source === 'remote' && remoteError && <div role="status" className="fox-knowledge-picker-notice"><AlertTriangle aria-hidden="true" /><div><strong>远程连接暂不可用</strong><p>{remoteError}</p><p>本机资料库仍可使用。</p></div><Button variant="ghost" disabled={remoteLoading} onClick={onRetry}>重试</Button></div>}
            <div className="fox-knowledge-picker-list" aria-label={source === 'local' ? '本机资料库' : '远程资料库'} aria-busy={loading}>
              {visibleItems.map((item) => {
                const key = knowledgeReferenceKey(item.reference)
                const checked = selected.includes(key)
                return <label key={key} className={`fox-knowledge-picker-row${checked ? ' is-checked' : ''}${item.unavailable ? ' is-unavailable' : ''}`}>
                  <input type="checkbox" checked={checked} disabled={busy || (item.unavailable && !checked)} onChange={() => toggle(item.reference)} aria-label={item.name} />
                  <span className="fox-knowledge-picker-copy"><strong>{item.name}</strong><span>{item.description}{!item.unavailable && item.status ? ` · ${item.status}` : ''}</span></span>
                  {item.unavailable && <span className="fox-knowledge-picker-status">{item.status}</span>}
                </label>
              })}
              {!visibleItems.length && <p className="fox-knowledge-picker-empty" role="status">{loading ? '正在读取资料库…' : search ? '没有匹配的资料库，请换个关键词。' : source === 'local' ? '暂无本机资料库，请先在资料库页面导入。' : remoteError ? '连接恢复后可查看远程资料库。' : '暂无可访问的远程资料库。'}</p>}
            </div>
            {source === 'local' && <div className="fox-knowledge-picker-hint"><Popover><PopoverTrigger asChild><Button type="button" variant="ghost" size="sm" aria-label="为什么资料库暂不可选"><CircleHelp aria-hidden="true" />选择条件</Button></PopoverTrigger><PopoverContent className="max-w-80 text-sm" align="start"><strong>导入文件后，等待整理完成即可选择</strong><p>按字词查找可用的资料库也能加入聊天，无须安装向量模型。若显示未整理、暂停或失败，请到资料库页面查看任务进度和错误原因。</p></PopoverContent></Popover><span>有可检索内容即可加入聊天。</span><Button type="button" variant="ghost" size="sm" disabled={loading} onClick={onRetry}>刷新</Button></div>}
          </TabsContent>
        </Tabs>
        {error && <p role="alert" className="fox-knowledge-picker-error">{error}</p>}
        <DialogFooter className="fox-knowledge-picker-footer"><span aria-live="polite">已选 <strong>{selected.length}</strong> 个</span><div><Button variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>取消</Button><Button disabled={busy} onClick={confirm}>{busy && <LoaderCircle className="animate-spin" />}保存选择</Button></div></DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

export function ConversationManagementDialogs({ conversation, mode, busy, error, onClose, onRename, onDelete }: { conversation: { id: string; title: string } | null; mode: 'rename' | 'purge' | null; busy: boolean; error: string | null; onClose: () => void; onRename: (title: string) => void; onDelete: () => void }) {
  const [title, setTitle] = useState('')
  useEffect(() => { if (conversation && mode === 'rename') setTitle(conversation.title) }, [conversation, mode])
  return <>
    <Dialog open={mode === 'rename'} onOpenChange={(open) => { if (!open) onClose() }}>
      <DialogContent className="fox-conversation-dialog">
        <DialogHeader><DialogTitle>重命名</DialogTitle><DialogDescription>新的标题只改变显示名称，不会改变当前对话绑定的 Agent 或 Runtime。</DialogDescription></DialogHeader>
        <Input autoFocus value={title} maxLength={120} onChange={(event) => setTitle(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter' && title.trim() && !busy) onRename(title.trim()) }} />
        <DialogFooter><Button variant="outline" onClick={onClose}>取消</Button><Button disabled={busy || !title.trim()} onClick={() => onRename(title.trim())}>{busy && <LoaderCircle className="animate-spin" />}保存</Button></DialogFooter>
      </DialogContent>
    </Dialog>
    <Dialog open={mode === 'purge'} onOpenChange={(open) => { if (!open) onClose() }}>
      <DialogContent className="fox-conversation-dialog fox-delete-dialog">
        <DialogHeader><DialogTitle>永久删除对话？</DialogTitle><DialogDescription>“{conversation?.title}”及其消息、工具与审批记录、Fox 内部附件和未导出产物、Runtime Session，以及对应的远程线程（如有）将被永久删除。已导出到项目目录的文件不会受影响。此操作无法撤销。</DialogDescription></DialogHeader>
        {error && <p className="fox-conversation-dialog-error" role="alert">{error}</p>}
        <DialogFooter><Button variant="outline" onClick={onClose}>取消</Button><Button variant="destructive" disabled={busy} onClick={onDelete}>{busy && <LoaderCircle className="animate-spin" />}永久删除</Button></DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}

export function ProjectDeleteDialog({ project, busy, onClose, onDelete }: { project: { name: string; root: string; items: Array<{ id: string; title: string }> } | null; busy: boolean; onClose: () => void; onDelete: () => void }) {
  return <Dialog open={Boolean(project)} onOpenChange={(open) => { if (!open) onClose() }}>
    <DialogContent className="fox-conversation-dialog">
      <DialogHeader>
        <DialogTitle>删除项目？</DialogTitle>
        <DialogDescription>“{project?.name}”及其 {project?.items.length ?? 0} 个对话、消息、工具记录和 Fox 内部生成的文件将被删除。这里只会移除 Fox 中的项目记录，不会删除本机文件夹“{project?.root}”。此操作无法撤销。</DialogDescription>
      </DialogHeader>
      <DialogFooter><Button variant="outline" onClick={onClose}>取消</Button><Button variant="destructive" disabled={busy} onClick={onDelete}>{busy && <LoaderCircle className="animate-spin" />}删除项目</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}
