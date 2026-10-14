import { useId } from 'react'
import type { EmptyStatesProps } from '@/components/arc/blocks/empty-states/empty-states'
import { FRAMES, KINDS, VIEW, shapePath } from '@/components/arc/blocks/empty-states/empty-states-art'
import styles from '@/components/arc/blocks/empty-states/empty-states.module.css'
import { deferredModule } from './use-deferred-module'
const implementation = deferredModule(() => import('@/components/arc/blocks/empty-states/empty-states'))
export type FoxEmptyStateProps = EmptyStatesProps
/** Preserve Arc's illustration and layout while its morphing engine loads. */
export function FoxEmptyState({ title, description, icon, action, variant = 'empty', className }: FoxEmptyStateProps) {
  const loaded = implementation.useModule()
  const uid = useId()
  if (loaded) return <loaded.EmptyStates {...{ title, description, icon, action, variant, className }} />
  const frame = FRAMES[variant === 'search' ? 'search' : variant === 'error' ? 'offline' : 'inbox'].idle
  return <section className={[styles.root, className].filter(Boolean).join(' ')} data-variant={variant} aria-labelledby={`${uid}-title`}><div className={styles.panel}>
    <div className={styles.stage} aria-hidden="true"><svg className={styles.art} viewBox={`0 0 ${VIEW.width} ${VIEW.height}`}><g>{frame.prims.map((p, i) => {
      const path = shapePath(KINDS[i]!, p.geo, p.fx[1], 0, 0)
      return <path key={i} className={styles.shape} data-kind={KINDS[i]} data-tone={p.tone} d={path.d} opacity={p.fx[0]} strokeDasharray={path.dash} />
    })}</g></svg></div><div className={styles.copy}>{icon && <span className={styles.customIcon} aria-hidden="true">{icon}</span>}<h3 id={`${uid}-title`} className={styles.title}>{title}</h3>{description != null && <div className={styles.line}>{description}</div>}{action != null && <div className={styles.action}>{action}</div>}</div>
  </div></section>
}
