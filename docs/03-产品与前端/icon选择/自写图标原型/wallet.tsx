// Animated icon for 本机资料库 (local knowledge base) in the knowledge sidebar.
//
// Glyph: lucide `wallet`. This is a DEPARTURE from the `database` glyph the row
// used before — the user chose the wallet deliberately, so the change of shape
// is intended rather than a drift. Both paths are taken verbatim from the
// registry component the user supplied; only the wrapper differs.
//
// Motion: the whole wallet lifts and tips 4 degrees, then settles.
'use client';

import * as React from 'react';
import { motion, type Variants } from 'motion/react';

import {
  getVariants,
  useAnimateIconContext,
  IconWrapper,
  type IconProps,
} from '@/components/animate-ui/icons/icon';

type WalletProps = IconProps<keyof typeof animations>;

const animations = {
  default: {
    // Whole-glyph motion, so one variant on the <svg> drives both paths.
    group: {
      initial: { y: 0, rotate: 0 },
      animate: {
        y: [0, -3, 0],
        rotate: [0, -4, 0],
        transformOrigin: '12px 12px',
        transition: { duration: 0.55, ease: 'easeInOut', times: [0, 0.45, 1] },
      },
    },
    body: {},
    fold: {},
  } satisfies Record<string, Variants>,
} as const;

function IconComponent({ size, ...props }: WalletProps) {
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
      variants={variants.group}
      initial="initial"
      animate={controls}
      {...props}
    >
      <motion.path
        d="M19 7V4a1 1 0 0 0-1-1H5a2 2 0 0 0 0 4h15a1 1 0 0 1 1 1v4h-3a2 2 0 0 0 0 4h3a1 1 0 0 0 1-1v-2a1 1 0 0 0-1-1"
        variants={variants.body}
        initial="initial"
        animate={controls}
      />
      <motion.path
        d="M3 5v14a2 2 0 0 0 2 2h15a1 1 0 0 0 1-1v-4"
        variants={variants.fold}
        initial="initial"
        animate={controls}
      />
    </motion.svg>
  );
}

function Wallet(props: WalletProps) {
  return <IconWrapper icon={IconComponent} {...props} />;
}

export {
  animations,
  Wallet,
  Wallet as WalletIcon,
  type WalletProps,
  type WalletProps as WalletIconProps,
};
