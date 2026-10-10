import { type DataMessagePartProps } from '@assistant-ui/react'
import { RuntimeArtifacts } from '../../runtime-artifacts'
import type { ArtifactRecord } from '@/features/conversations/model/types'
import { useFoxThread } from '../fox-thread-context'

/**
 * The `data-fox-artifacts` part renderer: the same file-result groups the
 * legacy timeline used (本次文件结果 / 过程文件), opened via the host's
 * artifact handler.
 */
export function FoxArtifactsPart(part: DataMessagePartProps<ArtifactRecord[]>) {
  const fox = useFoxThread()
  if (!part.data?.length) return null
  return <RuntimeArtifacts artifacts={part.data} onOpenArtifact={fox.onOpenArtifact} />
}
