/** Independent follow state for one capped process group. */
export interface ProcessScrollMetrics {
  readonly top: number
  readonly bottom: number
}

export function processScrollMetrics(element: HTMLElement): ProcessScrollMetrics {
  return { top: element.scrollTop, bottom: Math.max(0, element.scrollHeight - element.clientHeight) }
}

export class ProcessScrollFollow {
  private following = false
  private target: number | null = null
  private sampledTop: number | undefined

  get active(): boolean { return this.following }
  get animating(): boolean { return this.target !== null }

  /** Reader movement, unlike native animation progress, changes follow intent. */
  sample(metrics: ProcessScrollMetrics): void {
    const moved = this.sampledTop === undefined || Math.abs(metrics.top - this.sampledTop) > 0.5
    this.sampledTop = metrics.top
    if (moved && !this.animating) this.following = metrics.bottom - metrics.top <= 1
  }

  /** A manual opening has an explicit intent even when content initially fits. */
  initialize(element: HTMLElement, position: 'top' | 'bottom'): ProcessScrollMetrics {
    this.target = null
    const metrics = processScrollMetrics(element)
    const top = position === 'bottom' ? metrics.bottom : 0
    element.scrollTop = top
    this.sampledTop = element.scrollTop
    this.following = position === 'bottom'
    return processScrollMetrics(element)
  }

  /** Restore a reading position after a temporary uncapped display mode. */
  restore(element: HTMLElement, top: number): ProcessScrollMetrics {
    this.target = null
    const metrics = processScrollMetrics(element)
    element.scrollTop = this.following ? metrics.bottom : Math.max(0, Math.min(top, metrics.bottom))
    this.sampledTop = element.scrollTop
    return processScrollMetrics(element)
  }

  /** Stop native motion at its delivered position before reader input. */
  interrupt(element: HTMLElement): void {
    if (!this.animating) return
    this.target = null
    const top = element.scrollTop
    this.sampledTop = top
    if (typeof element.scrollTo === 'function') element.scrollTo({ top, behavior: 'instant' })
    else element.scrollTop = top
  }

  /** A completed native animation may have an older target than the latest content height. */
  settle(metrics: ProcessScrollMetrics): void {
    const target = this.target
    this.target = null
    if (target !== null) {
      this.following = Math.abs(metrics.top - Math.min(target, metrics.bottom)) <= 1
      this.sampledTop = metrics.top
    } else this.sample(metrics)
  }

  /** Start at most one native smooth animation until scrollend settles it. */
  followGrowth(element: HTMLElement, metrics = processScrollMetrics(element)): void {
    if (!this.following || this.animating || metrics.bottom - metrics.top <= 1) return
    if (typeof matchMedia === 'function' && matchMedia('(prefers-reduced-motion: reduce)').matches) {
      element.scrollTop = metrics.bottom
      this.sampledTop = element.scrollTop
      return
    }
    if (typeof element.scrollTo !== 'function') {
      element.scrollTop = metrics.bottom
      this.sampledTop = element.scrollTop
      return
    }
    this.target = metrics.bottom
    element.scrollTo({ top: metrics.bottom, behavior: 'smooth' })
  }

  reset(): void {
    this.following = false
    this.target = null
    this.sampledTop = undefined
  }
}
