import { useAui } from '@assistant-ui/react'
import { ErrorState } from '@/components/assistant-ui/elements/error-state'
import type { DataMessagePartProps } from '@assistant-ui/react'
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
  return <ErrorState className="fox-thread-error" title={failure.kind === 'interrupted' ? '运行中断' : '运行失败'} detail={failure.message} onRetry={() => aui.message.reload()} />
}
