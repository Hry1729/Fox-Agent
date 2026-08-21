<template>
  <div ref="rootEl" class="graph-canvas-container">
    <div v-show="graphData.nodes.length > 0" ref="container" class="graph-canvas"></div>
    <div class="slots">
      <div v-if="$slots.top" class="overlay top">
        <slot name="top" />
      </div>
      <div class="canvas-content">
        <slot name="content" />
      </div>
      <div v-if="graphData.nodes.length > 0" class="graph-stats-wrapper">
        <div v-if="activeStatsPanel" class="floating-panel type-stats-card">
          <div class="panel-header">
            <span class="panel-title">
              {{ activeStatsPanel === 'node' ? '实体类型' : '关系类型' }}
            </span>
          </div>
          <div class="panel-body">
            <div class="type-stats-list">
              <div
                v-for="item in activeTypeStats"
                :key="item.name"
                class="type-stats-row"
                :title="`${item.name}: ${item.count}`"
              >
                <span class="type-color" :style="{ backgroundColor: item.color }"></span>
                <span class="type-name">{{ item.name }}</span>
                <span class="type-count">{{ item.count }}</span>
              </div>
            </div>
          </div>
        </div>
        <div class="floating-panel graph-stats-panel">
          <button
            class="stat-item"
            :class="{ active: activeStatsPanel === 'node' }"
            type="button"
            @click="toggleStatsPanel('node')"
          >
            <span class="stat-label">实体</span>
            <span class="stat-value">{{ visibleEntityCount }}</span>
          </button>
          <button
            class="stat-item"
            :class="{ active: activeStatsPanel === 'edge' }"
            type="button"
            @click="toggleStatsPanel('edge')"
          >
            <span class="stat-label">关系</span>
            <span class="stat-value">{{ visibleRelationshipCount }}</span>
          </button>
        </div>
      </div>
      <div v-if="$slots.bottom" class="overlay bottom">
        <slot name="bottom" />
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
  import { Graph } from '@antv/g6'
  import { storeToRefs } from 'pinia'
  import { useSettingStore } from '@/store/modules/setting'

  const props = withDefaults(
    defineProps<{
      graphData: { nodes: any[]; edges: any[] }
      labelField?: string
      autoFit?: boolean
      autoResize?: boolean
      layoutOptions?: Record<string, any>
      nodeStyleOptions?: Record<string, any>
      edgeStyleOptions?: Record<string, any>
      enableFocusNeighbor?: boolean
      sizeByDegree?: boolean
      highlightKeywords?: string[]
    }>(),
    {
      labelField: 'name',
      autoFit: true,
      autoResize: true,
      layoutOptions: () => ({}),
      nodeStyleOptions: () => ({}),
      edgeStyleOptions: () => ({}),
      enableFocusNeighbor: true,
      sizeByDegree: true,
      highlightKeywords: () => []
    }
  )

  const emit = defineEmits<{
    ready: [instance: any]
    'data-rendered': []
    'node-click': [nodeData: any]
    'edge-click': [edgeData: any]
    'canvas-click': []
  }>()

  const container = ref<HTMLElement | null>(null)
  const rootEl = ref<HTMLElement | null>(null)
  const { isDark } = storeToRefs(useSettingStore())
  const activeStatsPanel = ref<'node' | 'edge' | ''>('')

  let graphInstance: any = null
  let resizeObserver: ResizeObserver | null = null
  let renderTimeout: ReturnType<typeof setTimeout> | null = null
  let retryCount = 0
  let resizeTimer: ReturnType<typeof setTimeout> | null = null
  let layoutTimeout: ReturnType<typeof setTimeout> | null = null
  let highlightTimeout: ReturnType<typeof setTimeout> | null = null
  let isMounted = false
  const MAX_RETRIES = 5

  const defaultLayout = {
    type: 'd3-force',
    preventOverlap: true,
    alphaDecay: 0.1,
    alphaMin: 0.01,
    velocityDecay: 0.6,
    iterations: 150,
    force: {
      center: { x: 0.5, y: 0.5, strength: 0.1 },
      charge: { strength: -400, distanceMax: 600 },
      link: { distance: 100, strength: 0.8 }
    },
    collide: { radius: 40, strength: 0.8, iterations: 3 }
  }

  const CHUNK_NODE_LABEL = 'Chunk'
  const CHUNK_NODE_COLOR = '#8c8c8c'
  const CHUNK_MENTION_EDGE_LABEL = 'MENTIONS'
  const NODE_LABEL_COLORS = [
    '#3996ae',
    '#5ad8a6',
    '#f6bd16',
    '#f27c7c',
    '#9581cc',
    '#6dc8ec',
    '#ff9d4d',
    '#92d050',
    '#e885ba'
  ]
  const EDGE_LABEL_COLORS = [
    '#99add1',
    '#3996ae',
    '#13c2c2',
    '#faad14',
    '#f27c7c',
    '#9581cc',
    '#52c41a',
    '#ff9d4d'
  ]

  function getCSSVariable(variableName: string, element: Element = document.documentElement) {
    return getComputedStyle(element).getPropertyValue(variableName).trim()
  }

  function hashString(value: string) {
    let hash = 0
    for (let i = 0; i < value.length; i++) {
      hash = (hash << 5) - hash + value.charCodeAt(i)
      hash |= 0
    }
    return Math.abs(hash)
  }

  function getPaletteColor(label: string, colors: string[]) {
    return colors[hashString(label) % colors.length]
  }

  function getNodeVisualLabel(node: any) {
    return node?.type || node?.normalized?.type || node?.properties?.label || 'Entity'
  }

  function getNodeColor(node: any) {
    const label = getNodeVisualLabel(node)
    if (label === CHUNK_NODE_LABEL) return CHUNK_NODE_COLOR
    return getPaletteColor(label, NODE_LABEL_COLORS)
  }

  function getEdgeVisualLabel(edge: any) {
    return edge?.type || edge?.normalized?.type || 'RELATED_TO'
  }

  function getEdgeColor(edge: any) {
    return getPaletteColor(getEdgeVisualLabel(edge), EDGE_LABEL_COLORS)
  }

  function isEntityNode(node: any) {
    return getNodeVisualLabel(node) !== CHUNK_NODE_LABEL
  }

  function isEntityRelationEdge(edge: any) {
    return getEdgeVisualLabel(edge) !== CHUNK_MENTION_EDGE_LABEL
  }

  function buildTypeStats(
    items: any[],
    getName: (item: any) => string,
    getColor: (item: any) => string
  ) {
    const counts = new Map<string, number>()
    for (const item of items || []) {
      const name = getName(item)
      counts.set(name, (counts.get(name) || 0) + 1)
    }
    return Array.from(counts, ([name, count]) => ({
      name,
      count,
      color: getColor({ type: name, normalized: { type: name } })
    })).sort((a, b) => b.count - a.count || a.name.localeCompare(b.name))
  }

  const visibleEntityNodes = computed(() => (props.graphData?.nodes || []).filter(isEntityNode))
  const visibleRelationshipEdges = computed(() =>
    (props.graphData?.edges || []).filter(isEntityRelationEdge)
  )
  const visibleEntityCount = computed(() => visibleEntityNodes.value.length)
  const visibleRelationshipCount = computed(() => visibleRelationshipEdges.value.length)
  const nodeTypeStats = computed(() =>
    buildTypeStats(visibleEntityNodes.value, getNodeVisualLabel, getNodeColor)
  )
  const edgeTypeStats = computed(() =>
    buildTypeStats(visibleRelationshipEdges.value, getEdgeVisualLabel, getEdgeColor)
  )
  const activeTypeStats = computed(() =>
    activeStatsPanel.value === 'node' ? nodeTypeStats.value : edgeTypeStats.value
  )

  function toggleStatsPanel(type: 'node' | 'edge') {
    activeStatsPanel.value = activeStatsPanel.value === type ? '' : type
  }

  function edgeEndpoint(edge: any, key: 'source' | 'target') {
    const alt = key === 'source' ? 'source_id' : 'target_id'
    return String(edge[key] ?? edge[alt] ?? '')
  }

  function formatData() {
    const data = props.graphData || { nodes: [], edges: [] }
    const degrees = new Map<string, number>()

    for (const n of data.nodes) {
      degrees.set(String(n.id), 0)
    }
    for (const e of data.edges) {
      const s = edgeEndpoint(e, 'source')
      const t = edgeEndpoint(e, 'target')
      degrees.set(s, (degrees.get(s) || 0) + 1)
      degrees.set(t, (degrees.get(t) || 0) + 1)
    }

    const nodes = (data.nodes || []).map((n) => ({
      id: String(n.id),
      data: {
        label: n[props.labelField] ?? n.name ?? String(n.id),
        visualLabel: getNodeVisualLabel(n),
        color: getNodeColor(n),
        degree: degrees.get(String(n.id)) || 0,
        original: n
      }
    }))

    const edges = (data.edges || []).map((e, idx) => ({
      id: e.id ? String(e.id) : `edge-${idx}`,
      source: edgeEndpoint(e, 'source'),
      target: edgeEndpoint(e, 'target'),
      data: {
        label: e.type ?? '',
        visualLabel: getEdgeVisualLabel(e),
        color: getEdgeColor(e),
        original: e
      }
    }))

    return { nodes, edges }
  }

  function initGraph() {
    if (!container.value) return

    const width = container.value.offsetWidth
    const height = container.value.offsetHeight

    if (width === 0 && height === 0) {
      if (retryCount < MAX_RETRIES) {
        retryCount++
        if (renderTimeout) clearTimeout(renderTimeout)
        renderTimeout = setTimeout(initGraph, 200)
      }
      return
    }

    retryCount = 0
    container.value.innerHTML = ''

    if (graphInstance) {
      try {
        graphInstance.destroy()
      } catch {
        // ignore
      }
      graphInstance = null
    }

    graphInstance = new Graph({
      container: container.value,
      width,
      height,
      autoFit: props.autoFit ? 'view' : undefined,
      autoResize: props.autoResize,
      layout: { ...defaultLayout, ...props.layoutOptions },
      node: {
        type: 'circle',
        style: {
          labelText: (d: any) => d.data.label,
          labelFill: getCSSVariable('--el-text-color-regular'),
          labelWordWrap: true,
          labelMaxWidth: '300%',
          size: (d: any) => {
            if (!props.sizeByDegree) return 24
            const deg = d.data.degree || 0
            return Math.min(15 + deg * 5, 50)
          },
          fill: (d: any) => d.data.color,
          opacity: 0.9,
          stroke: getCSSVariable('--el-bg-color'),
          lineWidth: 1.5,
          shadowColor: getCSSVariable('--el-border-color'),
          shadowBlur: 4,
          ...(props.nodeStyleOptions.style || {})
        },
        palette: props.nodeStyleOptions.palette
      },
      edge: {
        type: 'quadratic',
        style: {
          labelText: (d: any) => d.data.label,
          labelFill: getCSSVariable('--el-text-color-primary'),
          labelBackground: true,
          labelBackgroundFill: getCSSVariable('--el-fill-color-light'),
          stroke: (d: any) => d.data.color,
          opacity: 0.8,
          lineWidth: 1.2,
          endArrow: true,
          ...(props.edgeStyleOptions.style || {})
        },
        palette: props.edgeStyleOptions.palette
      },
      behaviors: [
        'drag-element',
        'zoom-canvas',
        'drag-canvas',
        'hover-activate',
        {
          type: 'click-select',
          degree: 1,
          state: 'selected',
          neighborState: 'active',
          unselectedState: 'inactive',
          multiple: true,
          trigger: ['shift']
        }
      ]
    })

    graphInstance.on('node:click', (evt: any) => {
      const nodeId = evt.target.id
      emit('node-click', graphInstance.getNodeData(nodeId))
    })

    graphInstance.on('edge:click', (evt: any) => {
      const edgeId = evt.target.id
      emit('edge-click', graphInstance.getEdgeData(edgeId))
    })

    graphInstance.on('canvas:click', (evt: any) => {
      if (!evt.target) emit('canvas-click')
    })

    emit('ready', graphInstance)
  }

  function applyHighlightKeywords() {
    if (!graphInstance || !props.highlightKeywords?.length) return

    const { nodes } = graphInstance.getData()
    const updates: Record<string, string[]> = {}

    nodes.forEach((node: any) => {
      const nodeLabel = node.data.label || String(node.id)
      const shouldHighlight = props.highlightKeywords!.some(
        (keyword) =>
          keyword.trim() !== '' && nodeLabel.toLowerCase().includes(keyword.toLowerCase())
      )
      if (shouldHighlight) updates[node.id] = ['highlighted']
    })

    if (Object.keys(updates).length > 0) {
      graphInstance.setElementState(updates)
      graphInstance.draw()
    }
  }

  function clearHighlights() {
    if (!graphInstance) return
    const { nodes } = graphInstance.getData()
    const updates: Record<string, string[]> = {}
    nodes.forEach((node: any) => {
      updates[node.id] = []
    })
    if (Object.keys(updates).length > 0) {
      graphInstance.setElementState(updates)
      graphInstance.draw()
    }
  }

  function setGraphData() {
    if (!graphInstance || !isMounted) initGraph()
    if (!graphInstance || !isMounted) return
    const data = formatData()

    graphInstance.setData(data)
    graphInstance.render()

    if (layoutTimeout) clearTimeout(layoutTimeout)
    layoutTimeout = setTimeout(() => {
      if (!isMounted || !graphInstance) return
      try {
        graphInstance.layout?.()
      } catch {
        // ignore
      }

      if (highlightTimeout) clearTimeout(highlightTimeout)
      highlightTimeout = setTimeout(() => {
        if (!isMounted || !graphInstance) return
        applyHighlightKeywords()
        emit('data-rendered')
      }, 1500)
    }, 10)
  }

  function renderGraph() {
    if (!graphInstance) initGraph()
    setGraphData()
  }

  function refreshGraph() {
    if (!isMounted) return
    if (graphInstance) {
      try {
        graphInstance.destroy()
      } catch {
        // ignore
      }
      graphInstance = null
    }
    if (container.value) container.value.innerHTML = ''
    retryCount = 0
    if (renderTimeout) clearTimeout(renderTimeout)
    renderTimeout = setTimeout(() => {
      if (isMounted) renderGraph()
    }, 300)
  }

  function fitView() {
    try {
      graphInstance?.fitView()
    } catch {
      // ignore
    }
  }

  function fitCenter() {
    try {
      graphInstance?.fitCenter()
    } catch {
      // ignore
    }
  }

  function getInstance() {
    return graphInstance
  }

  async function focusNode(id: string) {
    if (!graphInstance || !props.enableFocusNeighbor) return
    const { nodes, edges } = graphInstance.getData()
    const updates: Record<string, string[]> = {}
    nodes.forEach((n: any) => {
      updates[n.id] = ['hidden']
    })
    edges.forEach((e: any) => {
      updates[e.id] = ['hidden']
    })
    const neighborSet = new Set<string>()
    const related: string[] = []
    edges.forEach((e: any) => {
      if (e.source === id) {
        neighborSet.add(e.target)
        related.push(e.id)
      } else if (e.target === id) {
        neighborSet.add(e.source)
        related.push(e.id)
      }
    })
    updates[id] = ['focus']
    neighborSet.forEach((nid) => {
      updates[nid] = ['focus']
    })
    related.forEach((eid) => {
      updates[eid] = ['focus']
    })
    await graphInstance.setElementState(updates)
    await graphInstance.draw()
  }

  async function clearFocus() {
    if (!graphInstance) return
    const { nodes, edges } = graphInstance.getData()
    const updates: Record<string, string[]> = {}
    nodes.forEach((n: any) => {
      updates[n.id] = []
    })
    edges.forEach((e: any) => {
      updates[e.id] = []
    })
    await graphInstance.setElementState(updates)
    await graphInstance.draw()
  }

  watch(
    () => props.graphData,
    () => {
      if (!isMounted) return
      if (renderTimeout) clearTimeout(renderTimeout)
      renderTimeout = setTimeout(() => setGraphData(), 50)
    },
    { deep: true }
  )

  watch(
    () => props.highlightKeywords,
    () => {
      if (graphInstance && isMounted) {
        clearHighlights()
        setTimeout(() => {
          if (isMounted) applyHighlightKeywords()
        }, 50)
      }
    },
    { deep: true }
  )

  watch(isDark, () => {
    if (graphInstance && isMounted) refreshGraph()
  })

  onMounted(() => {
    isMounted = true
    if (window.ResizeObserver) {
      resizeObserver = new ResizeObserver(() => {
        if (!container.value || !graphInstance || !isMounted) return
        const width = container.value.offsetWidth
        const height = container.value.offsetHeight
        if (width === 0 && height === 0) return
        graphInstance.setSize(width, height)
        if (resizeTimer) clearTimeout(resizeTimer)
        resizeTimer = setTimeout(() => {
          if (graphInstance && isMounted) graphInstance.fitView()
        }, 150)
      })
      if (container.value) resizeObserver.observe(container.value)
    }

    if (renderTimeout) clearTimeout(renderTimeout)
    renderTimeout = setTimeout(() => {
      if (isMounted) renderGraph()
    }, 300)

    window.addEventListener('resize', refreshGraph)
  })

  onUnmounted(() => {
    isMounted = false
    window.removeEventListener('resize', refreshGraph)
    if (resizeObserver && container.value) resizeObserver.unobserve(container.value)
    if (renderTimeout) clearTimeout(renderTimeout)
    if (resizeTimer) clearTimeout(resizeTimer)
    if (layoutTimeout) clearTimeout(layoutTimeout)
    if (highlightTimeout) clearTimeout(highlightTimeout)
    try {
      graphInstance?.destroy()
    } catch {
      // ignore
    }
    graphInstance = null
  })

  defineExpose({
    refreshGraph,
    fitView,
    fitCenter,
    getInstance,
    focusNode,
    clearFocus,
    setData: setGraphData,
    applyHighlightKeywords,
    clearHighlights
  })
