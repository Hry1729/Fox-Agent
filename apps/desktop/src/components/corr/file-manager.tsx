// Source: https://ui.corr.sh/r/file-manager.json (retrieved 2026-10-15).
"use client"

import * as React from "react"
import {
  DndContext,
  PointerSensor,
  useDraggable,
  useDroppable,
  useSensor,
  useSensors,
  type DragEndEvent,
} from "@dnd-kit/core"
import {
  ArrowLeft,
  Download,
  ChevronDown,
  ChevronRight,
  FileText,
  Folder,
  FolderOpen,
  Grid2X2,
  List,
  MoreHorizontal,
  Rows3,
  Trash2,
  UploadCloud,
} from "lucide-react"
import { AnimatePresence, motion, useReducedMotion } from "motion/react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { cn } from "@/lib/utils"
import {
  FileUploadDropzone,
  type FileUploadDropzoneFile,
} from "./file-upload-dropzone"

export type FileManagerView = "details" | "list" | "cards"
export type FileManagerMobileMode = "auto" | "list" | "sheet"

export type FileManagerItem = {
  id: string
  name: string
  size?: string
  modified?: string
  type?: string
  path?: string
  kind?: "file" | "folder"
}

function normalizePath(path: string) {
  if (!path || path === "/") return "/"
  return `/${path.split("/").filter(Boolean).join("/")}`
}

function joinPath(base: string, name: string) {
  return normalizePath(`${base}/${name}`)
}

function getPathParts(path: string) {
  return normalizePath(path).split("/").filter(Boolean)
}

function getParentPath(path: string) {
  const parts = getPathParts(path)
  return normalizePath(parts.slice(0, -1).join("/"))
}

function getItemPath(item: FileManagerItem) {
  return normalizePath(item.path ?? "/")
}

function getFolderPath(item: FileManagerItem) {
  return item.kind === "folder"
    ? joinPath(getItemPath(item), item.name)
    : getItemPath(item)
}

function getDerivedFolders(files: FileManagerItem[]) {
  const folders = new Set<string>(["/"])

  files.forEach((file) => {
    const path = getItemPath(file)
    const parts = getPathParts(path)

    parts.forEach((_, index) => {
      folders.add(normalizePath(parts.slice(0, index + 1).join("/")))
    })

    if (file.kind === "folder") {
      folders.add(getFolderPath(file))
    }
  })

  return Array.from(folders).sort((left, right) => {
    const a = getPathParts(left), b = getPathParts(right)
    for (let index = 0; index < Math.min(a.length, b.length); index++) {
      const compared = a[index].localeCompare(b[index], 'zh-CN', { numeric: true })
      if (compared) return compared
    }
    return a.length - b.length
  })
}

function getNavigationDirection(currentPath: string, nextPath: string) {
  const current = normalizePath(currentPath)
  const next = normalizePath(nextPath)

  if (next === current) return 0
  if (next.startsWith(`${current}/`)) return 1
  if (current.startsWith(`${next}/`)) return -1

  return getPathParts(next).length >= getPathParts(current).length ? 1 : -1
}

