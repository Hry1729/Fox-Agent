import type { CSSProperties } from 'react'
import './shiny-text.css'

type ShinyTextProps = {
  text: string
  disabled?: boolean
  speed?: number
  className?: string
}

export function ShinyText({
  text,
  disabled = false,
  speed = 5,
  className = '',
}: ShinyTextProps) {
  const style = { animationDuration: `${speed}s` } as CSSProperties

  return (
    <span
      className={`shiny-text ${disabled ? 'disabled' : ''} ${className}`.trim()}
      style={style}
    >
      {text}
    </span>
  )
}
