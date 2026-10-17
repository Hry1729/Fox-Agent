import type { FoxAnimation } from '../../run-activity-presentation'
export class BlueFoxEngine {
  constructor(svg: SVGSVGElement, options: { followPointer: boolean; autoTricks: boolean; reduceMotion: boolean; initialState?: FoxAnimation; thinkingStartedAt?: number })
  thinkingStartedAt?: number
  setMotionEnabled(enabled: boolean): void
  setState(state: FoxAnimation): void
  setProgress(percent: number): void
  setPaused(paused: boolean): void
  destroy(): void
}
