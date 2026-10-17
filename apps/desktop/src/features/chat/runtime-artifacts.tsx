import { useId, useMemo, useState } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { finalArtifactIds } from './final-artifact-references'
import { File as MessageFile } from '@/components/assistant-ui/elements/file'
import type { ArtifactRecord } from '@/features/conversations/model/types'

/** Host classification wins; legacy rows use the historical extension heuristic. */
function artifactClassOf(artifact: ArtifactRecord): 'deliverable' | 'preview' | 'process' | null {
  const recorded = (artifact as { artifactClass?: unknown }).artifactClass
  return recorded === 'deliverable' || recorded === 'preview' || recorded === 'process' ? recorded : null
}

const DELIVERABLE_FILE_EXTENSIONS = new Set(['xlsx', 'xls', 'xlsm', 'docx', 'doc', 'pptx', 'ppt', 'pdf'])
const WORKING_FILE_EXTENSIONS = new Set(['csv', 'tsv', 'json', 'jsonl', 'ndjson', 'txt', 'md', 'markdown', 'log', 'yaml', 'yml', 'xml'])
const WORKING_MEDIA_TYPES = new Set(['text/csv', 'text/tab-separated-values', 'application/json', 'text/json', 'application/x-ndjson'])

function artifactFileExtension(artifact: ArtifactRecord) {
  const source = (artifact.displayName || artifact.storagePath || '').split(/[?#]/)[0]
  const name = source.split(/[\\/]/).pop() ?? source
  const dot = name.lastIndexOf('.')
  if (dot <= 0 || dot >= name.length - 1) return ''
  return name.slice(dot + 1).toLowerCase()
}

function isDeliverableArtifact(artifact: ArtifactRecord) {
  const hostClass = artifactClassOf(artifact)
  if (hostClass) return hostClass === 'deliverable'
  const extension = artifactFileExtension(artifact)
  if (DELIVERABLE_FILE_EXTENSIONS.has(extension)) return true
  if (WORKING_FILE_EXTENSIONS.has(extension)) return false
  const mediaType = artifact.mediaType?.split(';')[0]?.trim().toLowerCase() ?? ''
  if (mediaType && WORKING_MEDIA_TYPES.has(mediaType)) return false
  return artifact.artifactType === 'created_file' || artifact.artifactType === 'modified_file'
}

function isPreviewArtifact(artifact: ArtifactRecord) {
  if (artifactClassOf(artifact) === 'preview') return true
  const recorded = (artifact as { artifactClass?: unknown }).artifactClass
  if (typeof recorded === 'string' && recorded.length > 0) return false
  return artifact.mediaType === 'text/html' && artifact.artifactOrigin === 'host_private'
}

export function formatFileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

/** The verification state in the user's words, shared with the file detail view. */
export function artifactVerificationLabel(status: string | undefined) {
  return {
    passed: '验收通过', limited: '有限核验', failed: '验收失败',
    stale: '版本已变化', unavailable: '文件不可用', unverified: '未核验',
  }[status ?? 'unverified']
}

/**
 * One warning word per delivery state that changes what the user can expect.
 *
 * A failed *content verification* is not a failed *publish*: the file did reach the
 * project, and saying "发布未通过" next to "已发布到项目" contradicts itself. The
 * publish fact stays the base phrase; these words describe the verification result.
 */
const DELIVERY_WARNINGS: Record<string, string> = {
  failed: '内容需核对',
  stale: '版本已变化',
  unavailable: '文件不可用',
}

/**
 * One short line about what this file *is*, for the card in the answer.
 *
 * Version numbers, byte sizes, verification tallies and the delivery sentence are
 * diagnostics: they belong to the file's detail view. The only extra word here is
 * a warning when the delivery itself failed, because that changes what the user
 * can expect from the file (and it is one phrase, not a metadata join).
 */
export function artifactSummaryLine(artifact: ArtifactRecord, finalResult = false) {
  const warning = DELIVERY_WARNINGS[artifact.delivery?.verificationStatus ?? '']
  if (isPreviewArtifact(artifact)) return '预览'
  const published = artifact.artifactOrigin === 'project' && Boolean(artifact.delivery?.versionId && artifact.delivery?.sourceToolCallId)
  const base = published ? '已发布到项目'
    : artifact.artifactOrigin === 'host_private' ? `${finalResult ? '生成文件' : '中间文件'} · 未发布` : '项目文件'
  return warning ? `${base} · ${warning}` : base
}

function ArtifactResultCard({ artifact, finalResult = false, onOpenArtifact }: { artifact: ArtifactRecord; finalResult?: boolean; onOpenArtifact?: (artifact: ArtifactRecord) => void }) {
  return <button type="button" className="fox-message-artifact-trigger" title={artifactSummaryLine(artifact, finalResult)} onClick={() => onOpenArtifact?.(artifact)}>
    <MessageFile className="fox-message-artifact" title={artifact.storagePath ?? artifact.displayName} name={artifact.displayName} mediaType={artifact.mediaType} bytes={artifact.byteSize} canOpen={Boolean(onOpenArtifact)} />
  </button>
}

/** Final files stay visible; larger intermediate batches keep their disclosure. */
export function RuntimeArtifacts({ artifacts, finalResponse = '', onOpenArtifact }: { artifacts: ArtifactRecord[]; finalResponse?: string; onOpenArtifact?: (artifact: ArtifactRecord) => void }) {
  const [openGroup, setOpenGroup] = useState<'process' | null>(null)
  const panelId = useId()
  const { deliverables, processFiles } = useMemo(() => {
    const selected = finalArtifactIds(finalResponse, artifacts)
    const deliverableItems: ArtifactRecord[] = []
    const previewItems: ArtifactRecord[] = []
    const workingItems: ArtifactRecord[] = []
    for (const artifact of artifacts) {
      if (selected.has(artifact.id)) deliverableItems.push(artifact)
      else if (isPreviewArtifact(artifact)) previewItems.push(artifact)
      else if (isDeliverableArtifact(artifact)) deliverableItems.push(artifact)
      else workingItems.push(artifact)
    }
    const finalOrder = new Map([...selected].map((id, index) => [id, index]))
    deliverableItems.sort((a, b) => (finalOrder.get(a.id) ?? selected.size) - (finalOrder.get(b.id) ?? selected.size))
    return { deliverables: deliverableItems, processFiles: [...previewItems, ...workingItems] }
  }, [artifacts, finalResponse])
  if (!artifacts.length) return null

  if (artifacts.length <= 3) return <section className="fox-message-file-results is-direct" aria-label="文件结果">
    {([['本次文件结果', deliverables], ['过程文件', processFiles]] as const).map(([label, files]) => files.length > 0 && <div key={label} role="region" aria-label={label}>
      <div className="fox-message-file-results-head"><strong>{label}</strong><span>{files.length}</span></div>
      <div className="fox-message-artifacts">{files.map(artifact => <ArtifactResultCard key={artifact.id} artifact={artifact} finalResult={label === '本次文件结果'} onOpenArtifact={onOpenArtifact} />)}</div>
    </div>)}
  </section>

  const activeGroup = openGroup === 'process' && processFiles.length ? 'process' : null
  return <section className="fox-message-file-results" aria-label="文件结果">
    <div className="fox-message-file-results-row">
      {processFiles.length > 0 && <button type="button" className="fox-message-file-results-head fox-message-artifact-group-trigger" aria-expanded={activeGroup === 'process'} aria-controls={`${panelId}-process`} onClick={() => setOpenGroup(openGroup === 'process' ? null : 'process')}>
        <strong>过程文件</strong><span>{processFiles.length}</span>{activeGroup === 'process' ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
      </button>}
    </div>
    {deliverables.length > 0 && <div id={`${panelId}-result`} className="fox-message-artifact-group-content" role="region" aria-label="本次文件结果">
      <div className="fox-message-file-results-head"><strong>本次文件结果</strong><span>{deliverables.length}</span></div>
      <div className="fox-message-artifacts">
        {deliverables.map((artifact) => <ArtifactResultCard key={artifact.id} artifact={artifact} finalResult onOpenArtifact={onOpenArtifact} />)}
      </div>
    </div>}
    <div id={`${panelId}-process`} className="fox-message-artifact-group-content" role="region" aria-label="过程文件" hidden={activeGroup !== 'process'}>
      {activeGroup === 'process' && <div className="fox-message-artifacts">
        {processFiles.map((artifact) => <ArtifactResultCard key={artifact.id} artifact={artifact} onOpenArtifact={onOpenArtifact} />)}
      </div>}
    </div>
  </section>
}
