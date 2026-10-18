import { useId, useMemo, useState } from 'react'
import { ChevronDown, ChevronRight } from 'lucide-react'
import { File as MessageFile } from '@/components/assistant-ui/elements/file'
import type { ArtifactRecord } from '@/features/conversations/model/types'

/**
 * A file the model explicitly declared through `present_files`.
 *
 * This reads the Host's recorded declaration and nothing else. It deliberately
 * does not look at the reply text, a heading, or a file extension: inferring a
 * delivery from prose is what made the panel disagree with what the model
 * actually handed over.
 */
function isDeclared(artifact: ArtifactRecord) {
  return artifact.declaredAt != null
}

function artifactFileExtension(artifact: ArtifactRecord) {
  const source = (artifact.displayName || artifact.storagePath || '').split(/[?#]/)[0]
  const name = source.split(/[\\/]/).pop() ?? source
  const dot = name.lastIndexOf('.')
  if (dot <= 0 || dot >= name.length - 1) return ''
  return name.slice(dot + 1).toLowerCase()
}

/**
 * A rendered HTML preview the Host produced for the user.
 *
 * Preview stays separate from delivery: it is a Host-side rendering of some
 * other file, not a file the model handed over.
 */
function isPreviewArtifact(artifact: ArtifactRecord) {
  if (artifact.artifactClass === 'preview') return true
  if (typeof artifact.artifactClass === 'string' && artifact.artifactClass.length > 0) return false
  return artifact.mediaType === 'text/html' && artifact.artifactOrigin === 'host_private'
}

export function formatFileSize(bytes: number) {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KB`
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`
}

/**
 * The verification word shown in the file *detail* panel.
 *
 * The chat card no longer shows this: a delivered file's card says what the file
 * is, not what a check concluded. It stays exported because the detail view, which
 * the user opens deliberately for diagnostics, still reports it for the Runs whose
 * contracts were frozen under the previous design.
 */
export function artifactVerificationLabel(status: string) {
  switch (status) {
    case 'passed': return '内容已核对'
    case 'failed': return '内容需核对'
    case 'stale': return '版本已变化'
    case 'unavailable': return '文件不可用'
    case 'pending': return '待核对'
    default: return '未核验'
  }
}

/**
 * The short line under a delivered file's name.
 *
 * Only what the user needs to decide whether to open it: the model's own note
 * when it supplied one, otherwise a plain statement of where the file lives.
 * No verification tally, no version number, no byte count — those describe the
 * delivery machinery, and the delivery no longer has a verdict to report.
 */
export function artifactSummaryLine(artifact: ArtifactRecord) {
  const description = artifact.declaredDescription?.trim()
  if (description) return description
  if (isPreviewArtifact(artifact)) return '预览'
  return artifact.artifactOrigin === 'host_private' ? 'Fox 内部文件' : '项目文件'
}

function ArtifactResultCard({ artifact, onOpenArtifact }: { artifact: ArtifactRecord; onOpenArtifact?: (artifact: ArtifactRecord) => void }) {
  return (
    <button
      type="button"
      className="fox-message-artifact-trigger"
      title={artifact.storagePath ?? artifact.displayName}
      onClick={() => onOpenArtifact?.(artifact)}
    >
      <MessageFile
        className="fox-message-artifact"
        title={artifact.storagePath ?? artifact.displayName}
        name={artifact.displayName}
        mediaType={artifact.mediaType}
        bytes={artifact.byteSize}
        canOpen={Boolean(onOpenArtifact)}
      />
      {artifactSummaryLine(artifact) ? (
        <span className="fox-message-artifact-description">{artifactSummaryLine(artifact)}</span>
      ) : null}
    </button>
  )
}

/**
 * The files of one reply, in the order the result area must present them:
 * declared deliverables first, then the process files collapsed behind a
 * disclosure, then whatever the message footer already renders.
 *
 * A declared file is never repeated in the process list, so a reply that hands
 * over one report does not show that report twice.
 */
export function RuntimeArtifacts({ artifacts, onOpenArtifact }: { artifacts: ArtifactRecord[]; onOpenArtifact?: (artifact: ArtifactRecord) => void }) {
  const [openGroup, setOpenGroup] = useState<'process' | null>(null)
  const panelId = useId()
  const { deliverables, processFiles } = useMemo(() => {
    const delivered: ArtifactRecord[] = []
    const process: ArtifactRecord[] = []
    const deliveredPaths = new Set<string>()
    for (const artifact of artifacts) {
      if (isDeclared(artifact)) {
        delivered.push(artifact)
        deliveredPaths.add(artifact.storagePath)
      }
    }
    for (const artifact of artifacts) {
      if (isDeclared(artifact)) continue
      // One file is one card. A process row for a file already delivered in
      // this reply would be the same file presented twice.
      if (deliveredPaths.has(artifact.storagePath)) continue
      process.push(artifact)
    }
    // Declaration order is the model's own order, which is the order the user
    // should read them in.
    delivered.sort((a, b) => (a.declaredAt ?? 0) - (b.declaredAt ?? 0))
    return { deliverables: delivered, processFiles: process }
  }, [artifacts])

  // No declared file means no result area at all. An empty "本次文件结果"
  // heading would suggest something was delivered and then lost.
  if (!deliverables.length && !processFiles.length) return null

  const processDisclosure = processFiles.length > 0 && (
    <div className="fox-message-file-results-row">
      <button
        type="button"
        className="fox-message-file-results-head fox-message-artifact-group-trigger"
        aria-expanded={openGroup === 'process'}
        aria-controls={`${panelId}-process`}
        onClick={() => setOpenGroup(openGroup === 'process' ? null : 'process')}
      >
        <strong>过程文件</strong><span>{processFiles.length}</span>
        {openGroup === 'process' ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
      </button>
    </div>
  )

  return (
    <section className="fox-message-file-results" aria-label="文件结果">
      {deliverables.length > 0 && (
        <div className="fox-message-artifact-group-content" role="region" aria-label="交付文件">
          <div className="fox-message-file-results-head"><strong>交付文件</strong><span>{deliverables.length}</span></div>
          <div className="fox-message-artifacts">
            {deliverables.map((artifact) => (
              <ArtifactResultCard key={artifact.id} artifact={artifact} onOpenArtifact={onOpenArtifact} />
            ))}
          </div>
        </div>
      )}
      {processDisclosure}
      <div
        id={`${panelId}-process`}
        className="fox-message-artifact-group-content"
        role="region"
        aria-label="过程文件"
        hidden={openGroup !== 'process'}
      >
        {openGroup === 'process' && (
          <div className="fox-message-artifacts">
            {processFiles.map((artifact) => (
              <ArtifactResultCard key={artifact.id} artifact={artifact} onOpenArtifact={onOpenArtifact} />
            ))}
          </div>
        )}
      </div>
    </section>
  )
}