export function FileManager({
  files,
  path,
  defaultPath = "/",
  defaultView = "details",
  view: controlledView,
  contentOnly = false,
  onPathChange,
  onUpload,
  onDelete,
  onOpen,
  onMove,
  mobileMode = "auto",
  className,
  toolbar,
  treeFooter,
  renderFiles,
  filterFiles,
}: {
  files: FileManagerItem[]
  path?: string
  defaultPath?: string
  view?: FileManagerView
  contentOnly?: boolean
  defaultView?: FileManagerView
  mobileMode?: FileManagerMobileMode
  onPathChange?: (path: string) => void
  onUpload?: (path: string, files: FileUploadDropzoneFile[]) => void
  onDelete?: (file: FileManagerItem) => void
  onOpen?: (file: FileManagerItem) => void
  onMove?: (file: FileManagerItem, targetPath: string) => void
  className?: string
  toolbar?: React.ReactNode
  treeFooter?: React.ReactNode
  filterFiles?: (files: FileManagerItem[], path: string) => FileManagerItem[]
  renderFiles?: (context: { files: FileManagerItem[]; view: FileManagerView; onOpen: (file: FileManagerItem) => void }) => React.ReactNode
}) {
  const [internalPath, setInternalPath] = React.useState(defaultPath)
  const [internalView, setView] = React.useState<FileManagerView>(defaultView)
  const view = controlledView ?? internalView
  const [transitionDirection, setTransitionDirection] = React.useState(1)
  const [uploadOpen, setUploadOpen] = React.useState(false)
  const [folderPickerOpen, setFolderPickerOpen] = React.useState(false)
  const [collapsedFolders, setCollapsedFolders] = React.useState(new Set<string>())
  const [uploadFiles, setUploadFiles] = React.useState<
    FileUploadDropzoneFile[]
  >([])
  const reduceMotion = useReducedMotion()
  const sensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: {
        delay: 160,
        tolerance: 6,
      },
    })
  )
  const currentPath = normalizePath(path ?? internalPath)
  const forceMobileLayout = mobileMode === "list" || mobileMode === "sheet"
  const compactFolderNavigation = mobileMode === "sheet"
  const folders = React.useMemo(() => contentOnly ? [] : getDerivedFolders(files), [contentOnly, files])
  const visibleFiles = React.useMemo(
    () => filterFiles ? filterFiles(files, currentPath) : files.filter((file) => getItemPath(file) === currentPath),
    [currentPath, files, filterFiles]
  )
  const visibleView =
    forceMobileLayout && view === "details" ? "list" : view
  const viewOptions = [
    { value: "details", icon: Rows3, label: "详细列表" },
    { value: "list", icon: List, label: "紧凑列表" },
    { value: "cards", icon: Grid2X2, label: "网格视图" },
  ].filter((item) => !forceMobileLayout || item.value !== "details")

  function setPath(nextPath: string) {
    const normalized = normalizePath(nextPath)
    setTransitionDirection(getNavigationDirection(currentPath, normalized))
    setInternalPath(normalized)
    onPathChange?.(normalized)
  }

  function openItem(file: FileManagerItem) {
    if (file.kind === "folder") {
      setPath(getFolderPath(file))
      return
    }
    onOpen?.(file)
  }

  const isUploading = uploadFiles.some((file) => file.status === "uploading")
  const uploadableFiles = uploadFiles.filter((file) => file.status !== "error")

  function submitUpload() {
    if (uploadableFiles.length === 0 || isUploading) return
    onUpload?.(currentPath, uploadableFiles)
    setUploadOpen(false)
    setUploadFiles([])
  }

  function handleDragEnd(event: DragEndEvent) {
    const activeId = String(event.active.id)
    const overId = event.over ? String(event.over.id) : null

    if (!overId?.startsWith("folder:")) return

    const item = files.find((file) => file.id === activeId)
    if (!item) return

    const targetPath = normalizePath(overId.replace("folder:", ""))
    const currentItemPath = getItemPath(item)

    if (targetPath === currentItemPath) return

    if (item.kind === "folder") {
      const sourceFolderPath = getFolderPath(item)
      if (
        targetPath === sourceFolderPath ||
        targetPath.startsWith(`${sourceFolderPath}/`)
      ) {
        return
      }
    }

    onMove?.(item, targetPath)
  }

  const mobileFolderList = (
    <div className="space-y-1">
      {folders.map((folder) => {
        const active = folder === currentPath
        const label = folder === "/" ? "全部资料" : getPathParts(folder).at(-1)
        const depth = getPathParts(folder).length

        return (
          <FileManagerTreeFolder
            key={folder}
            folder={folder}
            label={label ?? "Folder"}
            depth={depth}
            active={active}
            onClick={() => {
              setPath(folder)
              setFolderPickerOpen(false)
            }}
          />
        )
      })}
    </div>
  )

  if (contentOnly && renderFiles) return <div className={className}>{renderFiles({ files: visibleFiles, view: visibleView, onOpen: openItem })}</div>

  return (
    <DndContext sensors={sensors} onDragEnd={handleDragEnd}>
      <div
        className={cn("overflow-hidden rounded-md border bg-card", className)}
      >
        <div className="border-b p-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div className="flex min-w-0 items-center gap-2">
              <Button
                type="button"
                variant="ghost"
                size="icon-sm"
                disabled={currentPath === "/"}
                onClick={() => setPath(getParentPath(currentPath))}
              >
                <ArrowLeft />
                <span className="sr-only">返回上级目录</span>
              </Button>
              {compactFolderNavigation ? (
                <FileManagerPathMenu
                  folders={folders}
                  currentPath={currentPath}
                  onSelect={setPath}
                />
              ) : (
                <div className="flex min-w-0 flex-wrap items-center gap-1 text-sm font-medium">
                  <button
                    type="button"
                    className="rounded px-1 py-0.5 hover:bg-muted"
                    onClick={() => setPath("/")}
                  >
                    全部资料
                  </button>
                  {getPathParts(currentPath).map((part, index, parts) => (
                    <React.Fragment key={`${part}-${index}`}>
                      <span className="text-muted-foreground">/</span>
                      <button
                        type="button"
                        className="rounded px-1 py-0.5 hover:bg-muted"
                        onClick={() =>
                          setPath(parts.slice(0, index + 1).join("/"))
                        }
                      >
                        {part}
                      </button>
                    </React.Fragment>
                  ))}
                </div>
              )}
            </div>
            <div className="flex items-center gap-2">
              {toolbar}
              {mobileMode === "list" || compactFolderNavigation ? null : (
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  className={cn(!forceMobileLayout && "md:hidden")}
                  onClick={() => setFolderPickerOpen(true)}
                >
                  <FolderOpen />
                  目录
                </Button>
              )}
              <div className="flex rounded-md border bg-background p-0.5">
                {viewOptions.map((item) => {
                  const Icon = item.icon

                  return (
                    <Button
                      key={item.value}
                      variant={
                        visibleView === item.value ? "secondary" : "ghost"
                      }
                      size="icon-sm"
                      aria-label={item.label}
                      aria-pressed={visibleView === item.value}
                      onClick={() => setView(item.value as FileManagerView)}
                    >
                      <Icon />
                      <span className="sr-only">{item.label}</span>
                    </Button>
                  )
                })}
              </div>
              {onUpload && <Button size="sm" onClick={() => setUploadOpen(true)}>
                <UploadCloud />
                添加文件
              </Button>}
            </div>
          </div>
          <div className="mt-2 pl-10 text-xs text-muted-foreground">
            {visibleFiles.length}{" "}
            个项目
          </div>
        </div>

        {mobileMode === "list" ? (
          <div className="border-b p-2 md:hidden">
            <div className="scrollbar-none flex gap-1 overflow-x-auto">
              {folders.map((folder) => {
                const active = folder === currentPath
                const label =
                  folder === "/" ? "全部资料" : getPathParts(folder).at(-1)

                return (
                  <Button
                    key={folder}
                    type="button"
                    variant={active ? "secondary" : "ghost"}
                    size="sm"
                    className="shrink-0"
                    onClick={() => setPath(folder)}
                  >
                    {active ? <FolderOpen /> : <Folder />}
                    {label}
                  </Button>
                )
              })}
            </div>
          </div>
        ) : null}

        <div
          className={cn(
            "grid min-h-80",
            !forceMobileLayout && "md:grid-cols-[13rem_minmax(0,1fr)]"
          )}
        >
          <aside aria-label="资料目录"
            className={cn(
              "hidden border-b bg-muted/20 p-2 md:block md:border-r md:border-b-0",
              forceMobileLayout && "md:hidden"
            )}
          >
            <div className="space-y-1" role="tree" aria-label="文件夹">
              {folders.filter((folder) => ![...collapsedFolders].some((parent) => parent === '/' ? folder !== '/' : folder.startsWith(`${parent}/`))).map((folder) => {
                const active = folder === currentPath
                const label =
                  folder === "/" ? "全部资料" : getPathParts(folder).at(-1)
                const depth = getPathParts(folder).length

                return (
                  <FileManagerTreeFolder
                    key={folder}
                    folder={folder}
                    label={label ?? "Folder"}
                    depth={depth}
                    active={active}
                    expanded={folders.some((child) => child !== folder && (folder === '/' || child.startsWith(`${folder}/`))) ? !collapsedFolders.has(folder) : undefined}
                    onToggle={() => setCollapsedFolders((current) => { const next = new Set(current); if (next.has(folder)) next.delete(folder); else next.add(folder); return next })}
                    onClick={() => setPath(folder)}
                  />
                )
              })}
            </div>
            {treeFooter}
          </aside>

          <main className={cn("min-w-0 p-3", forceMobileLayout && "p-2")}>
            <AnimatePresence mode="wait" initial={false}>
              <motion.div
                key={`${currentPath}-${visibleView}`}
                initial={{
                  opacity: 0,
                  x: reduceMotion ? 0 : transitionDirection * 12,
                }}
                animate={{ opacity: 1, x: 0 }}
                exit={{
                  opacity: 0,
                  x: reduceMotion ? 0 : transitionDirection * -12,
                }}
                transition={{ duration: reduceMotion ? 0 : 0.16 }}
              >
                {renderFiles ? renderFiles({ files: visibleFiles, view: visibleView, onOpen: openItem }) : visibleFiles.length === 0 ? (
                  <div className="rounded-md border border-dashed px-4 py-10 text-center text-sm text-muted-foreground">
                    这个文件夹还是空的。
                  </div>
                ) : visibleView === "details" ? (
                  <div className="overflow-hidden rounded-md border">
                    <div className="grid grid-cols-[minmax(0,1fr)_7rem_8rem_2rem] gap-3 border-b bg-muted/30 px-3 py-2 text-xs font-medium text-muted-foreground">
                      <div>名称</div>
                      <div>大小</div>
                      <div>修改时间</div>
                      <div />
                    </div>
                    {visibleFiles.map((file) => (
                      <FileManagerRow
                        key={file.id}
                        file={file}
                        view="details"
                        onOpen={openItem}
                        onDelete={onDelete}
                        draggable={Boolean(onMove)}
                      />
                    ))}
                  </div>
                ) : (
                  <div
                    className={cn(
                      "grid gap-2",
                      visibleView === "cards" &&
                        (forceMobileLayout
                          ? "grid-cols-1"
                          : "sm:grid-cols-2 lg:grid-cols-3")
                    )}
                  >
                    {visibleFiles.map((file) => (
                      <FileManagerRow
                        key={file.id}
                        file={file}
                        view={visibleView}
                        onOpen={openItem}
                        onDelete={onDelete}
                        draggable={Boolean(onMove)}
                      />
                    ))}
                  </div>
                )}
              </motion.div>
            </AnimatePresence>
          </main>
        </div>

        <Dialog open={folderPickerOpen} onOpenChange={setFolderPickerOpen}>
          <DialogContent className="sm:max-w-sm">
            <DialogHeader>
              <DialogTitle>选择目录</DialogTitle>
              <DialogDescription>
                Move through the file tree on smaller screens.
              </DialogDescription>
            </DialogHeader>
            {mobileFolderList}
          </DialogContent>
        </Dialog>

        <Dialog
          open={uploadOpen}
          onOpenChange={(open) => {
            setUploadOpen(open)
            if (!open) setUploadFiles([])
          }}
        >
          <DialogContent className="sm:max-w-md">
            <DialogHeader>
              <DialogTitle>Upload files</DialogTitle>
              <DialogDescription>
                Add files to {currentPath === "/" ? "Files" : currentPath}.
              </DialogDescription>
            </DialogHeader>
            <FileUploadDropzone
              files={uploadFiles}
              onFilesChange={setUploadFiles}
              simulateUpload
              label="Drop files to upload"
              description="Uploads are added to the active directory"
            />
            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                onClick={() => setUploadOpen(false)}
              >
                Cancel
              </Button>
              <Button
                type="button"
                onClick={submitUpload}
                disabled={uploadableFiles.length === 0 || isUploading}
              >
                Upload files
              </Button>
            </DialogFooter>
          </DialogContent>
        </Dialog>
      </div>
    </DndContext>
  )
}

