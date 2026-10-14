// Animated icon for the 资料 / assistant mode switch in the chat sidebar.
//
// Glyph: lucide `book-copy` — the front cover, the spine foot, and the stacked
// cover beneath. Already in the repo, so resting/end frames are unchanged.
//
// Motion: the front cover tips on its spine foot while the stack beneath it
// settles a beat later, so the switch reads as "putting the other book
// forward" rather than as a generic wiggle.
'use client';

import * as React from 'react';
import { motion, type Variants } from 'motion/react';

import {
  getVariants,
  useAnimateIconContext,
  IconWrapper,
  type IconProps,
} from '@/components/animate-ui/icons/icon';

type BookCopyProps = IconProps<keyof typeof animations>;

const animations = {
  default: {
    // `left bottom` is the spine foot — the cover's own bottom-left corner.
    // Keywords rather than `px` because Motion sets `transform-box: fill-box`
    // on SVG, so an origin resolves against the path's own bounding box.
    back: {
      initial: { rotate: 0, y: 0 },
      animate: {
        rotate: [0, -5, 2, 0],
        y: [0, -1, 0],
        transformOrigin: 'left bottom',
        transition: { duration: 0.7, ease: 'easeInOut' },
      },
    },
    stack: {
      initial: { x: 0, y: 0 },
      animate: {
        x: [0, 1, 0],
        y: [0, 1, 0],
        transition: { duration: 0.7, ease: 'easeInOut', delay: 0.06 },
      },
    },
    spine: {},
  } satisfies Record<string, Variants>,
} as const;

function IconComponent({ size, ...props }: BookCopyProps) {
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
      <motion.path
        d="M5 7a2 2 0 0 0-2 2v11"
        variants={variants.spine}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="M5.803 18H5a2 2 0 0 0 0 4h9.5a.5.5 0 0 0 .5-.5V21"
        variants={variants.stack}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="M9 15V4a2 2 0 0 1 2-2h9.5a.5.5 0 0 1 .5.5v14a.5.5 0 0 1-.5.5H11a2 2 0 0 1 0-4h10"
        variants={variants.back}
        initial="initial"
        animate={controls}
      />
    </motion.svg>
  );
}

function BookCopy(props: BookCopyProps) {
  return <IconWrapper icon={IconComponent} {...props} />;
}

export {
  animations,
  BookCopy,
  BookCopy as BookCopyIcon,
  type BookCopyProps,
  type BookCopyProps as BookCopyIconProps,
};
