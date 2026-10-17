// Source: https://ui.corr.sh/r/animated-progress.json (retrieved 2026-10-15).
"use client"

import { motion, useReducedMotion } from "motion/react"

import { cn } from "@/lib/utils"
import { AnimatedNumber } from "./animated-number"

export type AnimatedProgressTone = "neutral" | "success" | "warning"

const toneClassName: Record<AnimatedProgressTone, string> = {
  neutral: "bg-primary",
  success: "bg-emerald-500",
  warning: "bg-amber-500",
}

function clampProgress(value: number) {
  return Math.min(100, Math.max(0, value))
}

export function AnimatedProgress({
  value,
  label,
  showValue = false,
  tone = "neutral",
  className,
}: {
  value?: number
  label?: string
  showValue?: boolean
  tone?: AnimatedProgressTone
  className?: string
}) {
  const reduceMotion = useReducedMotion()
  const isIndeterminate = value === undefined
  const progress = clampProgress(value ?? 0)

  return (
    <div className={cn("w-full space-y-2", className)}>
      {(label || showValue) && (
        <div className="flex items-center justify-between gap-3 text-xs font-medium">
          {label ? <span>{label}</span> : <span />}
          {showValue && !isIndeterminate ? (
            <span className="flex items-center font-mono text-muted-foreground">
              <AnimatedNumber value={Math.round(progress)} />
              <span>%</span>
            </span>
          ) : null}
        </div>
      )}
      <div className="h-2 overflow-hidden rounded-full bg-muted">
        {isIndeterminate ? (
          <motion.div
            className={cn("h-full w-1/3 rounded-full", toneClassName[tone])}
            animate={
              reduceMotion
                ? { x: "100%" }
                : {
                    x: ["-130%", "105%", "340%"],
                    scaleX: [0.45, 1, 0.45],
                  }
            }
            transition={{
              duration: 1.85,
              ease: [0.65, 0, 0.35, 1],
              repeat: reduceMotion ? 0 : Infinity,
            }}
          />
        ) : (
          <motion.div
            className={cn("h-full rounded-full", toneClassName[tone])}
            initial={false}
            animate={{ width: `${progress}%` }}
            transition={
              reduceMotion
                ? { duration: 0 }
                : { duration: 0.45, ease: [0.22, 1, 0.36, 1] }
            }
          />
        )}
      </div>
    </div>
  )
}
