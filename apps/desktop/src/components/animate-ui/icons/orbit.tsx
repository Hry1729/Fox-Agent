// Animated icon for 向量模型 (vector models) in the knowledge sidebar.
//
// Glyph: lucide `orbit` — a centre dot, two orbital arcs, and two satellites.
// Already in the repo, so resting/end frames are unchanged.
//
// Motion: the two orbital arcs sweep as ONE body, and the core pulses.
//
// Two traps this avoids:
//  1. The arcs must be rotated together in a <motion.g>, not individually. Each
//     arc's bbox is its own quarter-ish span, so with `transform-box: fill-box`
//     the same `rotate` value pivots them about different points and they pull
//     apart. One group gives them a single pivot.
//  2. The satellites are deliberately NOT animated and are kept OUTSIDE the
//     group, so they stay pinned to the arc endpoints the way the static glyph
//     draws them. Animating the arc `d` would visually detach them.
'use client';

import * as React from 'react';
import { motion, type Variants } from 'motion/react';

import {
  getVariants,
  useAnimateIconContext,
  IconWrapper,
  type IconProps,
} from '@/components/animate-ui/icons/icon';

type OrbitProps = IconProps<keyof typeof animations>;

const animations = {
  default: {
    ring: {
      initial: { rotate: 0 },
      animate: {
        rotate: [0, 8, -4, 0],
        transition: { duration: 0.9, ease: 'easeInOut' },
      },
    },
    // A circle's bbox is centred on the circle, so the default 50% 50% origin
    // is already the right pivot — no transformOrigin needed.
    core: {
      initial: { scale: 1 },
      animate: {
        scale: [1, 1.12, 1],
        transition: { duration: 0.9, ease: 'easeInOut' },
      },
    },
    sat: {},
  } satisfies Record<string, Variants>,
} as const;

function IconComponent({ size, ...props }: OrbitProps) {
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
      <motion.g
        variants={variants.ring}
        initial="initial"
        animate={controls}
      >
        <motion.path
          d="M20.341 6.484A10 10 0 0 1 10.266 21.85"
          variants={variants.ring}
          initial="initial"
          animate={controls}
        />
        <motion.path
          d="M3.659 17.516A10 10 0 0 1 13.74 2.152"
          variants={variants.ring}
          initial="initial"
          animate={controls}
        />
      </motion.g>
      <motion.circle
        cx={12}
        cy={12}
        r={3}
        variants={variants.core}
        initial="initial"
        animate={controls}
      />
      <motion.circle
        cx={19}
        cy={5}
        r={2}
        variants={variants.sat}
        initial="initial"
        animate={controls}
      />
      <motion.circle
        cx={5}
        cy={19}
        r={2}
        variants={variants.sat}
        initial="initial"
        animate={controls}
      />
    </motion.svg>
  );
}

function Orbit(props: OrbitProps) {
  return <IconWrapper icon={IconComponent} {...props} />;
}

export {
  animations,
  Orbit,
  Orbit as OrbitIcon,
  type OrbitProps,
  type OrbitProps as OrbitIconProps,
};
