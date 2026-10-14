import { type ComponentProps, type CSSProperties } from 'react'
import { cn } from '@/lib/utils'
import type { SkeletonProps } from '@/components/arc/skeleton/skeleton'
import styles from '@/components/arc/skeleton/skeleton.module.css'
import { deferredModule } from '@/components/fox-ui/use-deferred-module'

const reveal = deferredModule(() => import('@/components/arc/skeleton/skeleton'))
function Skeleton({ className, ...props }: ComponentProps<'div'>) {
  return <div data-slot="skeleton" aria-hidden="true" className={cn(styles.block, 'rounded-md bg-muted', className)} {...props} />
}
function Placeholder({ label = 'Loading content', lines = 3, avatar = false, className }: SkeletonProps) {
  const count = Math.min(Math.max(Math.floor(lines), 1), 6)
  return <div className={cn(styles.root, className)} role="status" aria-label={label} aria-busy="true">
    {avatar && <span className={styles.avatar} aria-hidden="true" />}
    <span className={styles.lines} aria-hidden="true">{Array.from({ length: count }, (_, index) => <span className={styles.line} style={{ '--index': index + (avatar ? 1 : 0) } as CSSProperties} key={index} />)}</span>
  </div>
}
function SkeletonReveal(props: SkeletonProps) {
  const loaded = reveal.useModule()
  if (loaded) return <loaded.Skeleton {...props} />
  return <div className={props.className} aria-busy={props.loading ?? true}><div className={styles.frame}>{props.loading === false ? props.children : <Placeholder {...props} className={undefined} />}</div></div>
}
function SkeletonContent(props: SkeletonProps) {
  return props.children === undefined ? <Placeholder {...props} /> : <SkeletonReveal {...props} />
}
export { Skeleton, SkeletonContent }
