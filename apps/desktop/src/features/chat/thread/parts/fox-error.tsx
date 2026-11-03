import { useAui } from '@assistant-ui/react'
import { ErrorState } from '@/components/assistant-ui/elements/error-state'
import type { DataMessagePartProps } from '@assistant-ui/react'
import { runFailureDetailText, runFailureUserMessage, type RunFailure } from '../../../conversations/model/run-failure'
import { useDeveloperUi } from '@/features/chat/developer-ui'

/**
 * The `data-fox-error` part renderer: one quiet failure line per run — what
 * happened and a retry. The retry re-runs the latest run through the runtime's
 * reload path (`retryLatestRun` on the Fox side).
 */
export function FoxErrorPart(part: DataMessagePartProps<RunFailure>) {
  const aui = useAui()
  const developerUi = useDeveloperUi()
  const failure = part.data
  if (!failure) return null
  // 13.38（R1-3 / B4-2）：普通模式只显示按错误码映射的中文提示；错误码、英文原文和后端诊断
  // 只在开发者模式的「查看错误详情」里。
  return <ErrorState className="fox-thread-error" title={failure.kind === 'interrupted' ? '运行中断' : '运行失败'}
    detail={developerUi ? failure.message : runFailureUserMessage(failure)}
    diagnostic={developerUi ? runFailureDetailText(failure) : undefined} onRetry={() => aui.message.reload()} />
}
