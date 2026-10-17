// Glyph: lucide `align-left` — three full-width rules whose lower two breathe
// left/right on hover. Port of the registry `AlignLeftIcon` the user supplied,
// rewritten onto the house `IconWrapper` contract so AnimateIconTrigger can drive
// it like every other icon in this folder.
'use client';

import * as React from 'react';
import { motion, type Variants } from 'motion/react';

import {
  getVariants,
  useAnimateIconContext,
  IconWrapper,
  type IconProps,
} from '@/components/animate-ui/icons/icon';

type AlignLeftProps = IconProps<keyof typeof animations>;

const animations = {
  default: {
    line1: {},
    line2: {
      initial: { x2: 15 },
      animate: {
        x2: 19,
        transition: { type: 'spring', stiffness: 150, damping: 15, mass: 0.3 },
      },
    },
    line3: {
      initial: { x2: 17 },
      animate: {
        x2: 12,
        transition: { type: 'spring', stiffness: 150, damping: 15, mass: 0.3 },
      },
    },
  } satisfies Record<string, Variants>,
} as const;

function IconComponent({ size, ...props }: AlignLeftProps) {
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
      {...props}
    >
      <motion.line
        x1={3}
        y1={6}
        x2={21}
        y2={6}
        variants={variants.line1}
        initial="initial"
        animate={controls}
      />
      <motion.line
        x1={3}
        y1={12}
        x2={15}
        y2={12}
        variants={variants.line2}
        initial="initial"
        animate={controls}
      />
      <motion.line
        x1={3}
        y1={18}
        x2={17}
        y2={18}
        variants={variants.line3}
        initial="initial"
        animate={controls}
      />
    </motion.svg>
  );
}

function AlignLeft(props: AlignLeftProps) {
  return <IconWrapper icon={IconComponent} {...props} />;
}

export {
  animations,
  AlignLeft,
  AlignLeft as AlignLeftIcon,
  type AlignLeftProps,
  type AlignLeftProps as AlignLeftIconProps,
};
