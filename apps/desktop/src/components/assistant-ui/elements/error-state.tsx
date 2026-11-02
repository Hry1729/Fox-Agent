import { CircleStop } from 'lucide-react'
import { cn } from '@/lib/utils'
import './fox-elements.css'
import { ErrorState as UpstreamError } from '../upstream/elements/error-state'
import { StoppedRun as UpstreamStopped } from '../upstream/elements/stopped-run'

export function ErrorState({ title, detail, diagnostic, onRetry, retryLabel = '重试', className }: { title: string; detail?: string; diagnostic?: string; onRetry?: () => void; retryLabel?: string; className?: string }) {
  return <UpstreamError title={title} detail={detail || ''} diagnostic={diagnostic} retryLabel={retryLabel} retrying={false} onRetry={onRetry} className={cn('aui-error-state max-w-none rounded-[var(--fox-radius-card)]', className)} />
}

/** UI 1104-04：取消行用 lucide CircleStop（圆里一个方块），和思维链共用 15px 图标列与文字起点。 */
export function StoppedRun({ reason }: { reason: string }) {
  return <UpstreamStopped words={[]} reason={reason} icon={<CircleStop className="aui-stopped-run-icon" aria-hidden="true" />} className="aui-stopped-run max-w-none" role="status" />
}
