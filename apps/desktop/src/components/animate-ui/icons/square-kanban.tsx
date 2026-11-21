// Lucide square-kanban with its three columns popping up in sequence.
'use client';

import * as React from 'react';
import { motion, type Variants } from 'motion/react';

import {
  getVariants,
  useAnimateIconContext,
  IconWrapper,
  type IconProps,
} from '@/components/animate-ui/icons/icon';

type SquareKanbanProps = IconProps<keyof typeof animations>;

const column = (delay: number): Variants => ({
  initial: { scaleY: 1, opacity: 1 },
  animate: {
    scaleY: [1, 0.45, 1.12, 1],
    opacity: [1, 0.65, 1, 1],
    transformOrigin: 'center top',
    transition: { duration: 0.6, delay, ease: 'easeOut', times: [0, 0.35, 0.7, 1] },
  },
});

const animations = {
  default: {
    frame: {},
    first: column(0),
    second: column(0.07),
    third: column(0.14),
  } satisfies Record<string, Variants>,
} as const;

function IconComponent({ size, ...props }: SquareKanbanProps) {
  const { controls } = useAnimateIconContext();
  const variants = getVariants(animations);

  return (
    <motion.svg
      xmlns="http://www.w3.org/2000/svg"
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
      initial="initial"
      animate={controls}
      {...props}
    >
      <motion.rect width="18" height="18" x="3" y="3" rx="2" variants={variants.frame} />
      <motion.path d="M8 7v7" variants={variants.first} initial="initial" animate={controls} />
      <motion.path d="M12 7v4" variants={variants.second} initial="initial" animate={controls} />
      <motion.path d="M16 7v9" variants={variants.third} initial="initial" animate={controls} />
    </motion.svg>
  );
}

function SquareKanban(props: SquareKanbanProps) {
  return <IconWrapper icon={IconComponent} {...props} />;
}

export {
  animations,
  SquareKanban,
  SquareKanban as SquareKanbanIcon,
  type SquareKanbanProps,
  type SquareKanbanProps as SquareKanbanIconProps,
};
