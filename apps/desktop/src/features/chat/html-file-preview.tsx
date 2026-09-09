import { useState } from 'react'
import { Button } from '@/components/ui/button'

export function HtmlFilePreview({ content, name }: { content: string; name: string }) {
  const [source, setSource] = useState(false)
  return <div className="fox-html-file-preview">
    <div className="fox-html-preview-toolbar">
      <span>网页预览</span>
      <Button variant="outline" size="sm" onClick={() => setSource(value => !value)}>
        {source ? '查看网页' : '查看源码'}
      </Button>
    </div>
    {source ? <pre>{content}</pre> : <iframe
      title={`${name} 网页预览`}
      sandbox="allow-scripts"
      referrerPolicy="no-referrer"
      srcDoc={content}
    />}
  </div>
}
