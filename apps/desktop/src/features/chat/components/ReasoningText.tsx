import { memo, useState } from 'react'

const PAGE_SIZE = 4000
function pageBoundary(text: string, offset: number) {
  const value = text.charCodeAt(offset)
  const before = text.charCodeAt(offset - 1)
  return value >= 0xdc00 && value <= 0xdfff && before >= 0xd800 && before <= 0xdbff ? offset - 1 : offset
}

/** Reasoning is source text; incomplete Markdown links must not swallow it.
 * A bounded page also avoids reparsing the entire history on every stream delta.
 */
export const ReasoningText = memo(function ReasoningText({ text }: { text: string }) {
  const [page, setPage] = useState(0)
  const pages = Math.max(1, Math.ceil(text.length / PAGE_SIZE))
  const current = Math.min(page, pages - 1)
  const start = current * PAGE_SIZE
  return <div className="fox-reasoning-text">
    {pages > 1 && <div className="fox-reasoning-pagination">
      <span>思考记录 · {current + 1} / {pages} 页</span>
      <button type="button" disabled={current === 0} onClick={() => setPage(current - 1)}>上一页</button>
      <button type="button" disabled={current === pages - 1} onClick={() => setPage(current + 1)}>下一页</button>
    </div>}
    <pre>{text.slice(pageBoundary(text, start), pageBoundary(text, start + PAGE_SIZE))}</pre>
  </div>
})
