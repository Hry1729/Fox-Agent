// Source: https://animate-ui.com/r/primitives-base-progress.json
'use client';

import * as React from 'react';
import { Progress as ProgressPrimitives } from '@base-ui-components/react/progress';
import { motion, useReducedMotion } from 'motion/react';

import {
  CountingNumber,
  type CountingNumberProps,
} from '@/components/animate-ui/primitives/texts/counting-number';
import { getStrictContext } from '@/lib/get-strict-context';

type ProgressContextType = {
  value: number;
  percentage: number;
};

const [ProgressProvider, useProgress] =
  getStrictContext<ProgressContextType>('ProgressContext');

type ProgressProps = React.ComponentProps<typeof ProgressPrimitives.Root>;

const Progress = (props: ProgressProps) => {
  return (
    <ProgressProvider value={{ value: props.value ?? 0, percentage: props.value == null ? 0 : Math.min(100, Math.max(0, ((props.value - (props.min ?? 0)) / ((props.max ?? 100) - (props.min ?? 0) || 100)) * 100)) }}>
      <ProgressPrimitives.Root data-slot="progress" {...props} />
    </ProgressProvider>
  );
};

type ProgressIndicatorProps = React.ComponentProps<
  typeof MotionProgressIndicator
>;

const MotionProgressIndicator = motion.create(ProgressPrimitives.Indicator);

function ProgressIndicator({
  transition = { type: 'spring', stiffness: 100, damping: 30 },
  ...props
}: ProgressIndicatorProps) {
  const { percentage } = useProgress();
  const reduced = useReducedMotion();

  return (
    <MotionProgressIndicator
      data-slot="progress-indicator"
      animate={{ width: `${percentage}%` }}
      transition={reduced ? { duration: 0 } : transition}
      initial={false}
      {...props}
    />
  );
}

type ProgressTrackProps = React.ComponentProps<typeof ProgressPrimitives.Track>;

function ProgressTrack(props: ProgressTrackProps) {
  return <ProgressPrimitives.Track data-slot="progress-track" {...props} />;
}

type ProgressLabelProps = React.ComponentProps<typeof ProgressPrimitives.Label>;

function ProgressLabel(props: ProgressLabelProps) {
  return <ProgressPrimitives.Label data-slot="progress-label" {...props} />;
}

type ProgressValueProps = Omit<
  React.ComponentProps<typeof ProgressPrimitives.Value>,
  'render'
> &
  Omit<CountingNumberProps, 'number'>;

function ProgressValue({
  transition = { stiffness: 80, damping: 20 },
  ...props
}: ProgressValueProps) {
  const { value } = useProgress();

  return (
    <ProgressPrimitives.Value
      data-slot="progress-value"
      render={
        <CountingNumber
          number={value ?? 0}
          transition={transition}
          {...props}
        />
      }
    />
  );
}

export {
  Progress,
  ProgressIndicator,
  ProgressTrack,
  ProgressLabel,
  ProgressValue,
  useProgress,
  type ProgressProps,
  type ProgressIndicatorProps,
  type ProgressTrackProps,
  type ProgressLabelProps,
  type ProgressValueProps,
  type ProgressContextType,
};
