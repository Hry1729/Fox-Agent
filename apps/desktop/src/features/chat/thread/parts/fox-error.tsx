import { useAui } from '@assistant-ui/react'
import type { DataMessagePartProps } from '@assistant-ui/react'
import { Button } from '@/components/ui/button'
import { AlertTriangle, RotateCcw } from 'lucide-react'
import type { RunFailure } from '../../../conversations/model/run-failure'

/**
 * The `data-fox-error` part renderer: one quiet failure line per run — what
 * happened and a retry. The retry re-runs the latest run through the runtime's
 * reload path (`retryLatestRun` on the Fox side).
 */
export function FoxErrorPart(part: DataMessagePartProps<RunFailure>) {
  const aui = useAui()
  const failure = part.data
  if (!failure) return null
  return (
    <div className="fox-thread-error" role="alert">
      <span className="fox-thread-error-icon"><AlertTriangle size={14} /></span>
      <span className="fox-thread-error-text">
        {failure.kind === 'interrupted' ? '运行中断' : '运行失败'}
        {failure.message && failure.message !== '运行失败' ? ` · ${failure.message}` : ''}
      </span>
      <Button variant="outline" size="sm" className="fox-thread-error-retry" onClick={() => aui.message.reload()}>
        <RotateCcw size={13} />重试
      </Button>
    </div>
  )
}
