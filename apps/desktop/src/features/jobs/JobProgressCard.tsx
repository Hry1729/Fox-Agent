import { useState } from 'react'
import { Ban, Eye, Loader2 } from 'lucide-react'
import { cn } from '@/lib/utils'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  canViewResult,
  isCancellable,
  isTerminal,
  kindLabel,
  progressRatio,
  progressText,
  stateLabel,
  type JobSnapshot,
} from './job-presentation'

export interface JobProgressCardProps {
  snapshot: JobSnapshot
  className?: string
  /** Cancel the job through the Host lifecycle; resolves when the Host answered. */
  onCancel?: (jobId: string) => Promise<void> | void
  /** Open the paged result view for a completed job (fox-result reference). */
  onViewResult?: (jobId: string, resultRef: string) => void
}

/**
 * Background compute job card: live progress, cancel, and the entry to page
 * through a completed result. Data and actions arrive as props so the card
 * never reaches around the Host lifecycle (wired by window A).
 */
export function JobProgressCard({ snapshot, className, onCancel, onViewResult }: JobProgressCardProps) {
  const [cancelling, setCancelling] = useState(false)
  const [cancelError, setCancelError] = useState<string | null>(null)
  const ratio = progressRatio(snapshot)
  const terminal = isTerminal(snapshot.state)

  const cancel = async () => {
    if (!onCancel || !isCancellable(snapshot.state)) return
    setCancelling(true)
    setCancelError(null)
    try {
      await onCancel(snapshot.jobId)
    } catch (cause) {
      setCancelError(cause instanceof Error ? cause.message : String(cause))
    } finally {
      setCancelling(false)
    }
  }

  return (
    <div className={cn('fox-job-card border rounded-lg bg-card p-3 space-y-2', className)}>
      <div className="flex items-center gap-2">
        {!terminal && <Loader2 className="h-4 w-4 animate-spin text-muted-foreground" aria-hidden />}
        <span className="text-sm font-medium">{kindLabel(snapshot.kind)}</span>
        <Badge variant={snapshot.state === 'failed' ? 'destructive' : terminal ? 'secondary' : 'default'}>
          {snapshot.cancelRequestedAt && !terminal ? '正在停止' : stateLabel(snapshot.state)}
        </Badge>
        <span className="ml-auto text-xs text-muted-foreground">作业 {snapshot.jobId.slice(0, 8)}</span>
      </div>

      <div className="space-y-1">
        <div className="h-2 w-full rounded bg-muted" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={ratio === null ? undefined : Math.round(ratio * 100)}>
          <div
            className={cn('h-2 rounded bg-primary transition-[width]', ratio === null && 'animate-pulse w-1/3')}
            style={ratio === null ? undefined : { width: `${Math.round(ratio * 100)}%` }}
          />
        </div>
        <div className="text-xs text-muted-foreground">{progressText(snapshot)}</div>
      </div>

      {snapshot.state === 'failed' && snapshot.errorMessage && (
        <div className="text-xs text-destructive">
          {snapshot.errorCode ? `[${snapshot.errorCode}] ` : ''}
          {snapshot.errorMessage}
        </div>
      )}
      {snapshot.state === 'cancelled' && snapshot.progressDone > 0 && (
        <div className="text-xs text-muted-foreground">已记录取消前的处理进度；未发布完整结果。重新计算会使用独立工作目录。</div>
      )}
      {cancelError && <div className="text-xs text-destructive">取消失败：{cancelError}</div>}

      <div className="flex gap-2">
        {isCancellable(snapshot.state) && onCancel && (
          <Button size="sm" variant="outline" onClick={cancel} disabled={cancelling}>
            <Ban className="h-3.5 w-3.5 mr-1" />
            {cancelling ? '取消中…' : '取消'}
          </Button>
        )}
        {canViewResult(snapshot) && onViewResult && (
          <Button size="sm" variant="secondary" onClick={() => onViewResult(snapshot.jobId, snapshot.resultRef!)}>
            <Eye className="h-3.5 w-3.5 mr-1" />
            查看结果
          </Button>
        )}
      </div>
    </div>
  )
}
