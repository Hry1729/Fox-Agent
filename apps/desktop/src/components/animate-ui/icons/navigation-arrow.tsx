'use client'

import { motion } from 'motion/react'
import { IconWrapper, useAnimateIconContext, type IconProps } from './icon'

function Arrow({ size, direction, ...props }: IconProps<'default'> & { direction: -1 | 1 }) {
  const { controls } = useAnimateIconContext()
  return <motion.svg xmlns="http://www.w3.org/2000/svg" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" {...props}>
    <motion.g initial="initial" animate={controls} variants={{ initial: { x: 0 }, animate: { x: [0, direction * 3, 0], transition: { duration: 0.4, ease: 'easeInOut' } } }}>
      <path d={direction === -1 ? 'm12 19-7-7 7-7M5 12h14' : 'M5 12h14m-7-7 7 7-7 7'} />
    </motion.g>
  </motion.svg>
}

function Left(props: IconProps<'default'>) { return <Arrow {...props} direction={-1} /> }
function Right(props: IconProps<'default'>) { return <Arrow {...props} direction={1} /> }

export function ArrowLeft(props: IconProps<'default'>) { return <IconWrapper icon={Left} {...props} /> }
export function ArrowRight(props: IconProps<'default'>) { return <IconWrapper icon={Right} {...props} /> }
