// Pure model for Host-managed file versions: grouping, naming and fingerprints.
// Every row is appended by the Rust Host around an intercepted write
// (write_file/edit_file/fox-office). The Host owns hashes, drift detection and
// the append-only registry; this module only reads the records.
import type { ManagedFileVersion } from '@/features/conversations/api/desktop-client'
import type { ConversationDetail } from '@/features/conversations/model/types'

export interface ManagedFileGroup {
  storagePath: string
  displayName: string
  /** The file's CURRENT bytes no longer match the newest registry version. */
  drifted: boolean
  deleted: boolean
  versions: ManagedFileVersion[] // newest version_no first
}

export const KIND_COPY: Record<string, string> = {
  created: '新建',
  modified: '修改',
  restored: '恢复版本',
  // Content a restore overwrote that the registry did not know about (an edit
  // made outside any recorded task). It is preserved as its own version, so the
  // user can select and bring it back.
  replaced: '被覆盖内容',
}

export function groupManagedFileVersions(rows: ManagedFileVersion[]): ManagedFileGroup[] {
  const map = new Map<string, ManagedFileGroup>()
  for (const row of rows) {
    let group = map.get(row.storagePath)
    if (!group) {
      group = { storagePath: row.storagePath, displayName: row.displayName, drifted: false, deleted: false, versions: [] }
      map.set(row.storagePath, group)
    }
    group.versions.push(row)
  }
  for (const group of map.values()) {
    group.versions.sort((a, b) => b.versionNo - a.versionNo)
    const latest = group.versions[0]
    group.drifted = latest?.drifted ?? false
    group.deleted = latest?.drifted === true && latest.currentHash == null
  }
  return [...map.values()].sort((a, b) => (b.versions[0]?.createdAt ?? 0) - (a.versions[0]?.createdAt ?? 0))
}

/** Stable identity of the loaded version set. A stored fingerprint that differs
 *  from this one means the user has not seen these versions yet. */
export function managedFilesFingerprint(groups: ManagedFileGroup[]): string {
  return groups
    .map(group => `${group.storagePath}#${group.versions[0]?.id ?? ''}#${group.versions.length}#${group.drifted ? 1 : 0}#${group.deleted ? 1 : 0}`)
    .join('|')
}

export function managedFilePathKey(path: string): string {
  // Host canonicalization can add the Windows verbatim path prefix.
  const normalized = path.trim().replace(/\\/g, '/')
    .replace(/^\/\/\?\/UNC\//i, '//')
    .replace(/^\/\/\?\/(?=[a-z]:\/)/i, '')
    .replace(/^\.\//, '')
  const unc = normalized.startsWith('//')
  const key = (unc ? '/' : '') + normalized.replace(/\/{2,}/g, '/').replace(/\/$/, '')
  return unc || /^[a-z]:\//i.test(key) ? key.toLowerCase() : key
}

export function formatVersionBytes(value: number | null): string {
  if (value == null) return ''
  if (value < 1024) return `${value} B`
  if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`
  return `${(value / (1024 * 1024)).toFixed(2)} MB`
}

/** Compact version timestamp: `HH:mm` for today, `MM-DD HH:mm` before that —
 *  the row already sits inside a 今天/本周/更早 time group. Formatted manually so
 *  the output never depends on the environment's locale data. */
export function formatVersionTime(at: number, now = Date.now()): string {
  const date = new Date(at)
  const pad = (value: number) => String(value).padStart(2, '0')
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}`
  const sameDay = date.toDateString() === new Date(now).toDateString()
  if (sameDay) return time
  return `${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${time}`
}

export function managedFileGroupAt(group: ManagedFileGroup): number {
  return group.versions[0]?.createdAt ?? 0
}

/** Stable runtime signal for the refresh counters: a write becomes a registry
 *  version when its tool COMPLETES, so row counts alone can miss it (the tool
 *  row existed while running). Status counts change exactly on lifecycle
 *  transitions and never on streamed characters, so re-checks stay bounded. */
export function toolStatusSignal(toolCalls: Array<{ status: string }> | null | undefined): string {
  if (!toolCalls?.length) return ''
  const counts = new Map<string, number>()
  for (const call of toolCalls) counts.set(call.status, (counts.get(call.status) ?? 0) + 1)
  return [...counts.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([status, count]) => `${status}×${count}`).join(',')
}

export function managedFilesRefreshKey(detail: ConversationDetail | null): string {
  return [
    detail?.messages?.length ?? 0,
    detail?.artifacts?.length ?? 0,
    toolStatusSignal(detail?.toolCalls),
    detail?.lastRun?.id ?? '',
    detail?.lastRun?.status ?? '',
    detail?.lastRun?.finishedAt ?? '',
  ].join(':')
}
