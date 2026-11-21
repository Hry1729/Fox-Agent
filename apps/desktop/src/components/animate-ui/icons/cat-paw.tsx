// Animated icon for the 伙伴 (companions) mode tab in the chat sidebar.
//
// Glyph: self-authored, not in the Animate UI registry. One outlined cat paw
// print, tilted 10 degrees: a main pad plus four toe beans (lucide's
// `paw-print` only has three).
// Geometry is mirrored in docs/03-产品与前端/伙伴页面/gen-icons.mjs.
//
// Motion: the paw drops from above at 1.6x, slams down past its resting size,
// rebounds once and settles, so it reads as a paw stamped onto the screen.
'use client';

import * as React from 'react';
import { motion, type Variants } from 'motion/react';

import {
  getVariants,
  useAnimateIconContext,
  IconWrapper,
  type IconProps,
} from '@/components/animate-ui/icons/icon';

type CatPawProps = IconProps<keyof typeof animations>;

const animations = {
  default: {
    paw: {
      initial: { opacity: 1, scale: 1, y: 0, rotate: 0 },
      animate: {
        opacity: [0, 1, 1, 1, 1],
        scale: [1.6, 0.86, 1.06, 0.98, 1],
        y: [-3, 0.6, 0, 0, 0],
        rotate: [-8, 0, 0, 0, 0],
        // Just below the centre of the paw, so the pad lands first.
        transformOrigin: '50% 60%',
        transition: {
          duration: 0.5,
          ease: 'easeOut',
          times: [0, 0.4, 0.62, 0.8, 1],
        },
      },
    },
  } satisfies Record<string, Variants>,
} as const;

// Paw drawn around (0, 0) at unit size, then scaled into the coordinates
// instead of with a scale transform, so the stroke keeps the same width as
// every other icon.
const PAD =
  'M-3.6 2.1C-3.6-.7-1.7-1.7 0-1.7C1.7-1.7 3.6-.7 3.6 2.1C3.6 3.9 2.5 4.6 1.5 4.6C.8 4.6.5 4.2 0 4.2C-.5 4.2-.8 4.6-1.5 4.6C-2.5 4.6-3.6 3.9-3.6 2.1Z';
const TOES = [
  [-4.6, -1.9, -30],
  [-1.75, -4.4, -10],
  [1.75, -4.4, 10],
  [4.6, -1.9, 30],
] as const;
// Fitted so the stroked glyph spans 1..23 on its long side.
const PLACE = { x: 12.634, y: 13.41, rotate: -10, scale: 1.609 };
const TOE = { rx: 1.45, ry: 1.8, spread: 1.03 };

const round = (n: number) => Math.round(n * 1000) / 1000;
const PAD_PATH = PAD.replace(/-?\d*\.?\d+/g, (n) =>
  String(round(Number(n) * PLACE.scale)),
);
const TOE_SHAPES = TOES.map(([x, y, rotate]) => ({
  cx: round(x * PLACE.scale * TOE.spread),
  cy: round(y * PLACE.scale * TOE.spread),
  rx: round(TOE.rx * PLACE.scale),
  ry: round(TOE.ry * PLACE.scale),
  rotate,
}));

function IconComponent({ size, ...props }: CatPawProps) {
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
      {/* Placement stays on a plain <g>: Motion's CSS transform on the
          animated child would override an SVG transform attribute. */}
      <g
        transform={`translate(${PLACE.x} ${PLACE.y}) rotate(${PLACE.rotate})`}
      >
        <motion.g variants={variants.paw} initial="initial" animate={controls}>
          <path d={PAD_PATH} />
          {TOE_SHAPES.map((t) => (
            <ellipse
              key={t.rotate}
              cx={t.cx}
              cy={t.cy}
              rx={t.rx}
              ry={t.ry}
              transform={`rotate(${t.rotate} ${t.cx} ${t.cy})`}
            />
          ))}
        </motion.g>
      </g>
    </motion.svg>
  );
}

function CatPaw(props: CatPawProps) {
  return <IconWrapper icon={IconComponent} {...props} />;
}

export {
  animations,
  CatPaw,
  CatPaw as CatPawIcon,
  type CatPawProps,
  type CatPawProps as CatPawIconProps,
};
