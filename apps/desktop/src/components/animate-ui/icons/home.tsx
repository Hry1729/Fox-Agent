// Animated icon for the 最近 entry in the knowledge-base sidebar nav.
//
// Glyph: the user's supplied `HomeIcon` (lucide `house`). Its two paths are the
// same geometry as lucide-react's own `House` — the roof is written
// `l7-5.999` here vs `l7-6` there, which is the same point — so this swap does
// NOT change the resting shape of the 最近 row.
//
// Ported into the animate-ui house contract. The supplied component was in the
// lucide-animated registry format (forwardRef + useImperativeHandle + its own
// <div> with its own mouse handlers + hardcoded strokeWidth="2"); dropped in
// as-is it could not be driven by AnimatedIconTrigger, and the hover target
// would shrink to the glyph alone. Same glyph strings and same drawn effect,
// different wrapper.
//
// The registry drew the door with `pathLength` (a normalised 0..1 length, which
// Motion resolves to a stroke-dasharray against the path's real length). This
// file originally "translated" that to a literal `strokeDasharray: '1 1 0'`,
// which was WRONG: those are user units, and the door is 23.14 units long, so a
// 1-unit dash rendered a 0.75px dot and the animation was invisible. `pathLength`
// is the idiom the official animate-ui sources use for exactly this draw-on
// (see list-todo.tsx's `check`), and Motion does animate it — verified in a
// browser. Use it directly.
'use client';

import * as React from 'react';
import { motion, type Variants } from 'motion/react';

import {
  getVariants,
  useAnimateIconContext,
  IconWrapper,
  type IconProps,
} from '@/components/animate-ui/icons/icon';

type HomeProps = IconProps<keyof typeof animations>;

const animations = {
  default: {
    // The roof/outline holds still; only the doorway draws itself in, which is
    // the effect the supplied component aimed for.
    frame: {},
    door: {
      initial: { pathLength: 1, opacity: 1 },
      animate: {
        pathLength: [0, 1],
        opacity: [0, 1],
        transition: { duration: 0.6, ease: 'easeInOut' },
      },
    },
  } satisfies Record<string, Variants>,
} as const;

function IconComponent({ size, ...props }: HomeProps) {
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
        d="M3 10a2 2 0 0 1 .709-1.528l7-5.999a2 2 0 0 1 2.582 0l7 5.999A2 2 0 0 1 21 10v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"
        variants={variants.frame}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="M15 21v-8a1 1 0 0 0-1-1h-4a1 1 0 0 0-1 1v8"
        variants={variants.door}
        initial="initial"
        animate={controls}
      />
    </motion.svg>
  );
}

function Home(props: HomeProps) {
  return <IconWrapper icon={IconComponent} {...props} />;
}

export {
  animations,
  Home,
  Home as HomeIcon,
  type HomeProps,
  type HomeProps as HomeIconProps,
};
