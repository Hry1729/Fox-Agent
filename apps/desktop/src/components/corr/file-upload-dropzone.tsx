// Source: https://ui.corr.sh/r/file-upload-dropzone.json (retrieved 2026-10-15).
"use client"

import * as React from "react"
import { Check, FileText, UploadCloud, X } from "lucide-react"

import { Button } from "@/components/ui/button"
import { cn } from "@/lib/utils"
import { AnimatedProgress } from "./animated-progress"

export type FileUploadDropzoneFile = {
  id: string
  file: File
  status?: "idle" | "uploading" | "success" | "error"
  progress?: number
  error?: string
}

function getFileExtension(fileName: string) {
  const extension = fileName.split(".").pop()
  return extension && extension !== fileName
    ? extension.slice(0, 4).toUpperCase()
    : "FILE"
}

export function FileUploadDropzone({
  files,
  onFilesChange,
  accept,
  multiple = true,
  maxFiles = 6,
  label = "Drop files here",
  description = "or click to browse from your device",
  disabled = false,
  simulateUpload = false,
  className,
}: {
  files: FileUploadDropzoneFile[]
  onFilesChange: (files: FileUploadDropzoneFile[]) => void
  accept?: string
  multiple?: boolean
  maxFiles?: number
  label?: string
  description?: string
  disabled?: boolean
  simulateUpload?: boolean
  className?: string
}) {
  const inputRef = React.useRef<HTMLInputElement>(null)
  const [dragging, setDragging] = React.useState(false)

  function addFiles(nextFiles: FileList | File[]) {
    if (disabled) return

    const incoming = Array.from(nextFiles).map((file, index) => ({
      id: `${file.name}-${file.lastModified}-${crypto.randomUUID()}`,
      file,
      status: simulateUpload ? ("uploading" as const) : ("idle" as const),
      progress: simulateUpload ? 12 + index * 8 : undefined,
    }))
    const merged = multiple ? [...files, ...incoming] : incoming.slice(0, 1)
    onFilesChange(merged.slice(0, maxFiles))
  }

  React.useEffect(() => {
    if (!simulateUpload) return
    if (!files.some((item) => item.status === "uploading")) return

    const timer = window.setInterval(() => {
      onFilesChange(
        files.map((item) => {
          if (item.status !== "uploading") return item

          const nextProgress = Math.min(100, (item.progress ?? 0) + 18)
          const failed = item.file.name.toLowerCase().includes("fail")

          if (nextProgress >= 100) {
            return {
              ...item,
              progress: 100,
              status: failed ? ("error" as const) : ("success" as const),
              error: failed ? "Upload failed" : undefined,
            }
          }

          return { ...item, progress: nextProgress }
        })
      )
    }, 450)

    return () => window.clearInterval(timer)
  }, [files, onFilesChange, simulateUpload])

  return (
    <div className={cn("grid gap-3", className)}>
      <button
        type="button"
        disabled={disabled}
        onClick={() => inputRef.current?.click()}
        onDragEnter={(event) => {
          event.preventDefault()
          if (!disabled) setDragging(true)
        }}
        onDragOver={(event) => event.preventDefault()}
        onDragLeave={(event) => {
          event.preventDefault()
          setDragging(false)
        }}
        onDrop={(event) => {
          event.preventDefault()
          setDragging(false)
          addFiles(event.dataTransfer.files)
        }}
        className={cn(
          "flex min-h-36 w-full flex-col items-center justify-center rounded-md border border-dashed bg-card p-6 text-center transition-colors outline-none",
          "hover:bg-muted/40 focus-visible:border-ring focus-visible:ring-2 focus-visible:ring-ring/30",
          dragging && "border-primary bg-primary/5",
          disabled && "cursor-not-allowed opacity-50"
        )}
      >
        <input
          ref={inputRef}
          type="file"
          accept={accept}
          multiple={multiple}
          disabled={disabled}
          className="sr-only"
          onChange={(event) => {
            if (event.target.files) addFiles(event.target.files)
            event.target.value = ""
          }}
        />
        {files.length > 0 ? (
          <div className="grid w-full gap-2 text-left">
            {files.map((item) => (
              <div
                key={item.id}
                className="grid gap-2 rounded-md border bg-background/80 px-3 py-2 text-sm"
              >
                <div className="flex min-w-0 items-center justify-between gap-3">
                  <div className="flex min-w-0 items-center gap-3">
                    <span className="relative flex size-9 shrink-0 items-center justify-center rounded-md border bg-muted/40">
                      <FileText className="size-4 text-muted-foreground" />
                      <span className="absolute -right-1 -bottom-1 rounded-sm bg-foreground px-1 py-0.5 text-[0.5rem] leading-none font-semibold text-background">
                        {getFileExtension(item.file.name)}
                      </span>
                    </span>
                    <span className="min-w-0">
                      <span className="block truncate font-medium">
                        {item.file.name}
                      </span>
                      <span className="block text-xs text-muted-foreground">
                        {(item.file.size / 1024).toFixed(1)} KB
                      </span>
                    </span>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    {item.status === "success" ? (
                      <span className="flex size-7 items-center justify-center rounded-md bg-emerald-500/10 text-emerald-600">
                        <Check className="size-3.5" />
                      </span>
                    ) : item.status === "error" ? (
                      <span className="flex size-7 items-center justify-center rounded-md bg-destructive/10 text-destructive">
                        <X className="size-3.5" />
                      </span>
                    ) : null}
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon-sm"
                      onClick={(event) => {
                        event.stopPropagation()
                        onFilesChange(
                          files.filter((file) => file.id !== item.id)
                        )
                      }}
                    >
                      <X />
                      <span className="sr-only">Remove {item.file.name}</span>
                    </Button>
                  </div>
                </div>
                {item.status === "uploading" ? (
                  <AnimatedProgress value={item.progress ?? 0} showValue />
                ) : item.status === "error" && item.error ? (
                  <div className="text-xs text-destructive">{item.error}</div>
                ) : null}
              </div>
            ))}
          </div>
        ) : (
          <>
            <span className="flex size-10 items-center justify-center rounded-md border bg-background">
              <UploadCloud className="size-4 text-muted-foreground" />
            </span>
            <span className="mt-3 text-sm font-medium">{label}</span>
            <span className="mt-1 text-xs text-muted-foreground">
              {description}
            </span>
          </>
        )}
      </button>
    </div>
  )
}