function FileManagerPathMenu({
  folders,
  currentPath,
  onSelect,
}: {
  folders: string[]
  currentPath: string
  onSelect: (path: string) => void
}) {
  const parts = getPathParts(currentPath)
  const currentLabel =
    currentPath === "/" ? "Files" : parts.at(-1) ?? "Current folder"

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-9 max-w-56 justify-start gap-2 px-2"
        >
          <FolderOpen className="size-4 shrink-0" />
          <span className="min-w-0 truncate text-sm font-medium">
            {currentLabel}
          </span>
          <ChevronDown className="ml-auto size-3.5 shrink-0 text-muted-foreground" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-64">
        <DropdownMenuLabel>Path</DropdownMenuLabel>
        <DropdownMenuItem onClick={() => onSelect("/")}>
          <Folder className="size-4" />
          Files
        </DropdownMenuItem>
        {parts.map((part, index) => {
          const path = normalizePath(parts.slice(0, index + 1).join("/"))

          return (
            <DropdownMenuItem key={`${part}-${path}`} onClick={() => onSelect(path)}>
              <FolderOpen className="size-4" />
              {part}
            </DropdownMenuItem>
          )
        })}
        <DropdownMenuSeparator />
        <DropdownMenuLabel>Jump to folder</DropdownMenuLabel>
        <div className="max-h-56 overflow-y-auto">
          {folders.map((folder) => {
            const active = folder === currentPath
            const label = folder === "/" ? "Files" : getPathParts(folder).at(-1)
            const depth = getPathParts(folder).length

            return (
              <DropdownMenuItem
                key={folder}
                onClick={() => onSelect(folder)}
                className={cn(active && "bg-muted font-medium")}
                style={{ paddingLeft: `${0.5 + depth * 0.75}rem` }}
              >
                {active ? (
                  <FolderOpen className="size-4" />
                ) : (
                  <Folder className="size-4" />
                )}
                <span className="truncate">{label}</span>
              </DropdownMenuItem>
            )
          })}
        </div>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

function FileManagerTreeFolder({
  folder,
  label,
  depth,
  active,
  onClick,
  expanded,
  onToggle,
}: {
  folder: string
  label: string
  depth: number
  active: boolean
  onClick: () => void
  expanded?: boolean
  onToggle?: () => void
}) {
  const { isOver, setNodeRef } = useDroppable({
    id: `folder:${folder}`,
    data: { folder },
  })

  return (
    <div className="flex min-w-0 items-center" style={{ paddingLeft: `${depth * 0.75}rem` }}>
      {onToggle && <button type="button" className="flex size-5 shrink-0 items-center justify-center rounded text-muted-foreground" aria-label={`${expanded === false ? '展开' : '折叠'} ${label}`} disabled={expanded === undefined} onClick={onToggle}>{expanded !== undefined && (expanded ? <ChevronDown className="size-3" /> : <ChevronRight className="size-3" />)}</button>}
    <button
      ref={setNodeRef}
      type="button"
      role={onToggle ? 'treeitem' : undefined}
      aria-level={onToggle ? depth + 1 : undefined}
      aria-expanded={expanded}
      aria-selected={active}
      onClick={onClick}
      className={cn(
        "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-xs transition-colors hover:bg-muted",
        active && "bg-background font-medium shadow-xs",
        isOver && "bg-primary/10 text-primary ring-1 ring-primary/20"
      )}
      style={{ paddingLeft: onToggle ? '0.35rem' : `${0.5 + depth * 0.75}rem` }}
    >
      {active ? (
        <FolderOpen className="size-3.5 text-muted-foreground" />
      ) : (
        <Folder className="size-3.5 text-muted-foreground" />
      )}
      <span className="truncate">{label}</span>
    </button>
    </div>
  )
}

function FileManagerRow({
  file,
  view,
  onOpen,
  onDelete,
  draggable = false,
}: {
  file: FileManagerItem
  view: FileManagerView
  onOpen: (file: FileManagerItem) => void
  onDelete?: (file: FileManagerItem) => void
  draggable?: boolean
}) {
  const isFolder = file.kind === "folder"
  const Icon = isFolder ? Folder : FileText
  const { attributes, isDragging, listeners, setNodeRef } = useDraggable({
    id: file.id,
    data: { file },
    disabled: !draggable,
  })

  if (view === "details") {
    return (
      <div
        ref={setNodeRef}
        data-dragging={isDragging}
        className="grid grid-cols-[minmax(0,1fr)_7rem_8rem_2rem] items-center gap-3 border-b px-3 py-2 opacity-100 last:border-b-0 data-[dragging=true]:opacity-50"
        {...attributes}
        {...listeners}
      >
        <button
          type="button"
          onClick={() => onOpen(file)}
          className="flex min-w-0 items-center gap-2 text-left"
        >
          <Icon className="size-4 shrink-0 text-muted-foreground" />
          <span className="truncate text-sm font-medium">{file.name}</span>
        </button>
        <div className="text-xs text-muted-foreground">
          {isFolder ? "--" : file.size}
        </div>
        <div className="text-xs text-muted-foreground">{file.modified}</div>
        <FileManagerActions file={file} onDelete={onDelete} />
      </div>
    )
  }

  return (
    <div
      ref={setNodeRef}
      data-dragging={isDragging}
      className={cn(
        "flex items-center gap-3 rounded-md border bg-background p-3",
        view === "cards" && "items-start",
        isDragging && "opacity-50"
      )}
      {...attributes}
      {...listeners}
    >
      <button
        type="button"
        onClick={() => onOpen(file)}
        className="flex min-w-0 flex-1 items-center gap-3 text-left"
      >
        <div className="flex size-9 shrink-0 items-center justify-center rounded-md border bg-muted/40">
          <Icon className="size-4 text-muted-foreground" />
        </div>
        <div className="min-w-0">
          <div className="truncate text-sm font-medium">{file.name}</div>
          <div className="mt-1 text-xs text-muted-foreground">
            {isFolder ? "Folder" : file.size} · {file.modified}
          </div>
        </div>
      </button>
      <FileManagerActions file={file} onDelete={onDelete} />
    </div>
  )
}

function FileManagerActions({
  file,
  onDelete,
}: {
  file: FileManagerItem
  onDelete?: (file: FileManagerItem) => void
}) {
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm">
          <MoreHorizontal />
          <span className="sr-only">Open file actions</span>
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        <DropdownMenuItem>
          <Download />
          Download
        </DropdownMenuItem>
        <DropdownMenuItem
          variant="destructive"
          onClick={() => onDelete?.(file)}
        >
          <Trash2 />
          Delete
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
