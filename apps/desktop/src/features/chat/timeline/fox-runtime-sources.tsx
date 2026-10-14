import { Source, Sources, SourcesContent, SourcesTrigger } from '@/components/ai-elements/sources'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { type KnowledgeBindingRecord } from '@/features/conversations/model/types'
import { BookOpen, ChevronDown, ChevronRight, Copy, ExternalLink, FileText, Library } from 'lucide-react'
import { useState } from 'react'
import { copyTextWithFeedback } from '../fox-shared'
import { type RuntimeSource } from './fox-runtime-process-model'

export function RuntimeSources({ sources, knowledgeBindings = [], onOpenSource }: { sources: RuntimeSource[]; knowledgeBindings?: KnowledgeBindingRecord[]; onOpenSource?: (source: RuntimeSource) => void }) {
  const [selectedSource, setSelectedSource] = useState<RuntimeSource | null>(null)
  const selectedKnowledgeName = selectedSource
    ? knowledgeBindings.find((binding) => binding.knowledgeBaseId === selectedSource.knowledgeBaseId)?.knowledgeBaseName || selectedSource.knowledgeBaseId || '资料库'
    : '资料库'
  if (!sources.length) return null
  return <>
    <Sources className="fox-sources fox-runtime-sources"><SourcesTrigger count={sources.length}>引用了 {sources.length} 个资料库来源<ChevronDown size={14} /></SourcesTrigger><SourcesContent>{sources.map((source) => <Source key={source.id} href="#" title={source.title} onClick={(event) => { event.preventDefault(); setSelectedSource(source) }}><BookOpen size={14} /><span><strong>{source.title}</strong>{source.excerpt && <small>{source.excerpt}</small>}</span><ChevronRight size={14} /></Source>)}</SourcesContent></Sources>
    <Dialog open={Boolean(selectedSource)} onOpenChange={(open) => { if (!open) setSelectedSource(null) }}>
      <DialogContent className="fox-source-dialog">
        <DialogHeader>
          <div className="fox-source-dialog-heading"><span><BookOpen size={18} /></span><div><DialogTitle>{selectedSource?.title ?? '资料库引用'}</DialogTitle><DialogDescription>模型回答所引用的知识片段</DialogDescription></div></div>
        </DialogHeader>
        <div className="fox-source-dialog-meta">
          <span><Library size={14} />{selectedKnowledgeName}</span>
          <span><FileText size={14} />{selectedSource?.title ?? '原始文件'}</span>
          {selectedSource?.locator.page && <span>第 {selectedSource.locator.page} 页</span>}
        </div>
        <div className="fox-source-dialog-excerpt">{selectedSource?.excerpt || '该引用暂未返回可展示的文本片段。'}</div>
        <DialogFooter>
          <Button variant="outline" onClick={() => selectedSource?.excerpt && void copyTextWithFeedback(selectedSource.excerpt, '已复制引用内容')} disabled={!selectedSource?.excerpt}><Copy size={14} />复制引用</Button>
          <Button disabled={!selectedSource?.knowledgeBaseId || !selectedSource?.documentId || !onOpenSource} onClick={() => { if (!selectedSource || !onOpenSource) return; onOpenSource(selectedSource); setSelectedSource(null) }}><ExternalLink size={14} />查看原文</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </>
}
