import { forwardRef, useEffect, useImperativeHandle, useRef, useState } from 'react'

export interface KnowledgeGraphCanvasNode {
  id: string
  label: string
  type: string
  properties: Record<string, unknown>
}

export interface KnowledgeGraphCanvasEdge {
  id: string
  source: string
  target: string
  label: string
  properties: Record<string, unknown>
}

export interface KnowledgeGraphCanvasHandle {
  fitView: () => Promise<void>
  zoomBy: (delta: number) => Promise<void>
  getZoom: () => number
}

interface KnowledgeGraphCanvasProps {
  nodes: KnowledgeGraphCanvasNode[]
  edges: KnowledgeGraphCanvasEdge[]
  selectedId: string | null
  onSelect: (id: string | null) => void
  onExpand: (node: KnowledgeGraphCanvasNode) => void
  onZoomChange?: (zoom: number) => void
}

type G6Graph = import('@antv/g6').Graph
type G6Datum = { id?: string; data?: Record<string, unknown> }
type G6PointerEvent = { target?: { id?: string | number } }

const MIN_ZOOM = 0.18
const MAX_ZOOM = 2

export const KnowledgeGraphCanvas = forwardRef<KnowledgeGraphCanvasHandle, KnowledgeGraphCanvasProps>(function KnowledgeGraphCanvas({ nodes, edges, selectedId, onSelect, onExpand, onZoomChange }, ref) {
  const containerRef = useRef<HTMLDivElement>(null)
  const graphRef = useRef<G6Graph | null>(null)
  const nodesRef = useRef(nodes)
  const selectedRef = useRef(selectedId)
  const onSelectRef = useRef(onSelect)
  const onExpandRef = useRef(onExpand)
  const onZoomChangeRef = useRef(onZoomChange)
  const [ready, setReady] = useState(false)

  nodesRef.current = nodes
  selectedRef.current = selectedId
  onSelectRef.current = onSelect
  onExpandRef.current = onExpand
  onZoomChangeRef.current = onZoomChange

  useImperativeHandle(ref, () => ({
    fitView: async () => {
      const graph = graphRef.current
      if (!graph || nodesRef.current.length === 0) return
      await graph.fitView({ when: 'always', direction: 'both' }, { duration: 220 })
      onZoomChangeRef.current?.(graph.getZoom())
    },
    zoomBy: async (delta) => {
      const graph = graphRef.current
      if (!graph) return
      const zoom = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, graph.getZoom() + delta))
      await graph.zoomTo(zoom, { duration: 160 })
      onZoomChangeRef.current?.(graph.getZoom())
    },
    getZoom: () => graphRef.current?.getZoom() ?? 1,
  }), [])

  useEffect(() => {
    let disposed = false
    let resizeObserver: ResizeObserver | null = null

    void import('@antv/g6').then(({ Graph }) => {
      const container = containerRef.current
      if (disposed || !container) return

      const colors = graphColors()
      const graph = new Graph({
        container,
        autoResize: true,
        devicePixelRatio: Math.max(1, window.devicePixelRatio || 1),
        zoomRange: [MIN_ZOOM, MAX_ZOOM],
        layout: {
          type: 'd3-force',
          preventOverlap: true,
          alphaDecay: 0.08,
          alphaMin: 0.01,
          velocityDecay: 0.62,
          iterations: 220,
          force: {
            center: { x: 0.5, y: 0.5, strength: 0.12 },
            charge: { strength: -760, distanceMax: 900 },
            link: { distance: 190, strength: 0.72 },
          },
          collide: { radius: 108, strength: 1, iterations: 5 },
        },
        node: {
          // Native canvas shapes stay sharp at fractional zoom levels in WebView2.
          type: 'rect',
          style: {
            size: (datum: G6Datum) => [graphNodeWidth(String(datum.data?.label ?? '')), 58],
            radius: 8,
            fill: colors.card,
            stroke: colors.nodeBorder,
            lineWidth: 1.25,
            shadowColor: colors.shadow,
            shadowBlur: 10,
            shadowOffsetY: 3,
            cursor: 'pointer',
            icon: false,
            port: false,
            labelText: (datum: G6Datum) => String(datum.data?.label ?? ''),
            labelPlacement: 'center',
            labelOffsetY: -9,
            labelFill: colors.text,
            labelFontFamily: 'system-ui, sans-serif',
            labelFontSize: 14,
            labelLineHeight: 18,
            labelFontWeight: 600,
            labelMaxWidth: (datum: G6Datum) => graphNodeWidth(String(datum.data?.label ?? '')) - 28,
            labelWordWrap: true,
            labelMaxLines: 1,
            labelTextOverflow: '...',
            badges: (datum: G6Datum) => [{
              text: String(datum.data?.type ?? 'Entity'),
              placement: 'bottom',
              offsetY: -10,
              fill: colors.muted,
              fontFamily: 'system-ui, sans-serif',
              fontSize: 12,
              lineHeight: 14,
              fontWeight: 400,
              maxLines: 1,
              wordWrap: true,
              wordWrapWidth: graphNodeWidth(String(datum.data?.label ?? '')) - 28,
              textOverflow: '...',
              background: false,
            }],
          },
          state: {
            selected: {
              opacity: 1,
              zIndex: 20,
              stroke: colors.accent,
              lineWidth: 1.75,
              shadowColor: colors.accentShadow,
              shadowBlur: 12,
            },
            neighbor: { opacity: 1, zIndex: 10, stroke: colors.neighbor },
            inactive: { opacity: 0.16 },
          },
        },
        edge: {
          type: 'quadratic',
          style: {
            stroke: colors.accent,
            opacity: 0.22,
            lineWidth: 1.15,
            endArrow: true,
          },
          state: {
            active: { opacity: 0.78, lineWidth: 1.6, zIndex: 4 },
            inactive: { opacity: 0.035 },
          },
        },
        behaviors: [
          'drag-element',
          'drag-canvas',
          { type: 'zoom-canvas', sensitivity: 0.65 },
        ],
        data: graphData(nodesRef.current, edges),
      })

      graphRef.current = graph
      graph.on('node:click', (event) => {
        const id = String((event as G6PointerEvent).target?.id ?? '')
        if (id) onSelectRef.current(id)
      })
      graph.on('node:dblclick', (event) => {
        const id = String((event as G6PointerEvent).target?.id ?? '')
        const node = nodesRef.current.find((item) => item.id === id)
        if (node) onExpandRef.current(node)
      })
      graph.on('canvas:click', () => onSelectRef.current(null))
      graph.on('aftertransform', () => onZoomChangeRef.current?.(graph.getZoom()))

      void graph.render().then(async () => {
        if (disposed || graphRef.current !== graph) return
        if (nodesRef.current.length > 0) await graph.fitView({ when: 'always', direction: 'both' }, false)
        onZoomChangeRef.current?.(graph.getZoom())
        setReady(true)
      })

      resizeObserver = new ResizeObserver(() => {
        if (disposed || graphRef.current !== graph || nodesRef.current.length === 0) return
        void graph.fitView({ when: 'overflow', direction: 'both' }, { duration: 160 }).then(() => onZoomChangeRef.current?.(graph.getZoom()))
      })
      resizeObserver.observe(container)
    })

    return () => {
      disposed = true
      resizeObserver?.disconnect()
      const graph = graphRef.current
      graphRef.current = null
      setReady(false)
      graph?.destroy()
    }
  }, [])

  useEffect(() => {
    const graph = graphRef.current
    if (!ready || !graph) return
    nodesRef.current = nodes
    void graph.setData(graphData(nodes, edges))
    void graph.render().then(async () => {
      if (graphRef.current !== graph) return
      if (nodes.length > 0) await graph.fitView({ when: 'always', direction: 'both' }, { duration: 220 })
      onZoomChangeRef.current?.(graph.getZoom())
      await applyGraphFocus(graph, nodes, edges, selectedRef.current)
    })
  }, [edges, nodes, ready])

  useEffect(() => {
    selectedRef.current = selectedId
    const graph = graphRef.current
    if (!ready || !graph) return
    void applyGraphFocus(graph, nodes, edges, selectedId)
  }, [edges, nodes, ready, selectedId])

  return <div ref={containerRef} className="fox-g6-canvas" aria-label="知识图谱关系画布" />
})

