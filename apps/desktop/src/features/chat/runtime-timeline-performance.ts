import type { ArtifactRecord, RunEventRecord } from '@/features/conversations/model/types'

export const EMPTY_RUNTIME_EVENTS: RunEventRecord[] = []
export const EMPTY_RUNTIME_ARTIFACTS: ArtifactRecord[] = []

function sameReferenceArray<T>(previous: T[] | undefined, next: T[]) {
  return Boolean(previous && previous.length === next.length && next.every((item, index) => previous[index] === item))
}

/**
 * Groups records while retaining the previous bucket when its item references
 * did not change. This keeps completed message rows memoizable during a stream.
 */
export function groupRuntimeRecords<T extends { runId?: string | null }>(records: readonly T[], previous: Map<string, T[]>) {
  const grouped = new Map<string, T[]>()
  for (const record of records) {
    if (!record.runId) continue
    const bucket = grouped.get(record.runId)
    if (bucket) bucket.push(record)
    else grouped.set(record.runId, [record])
  }
  const stable = new Map<string, T[]>()
  for (const [runId, bucket] of grouped) {
    const previousBucket = previous.get(runId)
    stable.set(runId, sameReferenceArray(previousBucket, bucket) ? previousBucket! : bucket)
  }
  return stable
}
