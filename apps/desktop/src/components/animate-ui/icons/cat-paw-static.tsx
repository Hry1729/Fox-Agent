import type { SVGProps } from 'react';

const PAD =
  'M-3.6 2.1C-3.6-.7-1.7-1.7 0-1.7C1.7-1.7 3.6-.7 3.6 2.1C3.6 3.9 2.5 4.6 1.5 4.6C.8 4.6.5 4.2 0 4.2C-.5 4.2-.8 4.6-1.5 4.6C-2.5 4.6-3.6 3.9-3.6 2.1Z';
const TOES = [
  [-4.6, -1.9, -30],
  [-1.75, -4.4, -10],
  [1.75, -4.4, 10],
  [4.6, -1.9, 30],
] as const;
const PLACE = { x: 12.634, y: 13.41, rotate: -10, scale: 1.609 };
const TOE = { rx: 1.45, ry: 1.8, spread: 1.03 };
const round = (n: number) => Math.round(n * 1000) / 1000;
const PAD_PATH = PAD.replace(/-?\d*\.?\d+/g, (n) => String(round(Number(n) * PLACE.scale)));
const TOE_SHAPES = TOES.map(([x, y, rotate]) => ({
  cx: round(x * PLACE.scale * TOE.spread),
  cy: round(y * PLACE.scale * TOE.spread),
  rx: round(TOE.rx * PLACE.scale),
  ry: round(TOE.ry * PLACE.scale),
  rotate,
}));

export function StaticCatPaw({ size = 28, ...props }: SVGProps<SVGSVGElement> & { size?: number | string }) {
  return (
    <svg
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
      <g transform={`translate(${PLACE.x} ${PLACE.y}) rotate(${PLACE.rotate})`}>
        <path d={PAD_PATH} />
        {TOE_SHAPES.map((toe) => (
          <ellipse
            key={toe.rotate}
            cx={toe.cx}
            cy={toe.cy}
            rx={toe.rx}
            ry={toe.ry}
            transform={`rotate(${toe.rotate} ${toe.cx} ${toe.cy})`}
          />
        ))}
      </g>
    </svg>
  );
}
