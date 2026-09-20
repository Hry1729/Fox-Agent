import { useEffect, useState } from 'react'
import { convertFileSrc, invoke } from '@tauri-apps/api/core'
import { Button } from '@/components/ui/button'

export function HtmlFilePreview({ content, name }: { content: string; name: string }) {
  const [source, setSource] = useState(false)
  const [url, setUrl] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  useEffect(() => {
    let disposed = false
    let previewId: string | null = null
    setUrl(null)
    setError(null)
    void invoke<string>('html_preview_create', { content }).then(id => {
      if (disposed) { void invoke('html_preview_release', { id }).catch(() => {}); return }
      previewId = id
      setUrl(`${convertFileSrc(id, 'fox-preview')}/frame`)
    }).catch(() => { if (!disposed) setError('网页预览暂不可用，请查看源码。') })
    return () => { disposed = true; if (previewId) void invoke('html_preview_release', { id: previewId }).catch(() => {}) }
  }, [content])
  return <div className="fox-html-file-preview">
    <div className="fox-html-preview-toolbar">
      <span>网页预览</span>
      <Button variant="outline" size="sm" onClick={() => setSource(value => !value)}>
        {source ? '查看网页' : '查看源码'}
      </Button>
    </div>
    {source ? <pre>{content}</pre> : error ? <p role="status">{error}</p> : !url ? <p role="status">正在准备网页预览…</p> : <iframe
      title={`${name} 网页预览`}
      sandbox="allow-scripts"
      referrerPolicy="no-referrer"
      src={url}
    />}
  </div>
}