function graphData(nodes: KnowledgeGraphCanvasNode[], edges: KnowledgeGraphCanvasEdge[]) {
  const ids = new Set(nodes.map((node) => node.id))
  return {
    nodes: nodes.map((node) => ({ id: node.id, data: { ...node } })),
    edges: edges.filter((edge) => ids.has(edge.source) && ids.has(edge.target)).map((edge) => ({ id: edge.id, source: edge.source, target: edge.target, data: { ...edge } })),
  }
}

async function applyGraphFocus(graph: G6Graph, nodes: KnowledgeGraphCanvasNode[], edges: KnowledgeGraphCanvasEdge[], selectedId: string | null) {
  const state: Record<string, string[]> = {}
  if (!selectedId) {
    nodes.forEach((node) => { state[node.id] = [] })
    edges.forEach((edge) => { state[edge.id] = [] })
    await graph.setElementState(state, false)
    return
  }

  const neighbors = new Set<string>()
  const activeEdges = new Set<string>()
  edges.forEach((edge) => {
    if (edge.source === selectedId) {
      neighbors.add(edge.target)
      activeEdges.add(edge.id)
    } else if (edge.target === selectedId) {
      neighbors.add(edge.source)
      activeEdges.add(edge.id)
    }
  })
  nodes.forEach((node) => {
    state[node.id] = node.id === selectedId ? ['selected'] : neighbors.has(node.id) ? ['neighbor'] : ['inactive']
  })
  edges.forEach((edge) => { state[edge.id] = activeEdges.has(edge.id) ? ['active'] : ['inactive'] })
  await graph.setElementState(state, false)
  await graph.frontElement([selectedId, ...neighbors])
}

