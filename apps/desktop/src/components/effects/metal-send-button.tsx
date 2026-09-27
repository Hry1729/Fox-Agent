import { useRef, type ReactNode } from 'react'
import { MetalFx, useMetalBend, type MetalFxTheme } from 'metal-fx'

/**
 * Liquid-metal ring for the composer's send button.
 *
 * It lives in its own module so that `metal-fx` (a WebGL2 engine) is code-split
 * out of the entry bundle: `useMetalBend` is a hook and therefore cannot be
 * reached through `lazy()` directly, so the hook + component travel together in
 * this lazily imported chunk. Until it arrives the caller renders the plain
 * button, which is identical apart from the metal ring.
 *
 * The instance is deliberately never paused. `paused` freezes the instance on its
 * last composite, and a mount that freezes before the shared renderer has copied its
 * first frame shows as a blank white/transparent disc — and a frozen instance also
 * keeps the old palette after a theme switch. Leaving it running costs one 30px
 * shader on the library's shared 15 fps loop.
 */
export default function MetalSendButton({ children, className, theme }: {
  children: ReactNode
  className?: string
  theme: MetalFxTheme
}) {
  const rootRef = useRef<HTMLDivElement | null>(null)
  useMetalBend(rootRef)
  // `disableGlow` drops the halo layer: the library paints it with
  // `filter: saturate(7.5) brightness(0.6)`, which turns the chromatic preset into a
  // muddy smear at the button's top-left corner. The metal ring itself stays.
  return <MetalFx ref={rootRef} className={className} disableGlow innerShadow preset="chromatic" theme={theme} variant="circle">{children}</MetalFx>
}
