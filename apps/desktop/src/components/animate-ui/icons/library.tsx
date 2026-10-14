// Animated icon for 远程资料库 (remote library) in the knowledge sidebar.
//
// Glyph: lucide `library` — four upright spines at x = 4 / 8 / 12 / 16. The
// glyph is already in the repo, so the resting and end frames are byte-identical
// to the static icon it replaces; only the wrapper is new.
//
// Motion: the books fan apart around a foot, like a shelf being spread to show
// what's on it. Each spine pivots on its own bottom edge — NOT on the glyph
// centre — because Motion sets `transform-box: fill-box` on SVG elements, so
// `transformOrigin` resolves against each path's own bounding box.
'use client';

import * as React from 'react';
import { motion, type Variants } from 'motion/react';

import {
  getVariants,
  useAnimateIconContext,
  IconWrapper,
  type IconProps,
} from '@/components/animate-ui/icons/icon';

type LibraryProps = IconProps<keyof typeof animations>;

const animations = {
  default: {
    // `center bottom` is the middle of each spine's own bottom edge — which is
    // the whole point: the leftmost spine (bbox x = 4) and the rightmost
    // (bbox x = 15) each swing about their own foot rather than a shared axis.
    book1: {
      initial: { rotate: 0 },
      animate: {
        rotate: [0, -3, 0],
        transformOrigin: 'center bottom',
        transition: { duration: 0.6, ease: 'easeInOut' },
      },
    },
    book2: {
      initial: { rotate: 0 },
      animate: {
        rotate: [0, 6, -2, 0],
        transformOrigin: 'center bottom',
        transition: { duration: 0.7, ease: 'easeInOut', delay: 0.08 },
      },
    },
    book3: {
      initial: { rotate: 0 },
      animate: {
        rotate: [0, 8, -3, 0],
        transformOrigin: 'center bottom',
        transition: { duration: 0.7, ease: 'easeInOut', delay: 0.16 },
      },
    },
    book4: {
      initial: { rotate: 0 },
      animate: {
        rotate: [0, 10, -4, 0],
        transformOrigin: 'right bottom',
        transition: { duration: 0.8, ease: 'easeInOut', delay: 0.24 },
      },
    },
  } satisfies Record<string, Variants>,
} as const;

function IconComponent({ size, ...props }: LibraryProps) {
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
        d="M4 4v16"
        variants={variants.book1}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="M8 8v12"
        variants={variants.book2}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="M12 6v14"
        variants={variants.book3}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="m16 6 4 14"
        variants={variants.book4}
        initial="initial"
        animate={controls}
      />
    </motion.svg>
  );
}

function Library(props: LibraryProps) {
  return <IconWrapper icon={IconComponent} {...props} />;
}

export {
  animations,
  Library,
  Library as LibraryIcon,
  type LibraryProps,
  type LibraryProps as LibraryIconProps,
};
