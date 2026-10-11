import { useAuiState } from '@assistant-ui/react'
import type { DataMessagePartProps } from '@assistant-ui/react'
import { type FoxProcessData } from '../convert-detail'
import { useFoxThread } from '../fox-thread-context'
import { RuntimeProcess } from '../../timeline/fox-runtime-process-view'
import { FoxAssistantAvatar } from '../../components/FoxAssistantAvatar'
import { FoxSources } from './fox-sources'

/**
 * The `data-fox-process` part renderer (migration step 5C): the same process
 * rows the legacy timeline used, one collapsible group per run. Live state
 * (active tool, streaming thought) derives from the run's raw events, which the
 * thread provider groups by run id; the part id is the run id.
 *
 * A process part without events is an inline `<think>` block from history: it
 * still renders as a settled reasoning row from the part data alone.
 */
export function FoxProcessPart(part: DataMessagePartProps<FoxProcessData>) {
  const fox = useFoxThread()
  const data = part.data
  const events = fox.eventsByRun.get(part.id ?? '') ?? []
  const threadRunning = useAuiState((state) => state.thread.isRunning)
  const isLast = useAuiState((state) => state.optional.message?.isLast ?? false)
  const status = useAuiState((state) => state.optional.message?.status)
  const running = threadRunning && isLast
  const outcome = status?.type === 'incomplete' && status.reason === 'error'
    ? 'failed' as const
    : status?.type === 'incomplete' && status.reason === 'cancelled'
      ? 'cancelled' as const
      : undefined
  const hasEventData = events.length > 0
  if (!hasEventData && !(data?.reasoning || data?.tools?.length || data?.sources?.length)) return null
  return (
    <div className="fox-thread-process">
      <RuntimeProcess
        events={events}
        process={hasEventData ? undefined : data}
        groupHelpers={fox.groupHelpers}
        grouped
        running={running}
        runInProgress={running}
        outcome={outcome}
        /* The avatar leads the process head line, the same visual relation as
         * the legacy TurnProcessHeader (avatar + status on one row). */
        leading={<FoxAssistantAvatar />}
        attachmentNames={fox.attachmentNames}
        onOpenFileInSidebar={fox.onOpenFileInSidebar}
        onRevealFileInExplorer={fox.onRevealFileInExplorer}
      />
      {Boolean(data?.sources?.length) && <FoxSources sources={data.sources} />}
    </div>
  )
}