function graphNodeWidth(label: string) {
  const wideCharacters = Array.from(label).reduce((total, character) => total + (/[^\u0000-\u00ff]/.test(character) ? 14 : 7.4), 0)
  return Math.round(Math.min(210, Math.max(126, wideCharacters + 34)))
}

function graphColors() {
  const card = canvasColor('--fox-card', '#ffffff')
  const border = canvasColor('--fox-border', '#dbe3ea')
  const accent = canvasColor('--fox-accent', '#69aee8')
  return {
    card,
    border,
    accent,
    nodeBorder: mixCanvasColor(border, accent, 0.72),
    text: canvasColor('--fox-text', '#17212b'),
    muted: canvasColor('--fox-faint', '#708090'),
    neighbor: mixCanvasColor(accent, border, 0.42),
    shadow: 'rgba(23, 33, 43, 0.05)',
    accentShadow: withAlpha(accent, 0.22),
  }
}

function canvasColor(variable: string, fallback: string) {
  return resolveCanvasColor(`var(${variable}, ${fallback})`, fallback)
}

function mixCanvasColor(foreground: string, background: string, ratio: number) {
  return resolveCanvasColor(`color-mix(in srgb, ${foreground} ${ratio * 100}%, ${background})`, foreground)
}

function withAlpha(color: string, alpha: number) {
  return resolveCanvasColor(`color-mix(in srgb, ${color} ${alpha * 100}%, transparent)`, color)
}

function resolveCanvasColor(color: string, fallback: string) {
  if (typeof document === 'undefined') return fallback
  const probe = document.createElement('span')
  probe.style.color = fallback
  probe.style.color = color
  probe.style.display = 'none'
  document.body.appendChild(probe)
  const resolved = getComputedStyle(probe).color || fallback
  probe.remove()
  return normalizeCanvasColor(resolved, fallback)
}

function normalizeCanvasColor(color: string, fallback: string) {
  const canvas = document.createElement('canvas')
  canvas.width = 1
  canvas.height = 1
  const context = canvas.getContext('2d')
  if (!context) return color || fallback
  context.clearRect(0, 0, 1, 1)
  context.fillStyle = color || fallback
  context.fillRect(0, 0, 1, 1)
  const [red, green, blue, alpha] = context.getImageData(0, 0, 1, 1).data
  return alpha === 255
    ? `rgb(${red}, ${green}, ${blue})`
    : `rgba(${red}, ${green}, ${blue}, ${(alpha / 255).toFixed(3)})`
}
