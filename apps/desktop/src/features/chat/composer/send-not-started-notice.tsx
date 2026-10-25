import { X } from 'lucide-react'

/** Longest reason shown in the collapsed summary line. */
export const SEND_FAILURE_SUMMARY_CHARS = 120
/** Longest reason shown when expanded (the Host error can be long). */
export const SEND_FAILURE_DETAIL_CHARS = 4000

export function truncateReason(reason: string, limit: number): string {
  const text = reason.trim()
  return text.length > limit ? `${text.slice(0, limit)}…` : text
}

/**
 * 「发送未开始」 notice above the composer. When the Host said why (for
 * example `run.create_failed: <路径> 应为目录但是文件`), a one-line truncated
 * reason expands into the full text.
 */
export function SendNotStartedNotice({ message, detail, onDismiss }: { message: string; detail?: string | null; onDismiss?: () => void }) {
  const reason = detail?.trim() ?? ''
  return (
    <div className="fox-send-not-started" role="alert" data-testid="fox-send-not-started">
      <div className="fox-send-not-started-head">
        <span>{message}</span>
        {onDismiss && <button type="button" aria-label="关闭提示" onClick={onDismiss}><X /></button>}
      </div>
      {reason && (
        <details className="fox-send-not-started-reason">
          <summary title={truncateReason(reason, SEND_FAILURE_DETAIL_CHARS)}>原因：{truncateReason(reason, SEND_FAILURE_SUMMARY_CHARS)}</summary>
          <pre>{truncateReason(reason, SEND_FAILURE_DETAIL_CHARS)}</pre>
        </details>
      )}
    </div>
  )
}
