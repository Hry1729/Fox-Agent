import { forwardRef } from 'react'
import { RunActivity as ElementRunActivity, type RunActivityProps } from '../upstream/elements/run-activity'

export const RunActivity = forwardRef<HTMLDivElement, RunActivityProps>(function RunActivity(props, ref) {
  return <ElementRunActivity rootRef={ref} {...props} />
})
