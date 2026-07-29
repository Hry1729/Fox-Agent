import { useMemo, useState } from 'react'
import { Bookmark, BookmarkCheck, FileClock, MessageSquareText, Trash2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverHeader, PopoverTitle, PopoverTrigger } from '@/components/ui/popover'
import { Textarea } from '@/components/ui/textarea'
import type { KnowledgeDocumentActivity } from '@/features/conversations/model/types'
import { activityIsStale, type DocumentViewPosition } from './document-activity-model'

export function KnowledgeDocumentActivityToolbar({
  activity,
  sourceRevision,
  position,
  loading,
  error,
  onNavigate,
  onAddBookmark,
  onRemoveBookmark,
  onAddAnnotation,
  onRemoveAnnotation,
}: {
  activity: KnowledgeDocumentActivity
  sourceRevision: string | null
  position: DocumentViewPosition
  loading: boolean
  error: string | null
  onNavigate(position: DocumentViewPosition): void
  onAddBookmark(position: DocumentViewPosition): Promise<unknown>
  onRemoveBookmark(id: string): Promise<unknown>
  onAddAnnotation(position: DocumentViewPosition, note: string): Promise<unknown>
  onRemoveAnnotation(id: string): Promise<unknown>
}) {
  const [note, setNote] = useState('')
  const [saving, setSaving] = useState(false)
  const currentBookmark = useMemo(
    () => activity.bookmarks.find((item) => item.page === position.page && !activityIsStale(item, sourceRevision)),
    [activity.bookmarks, position.page, sourceRevision],
  )
  const saveNote = async () => {
    const value = note.trim()
    if (!value || saving) return
    setSaving(true)
    try {
      await onAddAnnotation(position, value)
      setNote('')
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="fox-document-activity-toolbar">
      <span className="fox-document-reading-indicator"><FileClock />{loading ? '正在恢复阅读位置' : `第 ${position.page} 页`}</span>
      <Button
        type="button"
        variant="ghost"
        size="sm"
        disabled={loading}
        onClick={() => void (currentBookmark ? onRemoveBookmark(currentBookmark.id) : onAddBookmark(position))}
      >
        {currentBookmark ? <BookmarkCheck /> : <Bookmark />}
        {currentBookmark ? '已加书签' : '添加书签'}
      </Button>
      <Popover>
        <PopoverTrigger asChild>
          <Button type="button" variant="ghost" size="sm"><Bookmark />书签 {activity.bookmarks.length}</Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="fox-document-activity-popover">
          <PopoverHeader><PopoverTitle>文档书签</PopoverTitle></PopoverHeader>
          <div className="fox-document-activity-list">
            {activity.bookmarks.map((bookmark) => {
              const stale = activityIsStale(bookmark, sourceRevision)
              return <div key={bookmark.id}>
                <button type="button" disabled={stale} onClick={() => onNavigate({ page: bookmark.page })}>
                  <b>{bookmark.label || `第 ${bookmark.page} 页`}</b>
                  <small>{stale ? '源文件已更新，位置需要重新确认' : formatActivityTime(bookmark.updatedAt)}</small>
                </button>
                <Button variant="ghost" size="icon-sm" aria-label="删除书签" onClick={() => void onRemoveBookmark(bookmark.id)}><Trash2 /></Button>
              </div>
            })}
            {!activity.bookmarks.length && <p>还没有书签。</p>}
          </div>
        </PopoverContent>
      </Popover>
      <Popover>
        <PopoverTrigger asChild>
          <Button type="button" variant="ghost" size="sm"><MessageSquareText />笔记 {activity.annotations.length}</Button>
        </PopoverTrigger>
        <PopoverContent align="start" className="fox-document-activity-popover is-notes">
          <PopoverHeader><PopoverTitle>阅读笔记</PopoverTitle></PopoverHeader>
          <Textarea value={note} onChange={(event) => setNote(event.target.value)} maxLength={32_000} placeholder={`记录第 ${position.page} 页的想法`} />
          <Button size="sm" disabled={!note.trim() || saving} onClick={() => void saveNote()}>{saving ? '保存中' : '保存笔记'}</Button>
          <div className="fox-document-activity-list">
            {activity.annotations.map((annotation) => {
              const stale = activityIsStale(annotation, sourceRevision)
              return <div key={annotation.id}>
                <button type="button" disabled={stale} onClick={() => onNavigate({ page: annotation.page })}>
                  <b>第 {annotation.page} 页</b>
                  <span>{annotation.note}</span>
                  <small>{stale ? '源文件已更新，位置需要重新确认' : formatActivityTime(annotation.updatedAt)}</small>
                </button>
                <Button variant="ghost" size="icon-sm" aria-label="删除笔记" onClick={() => void onRemoveAnnotation(annotation.id)}><Trash2 /></Button>
              </div>
            })}
            {!activity.annotations.length && <p>还没有阅读笔记。</p>}
          </div>
        </PopoverContent>
      </Popover>
      {error && <small className="fox-document-activity-error" title={error}>阅读记录暂未同步</small>}
    </div>
  )
}

function formatActivityTime(value: number) {
  return new Date(value).toLocaleString()
}
