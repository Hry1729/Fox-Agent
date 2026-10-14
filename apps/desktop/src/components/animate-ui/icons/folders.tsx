// Animated icon for 本机文件 (local files) in the knowledge sidebar.
//
// Glyph: lucide `folders` — the back folder and the front folder's lower lip.
// This is a DEPARTURE from the `files` glyph the row used before; the user chose
// it deliberately. The two paths and the spring are taken verbatim from the
// registry component the user supplied.
//
// Motion: the back folder slides up-left, the front lip slides down-right and
// fades out; on leave both spring back.
'use client';

import * as React from 'react';
import { motion, type Variants, type Transition } from 'motion/react';

import {
  getVariants,
  useAnimateIconContext,
  IconWrapper,
  type IconProps,
} from '@/components/animate-ui/icons/icon';

type FoldersProps = IconProps<keyof typeof animations>;

// The supplied component puts the spring on the motion element's `transition`
// prop. Here it lives inside each variant, which is the animate-ui idiom and
// behaves identically for a two-state animate/normal toggle.
const spring: Transition = { type: 'spring', stiffness: 250, damping: 25 };

const animations = {
  default: {
    back: {
      initial: { x: 0, y: 0 },
      animate: { x: -2, y: 2, transition: spring },
    },
    front: {
      initial: { x: 0, y: 0, opacity: 1, scale: 1 },
      animate: { x: 2, y: -2, opacity: 0, scale: 0.9, transition: spring },
    },
  } satisfies Record<string, Variants>,
} as const;

function IconComponent({ size, ...props }: FoldersProps) {
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
        d="M20 17a2 2 0 0 0 2-2V9a2 2 0 0 0-2-2h-3.9a2 2 0 0 1-1.69-.9l-.81-1.2a2 2 0 0 0-1.67-.9H8a2 2 0 0 0-2 2v9a2 2 0 0 0 2 2Z"
        variants={variants.back}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="M2 8v11a2 2 0 0 0 2 2h14"
        variants={variants.front}
        initial="initial"
        animate={controls}
      />
    </motion.svg>
  );
}

function Folders(props: FoldersProps) {
  return <IconWrapper icon={IconComponent} {...props} />;
}

export {
  animations,
  Folders,
  Folders as FoldersIcon,
  type FoldersProps,
  type FoldersProps as FoldersIconProps,
};
