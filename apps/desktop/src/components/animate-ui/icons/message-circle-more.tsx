'use client'

import { motion, type Variants } from 'motion/react'
import { IconWrapper, useAnimateIconContext, type IconProps } from './icon'

const dot: Variants = {
  initial: { y: 0 },
  animate: (index: number) => ({ y: [0, -2, 0], transition: { duration: 0.4, delay: index * 0.1 } }),
}

function IconComponent({ size, ...props }: IconProps<'default'>) {
  const { controls } = useAnimateIconContext()
  return <motion.svg xmlns="http://www.w3.org/2000/svg" width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" {...props}>
    <path d="M7.9 20A9 9 0 1 0 4 16.1L2 22Z" />
    {[8, 12, 16].map((x, index) => <motion.path key={x} d={`M${x} 12h.01`} custom={index} variants={dot} initial="initial" animate={controls} />)}
  </motion.svg>
}

export function MessageCircleMore(props: IconProps<'default'>) {
  return <IconWrapper icon={IconComponent} {...props} />
}