</script>

<style scoped lang="scss">
  .graph-canvas-container {
    position: relative;
    width: 100%;
    height: 100%;
  }

  .graph-canvas {
    width: 100%;
    height: 100%;
  }

  .graph-stats-wrapper {
    position: absolute;
    bottom: 10px;
    left: 10px;
    z-index: 10;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 8px;
    pointer-events: auto;
  }

  .floating-panel {
    border-radius: 8px;
    border: 1px solid var(--el-border-color-lighter);
    background: color-mix(in srgb, var(--el-bg-color) 92%, transparent);
    backdrop-filter: blur(12px);
    box-shadow: 0 2px 8px rgba(0, 0, 0, 0.06);
    font-size: 13px;
    user-select: auto;
  }

  .panel-header {
    display: flex;
    align-items: center;
    padding: 10px 14px;
    border-bottom: 1px solid var(--el-border-color-extra-light);
  }

  .panel-title {
    font-size: 13px;
    font-weight: 600;
    color: var(--el-text-color-primary);
    line-height: 1.2;
  }

  .panel-body {
    padding: 10px 14px;
  }

  .graph-stats-panel {
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 6px 12px;
  }

  .stat-item {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px 0;
    border: 0;
    background: transparent;
    cursor: pointer;
    font: inherit;
    line-height: 1.4;

    &:hover,
    &.active {
      .stat-label,
      .stat-value {
        color: var(--el-color-primary);
      }
    }
  }

  .stat-label {
    color: var(--el-text-color-secondary);
    font-weight: 500;
  }

  .stat-value {
    color: var(--el-text-color-primary);
    font-weight: 600;
  }

  .type-stats-card {
    width: 220px;
    max-width: calc(100vw - 40px);
    overflow: hidden;
  }

  .type-stats-list {
    display: flex;
    flex-direction: column;
    gap: 2px;
    max-height: 220px;
    overflow-y: auto;
    padding-right: 2px;
  }

  .type-stats-row {
    display: grid;
    grid-template-columns: 12px minmax(0, 1fr) auto;
    align-items: center;
    gap: 8px;
    min-height: 28px;
    padding: 4px 6px;
    border-radius: 6px;

    &:hover {
      background: var(--el-fill-color-lighter);
    }
  }

  .type-color {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    box-shadow:
      0 0 0 1px var(--el-bg-color),
      0 0 0 2px var(--el-border-color-lighter);
  }

  .type-name {
    min-width: 0;
    overflow: hidden;
    color: var(--el-text-color-regular);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .type-count {
    color: var(--el-text-color-primary);
    font-size: 12px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .slots {
    pointer-events: none;
    position: absolute;
    top: 0;
    left: 0;
    z-index: 20;
    display: flex;
    flex-direction: column;
    width: 100%;
    height: 100%;
  }

  .overlay {
    width: 100%;
    flex-shrink: 0;
    pointer-events: auto;
  }

  .canvas-content {
    flex: 1;
    pointer-events: none;
    background: transparent !important;
  }

  .canvas-content :deep(*) {
    pointer-events: none;
  }
</style>
