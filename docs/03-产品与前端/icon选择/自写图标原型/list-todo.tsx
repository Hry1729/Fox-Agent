// Animated icon for 处理进度 (job progress) in the knowledge sidebar.
//
// Glyph: lucide `list-todo` — a checkbox, a checkmark, and three rows. Already
// in the repo, so resting/end frames are unchanged.
//
// Motion: the checkmark draws itself (pathLength), then the three rows drop in
// one after another. This is the one entry in the sidebar with real state
// semantics (pending -> done), so it is allowed to actually "run through"
// rather than just nudge.
'use client';

import * as React from 'react';
import { motion, type Variants } from 'motion/react';

import {
  getVariants,
  useAnimateIconContext,
  IconWrapper,
  type IconProps,
} from '@/components/animate-ui/icons/icon';

type ListTodoProps = IconProps<keyof typeof animations>;

const animations = {
  default: {
    box: {},
    check: {
      initial: { pathLength: 1, opacity: 1 },
      animate: {
        pathLength: [0, 1],
        opacity: [0, 1],
        transition: { duration: 0.35, ease: 'easeOut' },
      },
    },
    row1: {
      initial: { opacity: 1, y: 0 },
      animate: {
        opacity: [0, 1],
        y: [3, 0],
        transition: { duration: 0.3, ease: 'easeOut', delay: 0.2 },
      },
    },
    row2: {
      initial: { opacity: 1, y: 0 },
      animate: {
        opacity: [0, 1],
        y: [3, 0],
        transition: { duration: 0.3, ease: 'easeOut', delay: 0.32 },
      },
    },
    row3: {
      initial: { opacity: 1, y: 0 },
      animate: {
        opacity: [0, 1],
        y: [3, 0],
        transition: { duration: 0.3, ease: 'easeOut', delay: 0.44 },
      },
    },
  } satisfies Record<string, Variants>,
} as const;

function IconComponent({ size, ...props }: ListTodoProps) {
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
      <motion.rect
        x={3}
        y={4}
        width={6}
        height={6}
        rx={1}
        variants={variants.box}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="M13 5h8"
        variants={variants.row1}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="M13 12h8"
        variants={variants.row2}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="M13 19h8"
        variants={variants.row3}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="m3 17 2 2 4-4"
        variants={variants.check}
        initial="initial"
        animate={controls}
      />
    </motion.svg>
  );
}

function ListTodo(props: ListTodoProps) {
  return <IconWrapper icon={IconComponent} {...props} />;
}

export {
  animations,
  ListTodo,
  ListTodo as ListTodoIcon,
  type ListTodoProps,
  type ListTodoProps as ListTodoIconProps,
};
