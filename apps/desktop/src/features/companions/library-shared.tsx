// Leaf helpers for the 资料库 row language (media kind, time bands, relative
// time, file glyph). Extracted from `files.tsx` so the managed-file version
// rows in the chat surfaces can render the exact same rows without importing
// (and re-entering) the library component.
import { FileAudio, FileVideo, Image as ImageIcon } from 'lucide-react'
import type { CompanionFileEntry } from './api'
import { formatSearchTime } from './chat-search'

export type LibraryTimeGroup = 'today' | 'week' | 'earlier'
export const LIBRARY_GROUPS: Array<[LibraryTimeGroup, string]> = [['today', '今天'], ['week', '本周'], ['earlier', '更早']]

type LibraryFileShape = Pick<CompanionFileEntry, 'name'> & Partial<Pick<CompanionFileEntry, 'mediaType' | 'missing'>>

export function libraryMediaKind(entry: LibraryFileShape): 'image' | 'video' | 'audio' | null {
  const mime = entry.mediaType ?? ''
  if (mime.startsWith('image/')) return 'image'
  if (mime.startsWith('video/')) return 'video'
  if (mime.startsWith('audio/')) return 'audio'
  const ext = entry.name.includes('.') ? entry.name.split('.').pop()!.toLowerCase() : ''
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'bmp', 'avif'].includes(ext)) return 'image'
  if (['mp4', 'mov', 'webm', 'mkv', 'avi'].includes(ext)) return 'video'
  if (['mp3', 'wav', 'ogg', 'm4a', 'flac'].includes(ext)) return 'audio'
  return null
}

export function libraryTimeGroup(at: number, now: number): LibraryTimeGroup {
  const date = new Date(at)
  const current = new Date(now)
  if (date.toDateString() === current.toDateString()) return 'today'
  const weekStart = new Date(current)
  weekStart.setHours(0, 0, 0, 0)
  weekStart.setDate(current.getDate() - ((current.getDay() + 6) % 7))
  return at >= weekStart.getTime() ? 'week' : 'earlier'
}

export function formatLibraryRelative(at: number, now = Date.now()): string {
  const minutes = Math.floor(Math.max(0, now - at) / 60_000)
  if (minutes < 1) return '刚刚'
  if (minutes < 60) return `${minutes} 分钟`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours} 小时`
  const days = Math.floor(hours / 24)
  if (days < 7) return `${days} 天`
  return formatSearchTime(at, now)
}

export function libraryGroupLabel(value: LibraryTimeGroup): string {
  return LIBRARY_GROUPS.find(([key]) => key === value)![1]
}

const BAND_COLORS: Record<string, string> = {
  pdf: '#e5484d', doc: '#3e63dd', docx: '#3e63dd', xls: '#30a46c', xlsx: '#30a46c', csv: '#30a46c',
  ppt: '#f76b15', pptx: '#f76b15', zip: '#f5a524', rar: '#f5a524', '7z': '#f5a524',
}

export function LibraryGlyph({ entry }: { entry: LibraryFileShape }) {
  const media = libraryMediaKind(entry)
  const missing = entry.missing ? ' is-missing' : ''
  if (media) {
    const Icon = media === 'image' ? ImageIcon : media === 'video' ? FileVideo : FileAudio
    return <span className={`fox-library-glyph is-media is-${media}${missing}`} aria-hidden="true"><Icon size={15} /></span>
  }
  const ext = entry.name.includes('.') ? entry.name.split('.').pop()!.toLowerCase() : ''
  const band = BAND_COLORS[ext]
  return <svg className={`fox-library-glyph is-doc${missing}`} viewBox="0 0 20 20" aria-hidden="true">
    <path className="fox-library-page" d="M4.5 1.5h7.5l3.5 3.5v13.5h-11z" />
    <path className="fox-library-fold" d="M12 1.5V5h3.5" />
    {band
      ? <rect x="2.5" y="10.5" width="15" height="4.5" rx="1.5" fill={band} />
      : <path className="fox-library-line" d="M7 8.5h6M7 11.5h6M7 14.5h4" />}
  </svg>
}
