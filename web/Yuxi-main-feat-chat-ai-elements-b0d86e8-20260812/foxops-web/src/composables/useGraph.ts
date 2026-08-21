import { nextTick, reactive, ref, type Ref } from 'vue'

export function useGraph(graphRef?: Ref<any>) {
  const fetching = ref(false)
  const showDetailDrawer = ref(false)
  const selectedItem = ref<any>(null)
  const selectedItemType = ref<'node' | 'edge' | null>(null)

  const graphData = reactive<{ nodes: any[]; edges: any[] }>({
    nodes: [],
    edges: []
  })

  const handleNodeClick = (nodeData: any) => {
    selectedItem.value = nodeData
    selectedItemType.value = 'node'
    showDetailDrawer.value = true
  }

  const handleEdgeClick = (edgeData: any) => {
    selectedItem.value = edgeData
    selectedItemType.value = 'edge'
    showDetailDrawer.value = true
  }

  const handleCanvasClick = () => {
    showDetailDrawer.value = false
    selectedItem.value = null
    selectedItemType.value = null
    graphRef?.value?.clearFocus?.()
  }

  const clearGraph = () => {
    graphData.nodes = []
    graphData.edges = []
    handleCanvasClick()
  }

  const updateGraphData = (nodes: any[], edges: any[]) => {
    graphData.nodes = nodes || []
    graphData.edges = edges || []
    nextTick(() => {
      graphRef?.value?.refreshGraph?.()
    })
  }

  return {
    fetching,
    graphData,
    showDetailDrawer,
    selectedItem,
    selectedItemType,
    handleNodeClick,
    handleEdgeClick,
    handleCanvasClick,
    clearGraph,
    updateGraphData
  }
}
