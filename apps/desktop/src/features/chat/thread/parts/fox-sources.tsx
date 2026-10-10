import { type RuntimeSource } from '../../timeline/fox-runtime-process-model'
import { copyTextWithFeedback } from '../../fox-shared'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { type KnowledgeBindingRecord } from '@/features/conversations/model/types'
import { BookOpen, Copy, ExternalLink, Library } from 'lucide-react'
import { useState } from 'react'
import { useFoxThread } from '../fox-thread-context'

/**
 * Compact source row (migration step 5C): knowledge citations merge into one
 * line of small text labels — no bordered card. Clicking a label opens the same
 * source dialog the legacy timeline used (excerpt + 查看原文).
 */
export function FoxSources({ sources }: { sources: RuntimeSource[] }) {
  const fox = useFoxThread()
  const [selectedSource, setSelectedSource] = useState<RuntimeSource | null>(null)
  const selectedKnowledgeName = selectedSource
    ? fox.knowledgeBindings.find((binding) => binding.knowledgeBaseId === selectedSource.knowledgeBaseId)?.knowledgeBaseName || selectedSource.knowledgeBaseId || '知识库'
    : '知识库'
  if (!sources.length) return null
  return (
    <div className="fox-thread-sources" aria-label="知识库来源">
      <span className="fox-thread-sources-label"><BookOpen size={12} />来源</span>
      {sources.map((source, index) => (
        <button type="button" key={source.id} className="fox-thread-source-chip" onClick={() => setSelectedSource(source)} title={source.excerpt || source.title}>
          <span className="fox-thread-source-index">{index + 1}</span>{source.title}
        </button>
      ))}
      <Dialog open={Boolean(selectedSource)} onOpenChange={(open) => { if (!open) setSelectedSource(null) }}>
        <DialogContent className="fox-source-dialog">
          <DialogHeader>
            <div className="fox-source-dialog-heading"><span><BookOpen size={18} /></span><div><DialogTitle>{selectedSource?.title ?? '知识库引用'}</DialogTitle><DialogDescription>模型回答所引用的知识片段</DialogDescription></div></div>
          </DialogHeader>
          <div className="fox-source-dialog-meta">
            <span><Library size={14} />{selectedKnowledgeName}</span>
            <span><BookOpen size={14} />{selectedSource?.title ?? '原始文件'}</span>
            {selectedSource?.locator.page && <span>第 {selectedSource.locator.page} 页</span>}
          </div>
          <div className="fox-source-dialog-excerpt">{selectedSource?.excerpt || '该引用暂未返回可展示的文本片段。'}</div>
          <DialogFooter>
            <Button variant="outline" onClick={() => selectedSource?.excerpt && void copyTextWithFeedback(selectedSource.excerpt, '已复制引用内容')} disabled={!selectedSource?.excerpt}><Copy size={14} />复制引用</Button>
            <Button disabled={!selectedSource?.knowledgeBaseId || !selectedSource?.documentId || !fox.onOpenSource} onClick={() => { if (!selectedSource || !fox.onOpenSource) return; fox.onOpenSource(selectedSource); setSelectedSource(null) }}><ExternalLink size={14} />查看原文</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  )
}
