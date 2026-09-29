import { useId, useMemo, useState } from 'react'
import { ChevronDown, ChevronRight, FileEdit, FileText, Globe2, ImagePlus } from 'lucide-react'
import { Artifact, ArtifactDescription, ArtifactHeader, ArtifactTitle } from '@/components/ai-elements/artifact'
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

function formatFileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

function ArtifactResultCard({ artifact, onOpenArtifact }: { artifact: ArtifactRecord; onOpenArtifact?: (artifact: ArtifactRecord) => void }) {
  const isWeb = artifact.mediaType === 'text/html' || /html|web/i.test(artifact.artifactType)
  const ArtifactIcon = isWeb ? Globe2 : artifact.mediaType?.startsWith('image/') ? ImagePlus : FileText
  const changeLabel = artifact.artifactType === 'created_file' ? '新建' : artifact.artifactType === 'modified_file' ? '已修改' : '文件结果'
  const classLabel = isPreviewArtifact(artifact) ? '预览' : null
  const detail = [classLabel, changeLabel].filter(Boolean).join(' · ')
  return <button type="button" className="fox-message-artifact-trigger" onClick={() => onOpenArtifact?.(artifact)}>
    <Artifact className="fox-message-artifact" title={artifact.displayName}>
      <ArtifactHeader className="fox-message-artifact-head">
        <div className="fox-message-artifact-title">
          <span className="fox-message-artifact-icon"><ArtifactIcon size={18} /></span>
          <div><ArtifactTitle>{artifact.displayName}</ArtifactTitle><ArtifactDescription>{detail} · {formatFileSize(artifact.byteSize)}</ArtifactDescription></div>
        </div>
        <ChevronRight size={14} />
      </ArtifactHeader>
    </Artifact>
  </button>
}

/** Keep both file categories on one row; the selected category opens below it. */
export function RuntimeArtifacts({ artifacts, onOpenArtifact }: { artifacts: ArtifactRecord[]; onOpenArtifact?: (artifact: ArtifactRecord) => void }) {
  const [openGroup, setOpenGroup] = useState<'result' | 'process' | null>(null)
  const panelId = useId()
  const { deliverables, processFiles } = useMemo(() => {
    const deliverableItems: ArtifactRecord[] = []
    const previewItems: ArtifactRecord[] = []
    const workingItems: ArtifactRecord[] = []
    for (const artifact of artifacts) {
      if (isPreviewArtifact(artifact)) previewItems.push(artifact)
      else if (isDeliverableArtifact(artifact)) deliverableItems.push(artifact)
      else workingItems.push(artifact)
    }
    return { deliverables: deliverableItems, processFiles: [...previewItems, ...workingItems] }
  }, [artifacts])
  if (!artifacts.length) return null

  const activeGroup = openGroup === 'result' && deliverables.length ? 'result'
    : openGroup === 'process' && processFiles.length ? 'process' : null
  return <section className="fox-message-file-results" aria-label="文件结果">
    <div className="fox-message-file-results-row">
      <button type="button" className="fox-message-file-results-head fox-message-artifact-group-trigger" aria-expanded={activeGroup === 'result'} aria-controls={`${panelId}-result`} disabled={!deliverables.length} onClick={() => setOpenGroup(openGroup === 'result' ? null : 'result')}>
        <FileEdit size={13} /><strong>本次文件结果</strong><span>{deliverables.length}</span>{activeGroup === 'result' ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
      </button>
      <button type="button" className="fox-message-file-results-head fox-message-artifact-group-trigger" aria-expanded={activeGroup === 'process'} aria-controls={`${panelId}-process`} disabled={!processFiles.length} onClick={() => setOpenGroup(openGroup === 'process' ? null : 'process')}>
        <strong>过程文件</strong><span>{processFiles.length}</span>{activeGroup === 'process' ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
      </button>
    </div>
    <div id={`${panelId}-result`} className="fox-message-artifact-group-content" role="region" aria-label="本次文件结果" hidden={activeGroup !== 'result'}>
      {activeGroup === 'result' && <div className="fox-message-artifacts">
        {deliverables.map((artifact) => <ArtifactResultCard key={artifact.id} artifact={artifact} onOpenArtifact={onOpenArtifact} />)}
      </div>}
    </div>
    <div id={`${panelId}-process`} className="fox-message-artifact-group-content" role="region" aria-label="过程文件" hidden={activeGroup !== 'process'}>
      {activeGroup === 'process' && <div className="fox-message-artifacts">
        {processFiles.map((artifact) => <ArtifactResultCard key={artifact.id} artifact={artifact} onOpenArtifact={onOpenArtifact} />)}
      </div>}
    </div>
  </section>
}
