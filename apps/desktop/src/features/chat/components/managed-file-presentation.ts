/**
 * Presentation of one managed-file version row.
 *
 * A version is a *content* version: the size shown next to `version N` must be
 * the size of the content a restore would write, and the restore action must be
 * offered exactly when that content is verified and still resolvable. Keeping
 * the rule here makes it testable without a DOM and keeps the panel from
 * inventing its own idea of what "restore this version" means.
 */

export interface ManagedFileVersionView {
  id: string
  versionNo: number
  changeKind: string
  tool: string
  beforeSize: number | null
  afterSize: number | null
  restoreSize: number | null
  canRestore: boolean
  restoreBlocker: string | null
  sourceKind: string
  drifted: boolean
  backupAvailable: boolean
}

export interface ManagedFileVersionPresentation {
  /** Always true: every recorded version is listed, including unusable ones. */
  visible: boolean
  /** UTF-8 byte count of the content this row records. */
  sizeLabel: number | null
  restorable: boolean
  /** Why it cannot be restored, shown to the user instead of hiding the row. */
  blocker: string | null
  /** This row is the file's current content (no later edit, no drift). */
  current: boolean
  /** Overwriting needs an explicit force because the file changed outside. */
  forceRequired: boolean
}

export function managedFileVersionPresentation(
  version: ManagedFileVersionView,
  options: { isLatest?: boolean; drifted?: boolean } = {},
): ManagedFileVersionPresentation {
  const drifted = options.drifted ?? version.drifted
  const current = (options.isLatest ?? false) && !drifted
  return {
    visible: true,
    // Fall back to afterSize only for rows written before restoreSize existed.
    sizeLabel: version.restoreSize ?? version.afterSize,
    restorable: version.canRestore && !current,
    blocker: version.canRestore ? null : version.restoreBlocker,
    current,
    forceRequired: drifted && !current && version.canRestore,
  }
}
