<template>
  <div class="knowledge-graph-panel">
    <div class="graph-stage" v-loading="statusLoading && !buildStatus">
      <!-- 顶栏：左搜索 / 中视图切换 / 右索引+设置 -->
      <div class="compact-actions">
        <div class="actions-left">
          <ElInput
            v-model="searchInput"
            clearable
            placeholder="搜索实体"
            class="entity-search"
            @keydown.enter.exact.prevent="onSearch"
          >
            <template #suffix>
              <ArtSvgIcon
                :icon="graphFetching ? 'ri:loader-4-line' : 'ri:search-line'"
                :class="{ spin: graphFetching }"
                class="search-suffix"
                @click="onSearch"
              />
            </template>
          </ElInput>
          <button type="button" class="action-btn" title="刷新" @click="loadGraph">
            <ArtSvgIcon
              icon="ri:refresh-line"
              :class="{ spin: graphFetching }"
            />
          </button>
        </div>

        <div v-if="hasGraphNodes" class="actions-center">
          <ElRadioGroup v-model="viewMode" class="view-mode-group">
            <ElRadioButton value="graph" label="graph">图谱</ElRadioButton>
            <ElRadioButton value="list" label="list">列表</ElRadioButton>
          </ElRadioGroup>
        </div>

        <div class="actions-right">
          <button
            type="button"
            class="action-btn index-action-btn"
            :class="{ 'has-index-label': hasPendingGraphChunks }"
            :title="graphIndexButtonTitle"
            @click="toggleBuildPanel"
          >
            <ArtSvgIcon icon="ri:database-2-line" />
            <span v-if="hasPendingGraphChunks" class="index-status-label">
              {{ pendingGraphChunks }} 待索引
            </span>
            <span
              v-if="graphIndexDotStatus"
              class="status-dot"
              :class="`status-dot--${graphIndexDotStatus}`"
            />
          </button>
          <button type="button" class="action-btn" title="设置" @click="toggleSettingsPanel">
            <ArtSvgIcon icon="ri:settings-3-line" />
          </button>
        </div>
      </div>

      <!-- 中央内容 -->
      <div
        class="graph-main"
        :class="{
          'is-canvas': hasGraphNodes && viewMode === 'graph',
          'is-list': hasGraphNodes && viewMode === 'list'
        }"
      >
        <!-- 未配置抽取器 -->
        <div v-if="showGraphConfigEmpty" class="graph-empty-state">
          <ArtSvgIcon icon="ri:share-line" class="empty-icon" />
          <h3>暂无知识图谱</h3>
          <p>配置抽取器后，才能从当前知识库构建实体与关系。</p>
          <ElButton type="primary" @click="openGraphConfig">
            <ArtSvgIcon icon="ri:settings-3-line" class="btn-icon" />
            配置抽取器
          </ElButton>
        </div>

        <!-- 已配置但无数据 -->
        <div v-else-if="showGraphDataEmpty" class="graph-empty-state">
          <ArtSvgIcon icon="ri:share-line" class="empty-icon" />
          <h3>{{ graphDataEmptyTitle }}</h3>
          <p>{{ graphDataEmptyDescription }}</p>
          <div class="empty-actions">
            <ElButton v-if="searchInput.trim()" @click="clearGraphSearch">清空搜索</ElButton>
            <ElButton
              v-else-if="hasPendingGraphChunks && !isBuildActive"
              type="primary"
              @click="startGraphBuild"
            >
              <ArtSvgIcon icon="ri:database-2-line" class="btn-icon" />
              开始索引
            </ElButton>
            <ElButton v-else :loading="graphFetching" @click="loadGraph">刷新图谱</ElButton>
          </div>
        </div>

        <!-- 有图谱数据：图谱画布 / 节点列表 -->
        <div
          v-else-if="hasGraphNodes"
          class="graph-data-view"
          v-loading="graphFetching"
        >
          <div v-show="viewMode === 'graph'" class="graph-canvas-wrap">
            <GraphCanvas
              ref="graphCanvasRef"
              :graph-data="canvasGraphData"
              :highlight-keywords="graphHighlightKeywords"
              @node-click="handleNodeClick"
              @edge-click="handleEdgeClick"
              @canvas-click="handleCanvasClick"
            />
            <GraphDetailPanel
              :visible="showDetailDrawer"
              :item="selectedItem"
              :type="selectedItemType"
              @close="handleCanvasClick"
            />
          </div>
          <div v-show="viewMode === 'list'" class="node-list-view">
            <div class="graph-stats-bar">
              <span>节点 {{ nodes.length }}</span>
              <span>关系 {{ edges.length }}</span>
            </div>
            <div class="node-grid">
              <div v-for="node in displayNodes" :key="node.id || node.name" class="node-chip">
                <span class="node-name">{{ node.name || node.label || node.id }}</span>
                <span class="node-type">{{ node.type || node.category || 'Entity' }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>

      <!-- 图谱设置浮层 -->
      <transition name="slide-fade">
        <div v-if="showSettings" class="floating-panel settings-panel">
          <div class="panel-header">
            <span class="panel-title">图谱设置</span>
          </div>
          <div class="panel-body">
            <ElForm label-position="top" size="small">
              <ElFormItem label="最大节点数 (limit)">
                <ElInputNumber
                  v-model="subgraphParams.maxNodes"
                  :min="10"
                  :max="1000"
                  :step="10"
                  controls-position="right"
                  class="full"
                />
              </ElFormItem>
              <ElFormItem label="搜索深度 (depth)">
                <ElInputNumber
                  v-model="subgraphParams.maxDepth"
                  :min="1"
                  :max="5"
                  :step="1"
                  controls-position="right"
                  class="full"
                />
              </ElFormItem>
              <ElFormItem label="排除 Chunk 节点">
                <ElSwitch v-model="subgraphParams.excludeChunk" />
              </ElFormItem>
              <ElButton type="primary" class="full" @click="applySettings">应用</ElButton>
            </ElForm>
          </div>
        </div>
      </transition>

      <!-- 索引管理浮层 -->
      <transition name="slide-fade">
        <div v-if="showBuildPanel" class="floating-panel build-panel">
          <div class="panel-header">
            <span class="panel-title">索引管理</span>
            <button
              type="button"
              class="panel-refresh-btn"
              :disabled="statusLoading"
              @click="loadGraphBuildStatus"
            >
              <ArtSvgIcon icon="ri:refresh-line" :class="{ spin: statusLoading }" />
            </button>
          </div>
          <div class="panel-body">
            <div class="status-row">
              <span class="status-label">状态</span>
              <ElTag v-if="isBuildActive" type="primary" size="small" effect="light">构建中</ElTag>
              <ElTag v-else-if="isBuildFailed" type="danger" size="small" effect="light">
                构建失败
              </ElTag>
              <ElTag v-else-if="buildStatus?.locked" type="success" size="small" effect="light">
                已配置
              </ElTag>
              <ElTag v-else type="warning" size="small" effect="light">未配置</ElTag>
            </div>

            <ElProgress
              v-if="isBuildActive"
              :percentage="Number(buildStatus?.build_task_progress || 0)"
              :stroke-width="6"
              class="build-progress"
            />

            <div class="stats-grid">
              <div class="stat-item">
                <span class="stat-value">{{ buildStatus?.total_chunks ?? '-' }}</span>
                <span class="stat-label">总 Chunk</span>
              </div>
              <div class="stat-item">
                <span class="stat-value">{{ buildStatus?.pending_chunks ?? '-' }}</span>
                <span class="stat-label">待构建</span>
              </div>
              <div class="stat-item">
                <span class="stat-value">{{ buildStatus?.indexed_chunks ?? '-' }}</span>
                <span class="stat-label">已构建</span>
              </div>
              <div class="stat-item">
                <span class="stat-value">{{ buildStatus?.entity_count ?? '-' }}</span>
                <span class="stat-label">实体</span>
              </div>
              <div class="stat-item">
                <span class="stat-value">{{ buildStatus?.relationship_count ?? '-' }}</span>
                <span class="stat-label">关系</span>
              </div>
            </div>

            <div class="build-actions">
              <ElButton v-if="!buildStatus?.locked" type="primary" class="full" @click="openGraphConfig">
                配置抽取器
              </ElButton>
              <ElButton v-else-if="isBuildActive" type="primary" class="full" disabled>
                构建中 {{ buildStatus?.build_task_progress ?? 0 }}%
              </ElButton>
              <ElButton
                v-else-if="isBuildFailed"
                type="primary"
                class="full"
                :disabled="!buildStatus?.pending_chunks"
                @click="startGraphBuild"
              >
                <ArtSvgIcon icon="ri:restart-line" class="btn-icon" />
                重试索引
              </ElButton>
              <ElButton
                v-else
                type="primary"
                class="full"
                :disabled="!buildStatus?.pending_chunks"
                @click="startGraphBuild"
              >
                <ArtSvgIcon icon="ri:database-2-line" class="btn-icon" />
                开始索引
              </ElButton>

              <div v-if="buildStatus?.locked && !isBuildActive" class="actions-secondary">
                <ElButton text size="small" @click="openGraphConfig">修改配置</ElButton>
                <ElButton text type="danger" size="small" @click="confirmResetGraph">重置</ElButton>
              </div>
            </div>
          </div>
        </div>
      </transition>
    </div>

    <!-- 抽取器配置弹窗 -->
    <ElDialog
      v-model="showGraphConfig"
      width="640px"
      destroy-on-close
      :show-close="false"
      class="extension-dialog"
      header-class="extension-dialog-header"
      body-class="extension-dialog-body"
      footer-class="extension-dialog-footer"
    >
      <template #header>
        <ExtensionDialogHeader :title="graphConfigTitle" @close="showGraphConfig = false" />
      </template>
      <ElAlert
        v-if="isEditingGraphConfig"
        type="warning"
        :closable="false"
        show-icon
        class="config-warning"
        title="修改配置仅影响后续构建；已构建的图谱不会自动重算，如需一致请重置后重新抽取。抽取器类型创建后不可修改。"
      />
      <ElForm label-position="top" class="graph-config-form">
        <ElFormItem label="抽取器类型">
          <div class="extractor-type-cards">
            <button
              v-for="option in extractorTypeOptions"
              :key="option.value"
              type="button"
              class="extractor-type-card"
              :class="{
                active: graphConfigForm.extractor_type === option.value,
                disabled: isEditingGraphConfig || option.disabled
              }"
              :disabled="isEditingGraphConfig || option.disabled"
              @click="selectExtractorType(option)"
            >
              <div class="card-header">
                <ArtSvgIcon :icon="option.icon" class="type-icon" />
                <span class="type-title">{{ option.label }}</span>
              </div>
              <div class="card-description">{{ option.description }}</div>
              <div v-if="option.helper" class="card-helper" :class="{ warning: option.disabled }">
                {{ option.helper }}
              </div>
            </button>
          </div>
        </ElFormItem>
        <ElFormItem label="模型">
          <ElSelect
            v-model="graphConfigForm.model_spec"
            filterable
            clearable
            placeholder="请选择抽取模型"
            style="width: 100%"
            :loading="chatModelsLoading"
          >
            <ElOptionGroup
              v-for="(group, providerId) in chatModels"
              :key="providerId"
              :label="group.provider_display_name || String(providerId)"
            >
              <ElOption
                v-for="model in group.models"
                :key="model.spec"
                :label="model.display_name || model.spec"
                :value="model.spec"
              />
            </ElOptionGroup>
          </ElSelect>
        </ElFormItem>
        <ElFormItem label="Schema">
          <ElInput
            v-model="graphConfigForm.schema"
            type="textarea"
            :rows="5"
            placeholder="描述实体类型、关系类型和属性约束。后端会把 Schema 拼接到固定抽取 Prompt 中。"
          />
        </ElFormItem>
        <div class="form-grid form-grid-params">
          <ElFormItem label="并发队列数" class="concurrency-item">
            <ElInputNumber
              v-model="graphConfigForm.concurrency_count"
              :min="1"
              :max="1000"
              controls-position="right"
              class="full"
            />
          </ElFormItem>
          <ElFormItem label="模型参数 JSON" class="params-item">
            <ElInput
              v-model="graphConfigForm.model_params_text"
              placeholder='例如 {"temperature":0.1}'
            />
          </ElFormItem>
        </div>
      </ElForm>
      <template #footer>
        <ElButton @click="showGraphConfig = false">取消</ElButton>
        <ElButton type="primary" :loading="configSaving" @click="configureGraphBuild">
          确定
        </ElButton>
      </template>
    </ElDialog>
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { graphApi } from '@/api/graph'
  import { modelProviderApi } from '@/api/model'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'
  import GraphCanvas from '@/components/extensions/knowledge/graph/GraphCanvas.vue'
  import GraphDetailPanel from '@/components/extensions/knowledge/graph/GraphDetailPanel.vue'
  import { useGraph } from '@/composables/useGraph'
  import { useTaskerStore } from '@/store/modules/tasker'
  import { unwrapApiData } from '@/utils/apiData'

  const props = defineProps<{ kbId: string }>()
  const tasker = useTaskerStore()

  const GRAPH_BUILD_TASK_TYPE = 'knowledge_graph_index'

  const searchInput = ref('')
  const viewMode = ref<'graph' | 'list'>('graph')
  const graphCanvasRef = ref<InstanceType<typeof GraphCanvas> | null>(null)
  const {
    showDetailDrawer,
    selectedItem,
    selectedItemType,
    handleNodeClick,
    handleEdgeClick,
    handleCanvasClick
  } = useGraph(graphCanvasRef)

  const showSettings = ref(false)
  const showBuildPanel = ref(false)
  const showGraphConfig = ref(false)
  const statusLoading = ref(false)
  const graphFetching = ref(false)
  const graphLoaded = ref(false)
  const configSaving = ref(false)
  const chatModelsLoading = ref(false)
  const chatModels = ref<
    Record<string, { provider_display_name?: string; models: Array<{ spec: string; display_name?: string }> }>
  >({})
  const buildStatus = ref<Record<string, any> | null>(null)
  const nodes = ref<any[]>([])
  const edges = ref<any[]>([])

  const subgraphParams = reactive({
    maxNodes: 100,
    maxDepth: 2,
    excludeChunk: true
  })

  const graphConfigForm = reactive({
    extractor_type: 'llm',
    model_spec: '',
    schema: '',
    concurrency_count: 50,
    model_params_text: ''
  })

  const extractorTypeOptions = [
    {
      value: 'llm',
      label: 'LLM',
      description: '使用大模型按 Schema 抽取实体和关系',
      helper: '当前唯一支持的图谱抽取方式',
      icon: 'ri:brain-line',
      disabled: false
    },
    {
      value: 'more',
      label: '更多',
      description: '更多抽取方式正在拓展中',
      helper: '拓展中',
      icon: 'ri:more-line',
      disabled: true
    }
  ]

  let buildStatusPollTimer: ReturnType<typeof setInterval> | null = null

  const isBuildActive = computed(() => {
    const s = buildStatus.value?.build_task_status
    return s === 'pending' || s === 'running'
  })
  const isBuildFailed = computed(() => buildStatus.value?.build_task_status === 'failed')
  const pendingGraphChunks = computed(() => Number(buildStatus.value?.pending_chunks ?? 0))
  const hasPendingGraphChunks = computed(() => pendingGraphChunks.value > 0)
  const isGraphIndexComplete = computed(
    () =>
      Boolean(buildStatus.value?.locked) && !isBuildActive.value && pendingGraphChunks.value === 0
  )
  const graphIndexDotStatus = computed(() => {
    if (isBuildActive.value) return 'active'
    if (hasPendingGraphChunks.value) return 'pending'
    if (isGraphIndexComplete.value) return 'complete'
    return ''
  })
  const graphIndexButtonTitle = computed(() => {
    if (hasPendingGraphChunks.value) return `索引管理，${pendingGraphChunks.value} 待索引`
    if (isGraphIndexComplete.value) return '索引管理，已全部索引'
    if (isBuildActive.value) return '索引管理，索引中'
    return '索引管理'
  })

  const isEditingGraphConfig = computed(() => Boolean(buildStatus.value?.locked))
  const graphConfigTitle = computed(() =>
    isEditingGraphConfig.value ? '修改图谱抽取配置' : '配置图谱抽取器'
  )

  const hasGraphNodes = computed(() => nodes.value.length > 0)
  const showGraphConfigEmpty = computed(
    () => !buildStatus.value?.locked && !statusLoading.value
  )
  const showGraphDataEmpty = computed(
    () =>
      Boolean(buildStatus.value?.locked) &&
      graphLoaded.value &&
      !graphFetching.value &&
      !hasGraphNodes.value
  )
  const graphDataEmptyTitle = computed(() =>
    searchInput.value.trim() ? '未找到匹配实体' : '暂无知识图谱'
  )
  const graphDataEmptyDescription = computed(() => {
    if (searchInput.value.trim()) return '换个关键词或调整图谱设置后再搜索。'
    if (isBuildActive.value) return '图谱索引正在运行，完成后会展示实体与关系。'
    if (hasPendingGraphChunks.value) return '当前还有待索引 Chunk，完成索引后会展示实体与关系。'
    return '当前知识库还没有可展示的实体与关系。'
  })

  const displayNodes = computed(() => nodes.value.slice(0, 200))
  const canvasGraphData = computed(() => ({
    nodes: nodes.value,
    edges: edges.value
  }))
  const graphHighlightKeywords = computed(() => {
    const keyword = searchInput.value.trim()
    return keyword ? [keyword] : []
  })

  const toggleBuildPanel = () => {
    showBuildPanel.value = !showBuildPanel.value
    showSettings.value = false
  }

  const toggleSettingsPanel = () => {
    showSettings.value = !showSettings.value
    showBuildPanel.value = false
  }

  const stopBuildStatusPoll = () => {
    if (buildStatusPollTimer) {
      clearInterval(buildStatusPollTimer)
      buildStatusPollTimer = null
    }
  }

  const startBuildStatusPoll = () => {
    stopBuildStatusPoll()
    buildStatusPollTimer = setInterval(() => {
      loadGraphBuildStatus()
    }, 5000)
  }

  const loadGraphBuildStatus = async () => {
    if (!props.kbId) return
    statusLoading.value = true
    try {
      const result: any = await graphApi.getGraphBuildStatus(props.kbId)
      buildStatus.value = unwrapApiData(result, result) || result
    } catch (error) {
      ElMessage.error((error as Error)?.message || '加载图谱构建状态失败')
    } finally {
      statusLoading.value = false
    }
  }

  const normalizeNodes = (list: any[]) =>
    (list || []).map((item, index) => ({
      id: item.id || item.entity_id || item.name || `node-${index}`,
      name: item.name || item.label || item.properties?.name || item.id,
      type: item.type || item.category || item.properties?.type || 'Entity',
      ...item
    }))

  const loadGraph = async () => {
    if (!props.kbId) return
    // 未配置抽取器时不强制拉子图，避免空库 404 干扰
    if (!buildStatus.value?.locked) {
      nodes.value = []
      edges.value = []
      graphLoaded.value = true
      return
    }
    graphFetching.value = true
    try {
      const result: any = await graphApi.getSubgraph(props.kbId, {
        nodeLabel: searchInput.value.trim() || '*',
        maxDepth: subgraphParams.maxDepth,
        maxNodes: subgraphParams.maxNodes,
        excludeChunk: subgraphParams.excludeChunk
      })
      const data = unwrapApiData(result, result) || {}
      nodes.value = normalizeNodes(data.nodes || data.entities || [])
      edges.value = data.edges || data.relations || []
    } catch (error) {
      nodes.value = []
      edges.value = []
      ElMessage.error((error as Error)?.message || '加载图谱失败')
    } finally {
      graphFetching.value = false
      graphLoaded.value = true
    }
  }

  const onSearch = () => loadGraph()

  const clearGraphSearch = () => {
    searchInput.value = ''
    loadGraph()
  }

  const applySettings = () => {
    showSettings.value = false
    loadGraph()
  }

  const fillGraphConfigForm = () => {
    const config = buildStatus.value?.config || {}
    const options = config.extractor_options || {}
    graphConfigForm.extractor_type = 'llm'
    graphConfigForm.model_spec = options.model_spec || ''
    graphConfigForm.schema = options.schema || ''
    graphConfigForm.concurrency_count = Number(options.concurrency_count || 50)
    graphConfigForm.model_params_text = options.model_params
      ? JSON.stringify(options.model_params)
      : ''
  }

  const loadChatModels = async () => {
    chatModelsLoading.value = true
    try {
      const result: any = await modelProviderApi.getV2Models('chat')
      const data = result?.data || unwrapApiData(result, result) || {}
      const mapped: Record<
        string,
        { provider_display_name?: string; models: Array<{ spec: string; display_name?: string }> }
      > = {}
      Object.entries(data).forEach(([providerId, value]: [string, any]) => {
        if (Array.isArray(value)) {
          mapped[providerId] = { provider_display_name: providerId, models: value }
        } else {
          mapped[providerId] = {
            provider_display_name: value?.provider_display_name || providerId,
            models: value?.models || []
          }
        }
      })
      chatModels.value = mapped
      // 未选模型时默认选中第一个可用 chat 模型
      if (!graphConfigForm.model_spec) {
        for (const group of Object.values(mapped)) {
          if (group.models?.[0]?.spec) {
            graphConfigForm.model_spec = group.models[0].spec
            break
          }
        }
      }
    } catch (error) {
      ElMessage.error((error as Error)?.message || '加载模型列表失败')
      chatModels.value = {}
    } finally {
      chatModelsLoading.value = false
    }
  }

  const openGraphConfig = async () => {
    fillGraphConfigForm()
    showGraphConfig.value = true
    showBuildPanel.value = false
    await loadChatModels()
  }

  const selectExtractorType = (option: { value: string; disabled?: boolean }) => {
    if (isEditingGraphConfig.value || option.disabled) return
    graphConfigForm.extractor_type = option.value
  }

  const parseModelParams = () => {
    const text = graphConfigForm.model_params_text.trim()
    if (!text) return {}
    const params = JSON.parse(text)
    if (!params || Array.isArray(params) || typeof params !== 'object') {
      throw new Error('模型参数必须是 JSON 对象')
    }
    return params
  }

  const configureGraphBuild = async () => {
    if (!graphConfigForm.model_spec) {
      ElMessage.warning('请选择抽取模型')
      return
    }
    configSaving.value = true
    try {
      let modelParams = {}
      try {
        modelParams = parseModelParams()
      } catch (error) {
        ElMessage.error((error as Error).message)
        return
      }
      await graphApi.configureGraphBuild(props.kbId, {
        extractor_type: 'llm',
        extractor_options: {
          model_spec: graphConfigForm.model_spec,
          schema: graphConfigForm.schema.trim(),
          concurrency_count: graphConfigForm.concurrency_count || 50,
          model_params: modelParams
        }
      })
      ElMessage.success(isEditingGraphConfig.value ? '图谱抽取配置已更新' : '图谱抽取配置已保存')
      showGraphConfig.value = false
      await loadGraphBuildStatus()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '配置图谱抽取失败')
    } finally {
      configSaving.value = false
    }
  }

  const startGraphBuild = async () => {
    try {
      const result: any = await graphApi.startGraphIndex(props.kbId, 20)
      const taskId = result?.task_id || result?.id
      if (taskId) await tasker.trackTask(String(taskId))
      ElMessage.success(result?.message || '图谱构建任务已提交')
      showBuildPanel.value = true
      await loadGraphBuildStatus()
    } catch (error) {
      ElMessage.error((error as Error)?.message || '提交图谱构建任务失败')
    }
  }

  const confirmResetGraph = async () => {
    try {
      await ElMessageBox.confirm(
        '将删除该知识库在 Neo4j 中的图谱，重置 Chunk 图谱状态，并清空抽取结果与配置。',
        '清空并重建图谱',
        { type: 'warning', confirmButtonText: '确认重置' }
      )
      await graphApi.resetGraphBuild(props.kbId, {
        clear_extraction_result: true,
        clear_config: true
      })
      ElMessage.success('图谱构建状态已重置')
      nodes.value = []
      edges.value = []
      graphLoaded.value = false
      await loadGraphBuildStatus()
    } catch {
      // 取消
    }
  }

  const refreshAll = async () => {
    showSettings.value = false
    showBuildPanel.value = false
    searchInput.value = ''
    nodes.value = []
    edges.value = []
    graphLoaded.value = false
    buildStatus.value = null
    await loadGraphBuildStatus()
    await loadGraph()
  }

  watch(isBuildActive, (active) => {
    if (active) startBuildStatusPoll()
    else {
      stopBuildStatusPoll()
      // 构建结束后刷新图谱
      if (buildStatus.value?.locked) loadGraph()
    }
  })

  watch(
    () =>
      Object.values(tasker.tasks)
        .filter((t) => t.type === GRAPH_BUILD_TASK_TYPE || String(t.name || '').includes('图谱'))
        .map((t) => `${t.id}:${t.status}`)
        .join('|'),
    () => {
      const related = Object.values(tasker.tasks).some(
        (t) =>
          (t.type === GRAPH_BUILD_TASK_TYPE || String(t.name || '').includes('图谱')) &&
          tasker.isTerminal(t.status)
      )
      if (related) {
        loadGraphBuildStatus().then(() => loadGraph())
      }
    }
  )

  watch(() => props.kbId, refreshAll, { immediate: true })

  onBeforeUnmount(() => {
    stopBuildStatusPoll()
  })
</script>

<style scoped>
  .knowledge-graph-panel {
    height: 100%;
    min-height: 0;
  }

  .graph-stage {
    position: relative;
    height: 100%;
    min-height: 0;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    background: var(--el-bg-color);
    overflow: hidden;
  }

  .compact-actions {
    position: absolute;
    top: 10px;
    left: 10px;
    right: 10px;
    z-index: 40;
    display: flex;
    justify-content: space-between;
    align-items: center;
    pointer-events: none;
  }

  .actions-left,
  .actions-right {
    pointer-events: auto;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 2px;
    border-radius: 8px;
    border: 1px solid var(--el-border-color-lighter);
    background: color-mix(in srgb, var(--el-bg-color) 88%, transparent);
    backdrop-filter: blur(12px);
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.06);
  }

  .actions-center {
    position: absolute;
    left: 50%;
    z-index: 41;
    display: flex;
    align-items: center;
    pointer-events: auto;
    transform: translateX(-50%);
  }

  .entity-search {
    width: 240px;
  }

  .entity-search :deep(.el-input__wrapper) {
    box-shadow: none;
    background: transparent;
  }

  .search-suffix {
    cursor: pointer;
    color: var(--el-text-color-secondary);
  }

  .action-btn {
    position: relative;
    width: 32px;
    height: 32px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border: none;
    border-radius: 6px;
    background: transparent;
    color: var(--el-text-color-regular);
    cursor: pointer;
  }

  .action-btn:hover {
    background: var(--el-fill-color);
    color: var(--el-color-primary);
  }

  .index-action-btn.has-index-label {
    width: auto;
    min-width: 84px;
    padding: 0 22px 0 8px;
    gap: 6px;
    justify-content: flex-start;
  }

  .index-status-label {
    font-size: 12px;
    line-height: 1;
    color: var(--el-text-color-regular);
    white-space: nowrap;
  }

  .status-dot {
    position: absolute;
    right: 4px;
    bottom: 4px;
    width: 7px;
    height: 7px;
    border-radius: 50%;
    box-shadow: 0 0 0 1px var(--el-bg-color);
  }

  .status-dot--pending,
  .status-dot--active {
    background: var(--el-color-warning);
  }

  .status-dot--active {
    animation: blink 1.2s ease-in-out infinite;
  }

  .status-dot--complete {
    background: var(--el-color-success);
  }

  .graph-main {
    height: 100%;
    min-height: 0;
    padding: 56px 16px 16px;
    box-sizing: border-box;
  }

  .graph-main.is-canvas {
    padding: 0;
  }

  .graph-main.is-list {
    padding: 56px 16px 16px;
    overflow: auto;
  }

  .graph-empty-state {
    height: 100%;
    min-height: 420px;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    text-align: center;
    color: var(--el-text-color-secondary);
  }

  .empty-icon {
    font-size: 42px;
    color: var(--el-color-primary-light-5);
    margin-bottom: 4px;
  }

  .graph-empty-state h3 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .graph-empty-state p {
    margin: 0 0 12px;
    font-size: 13px;
    max-width: 360px;
    line-height: 1.5;
  }

  .btn-icon {
    margin-right: 4px;
  }

  .empty-actions {
    display: flex;
    gap: 8px;
  }

  .floating-panel {
    position: absolute;
    top: 56px;
    right: 10px;
    z-index: 50;
    width: 300px;
    max-height: calc(100% - 70px);
    overflow: auto;
    border-radius: 8px;
    border: 1px solid var(--el-border-color-lighter);
    background: color-mix(in srgb, var(--el-bg-color) 92%, transparent);
    backdrop-filter: blur(12px);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.08);
    font-size: 13px;
  }

  .panel-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 14px;
    border-bottom: 1px solid var(--el-border-color-extra-light);
  }

  .panel-title {
    font-size: 13px;
    font-weight: 600;
  }

  .panel-refresh-btn {
    border: none;
    background: transparent;
    color: var(--el-text-color-secondary);
    cursor: pointer;
    padding: 2px 6px;
  }

  .panel-body {
    padding: 10px 14px 14px;
  }

  .status-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
  }

  .status-label {
    color: var(--el-text-color-secondary);
    font-size: 12px;
  }

  .build-progress {
    margin-bottom: 10px;
  }

  .stats-grid {
    display: grid;
    grid-template-columns: repeat(3, 1fr);
    gap: 8px;
    margin-bottom: 12px;
  }

  .stat-item {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 6px 4px;
    border-radius: 6px;
    background: var(--el-fill-color-light);
  }

  .stat-value {
    font-size: 14px;
    font-weight: 600;
    color: var(--el-text-color-primary);
  }

  .stat-label {
    margin-top: 2px;
    font-size: 11px;
    color: var(--el-text-color-secondary);
  }

  .build-actions .full,
  .full {
    width: 100%;
  }

  .actions-secondary {
    display: flex;
    justify-content: space-between;
    margin-top: 6px;
  }

  .settings-panel :deep(.el-form-item) {
    margin-bottom: 8px;
  }

  .settings-panel :deep(.el-form-item__label) {
    margin-bottom: 2px !important;
    line-height: 1.3;
    padding-bottom: 0;
  }

  .settings-panel :deep(.el-input-number) {
    width: 100%;
  }

  .settings-panel :deep(.el-input-number .el-input__wrapper) {
    min-height: 36px;
    padding-left: 11px;
  }

  .settings-panel :deep(.el-input-number .el-input__inner) {
    text-align: left;
    height: 34px;
    line-height: 34px;
  }

  .config-warning {
    margin-bottom: 12px;
  }

  .extractor-type-cards {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 10px;
    width: 100%;
  }

  .extractor-type-card {
    text-align: left;
    padding: 12px;
    border-radius: 8px;
    border: 1px solid var(--el-border-color);
    background: var(--el-bg-color);
    cursor: pointer;
    font: inherit;
  }

  .extractor-type-card.active {
    border-color: var(--el-color-primary);
    background: var(--el-color-primary-light-9);
  }

  .extractor-type-card.disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .card-header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 6px;
  }

  .type-icon {
    font-size: 22px;
    color: var(--el-color-primary);
  }

  .type-title {
    font-weight: 600;
    font-size: 15px;
  }

  .card-description {
    font-size: 12px;
    color: var(--el-text-color-secondary);
    line-height: 1.4;
  }

  .card-helper {
    margin-top: 6px;
    font-size: 12px;
    color: var(--el-color-primary);
  }

  .card-helper.warning {
    color: var(--el-color-warning);
  }

  .form-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 12px;
  }

  .form-grid-params {
    grid-template-columns: 140px minmax(0, 1fr);
    gap: 12px;
    align-items: start;
  }

  /* 抽取器弹窗：收紧行间距、数字左对齐 */
  .graph-config-form :deep(.el-form-item) {
    margin-bottom: 10px;
  }

  .graph-config-form :deep(.el-form-item__label) {
    margin-bottom: 2px !important;
    line-height: 1.3;
    padding-bottom: 0;
  }

  .concurrency-item :deep(.el-input-number) {
    width: 100%;
  }

  .concurrency-item :deep(.el-input-number .el-input__inner) {
    text-align: left;
  }

  .concurrency-item :deep(.el-input-number .el-input__wrapper) {
    padding-left: 11px;
  }

  .view-mode-group {
    display: inline-flex;
    height: 36px;
    overflow: hidden;
    border-radius: 10px;
    border: 1px solid var(--el-border-color-lighter);
    background: color-mix(in srgb, var(--el-bg-color) 88%, transparent);
    backdrop-filter: blur(12px);
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.06);

    :deep(.el-radio-button__inner) {
      height: 36px;
      padding: 0 18px;
      display: inline-flex;
      align-items: center;
      justify-content: center;
      font-size: 13px;
      line-height: 1;
      border: none !important;
      border-radius: 0 !important;
      box-shadow: none !important;
      background: transparent;
    }

    :deep(.el-radio-button:first-child .el-radio-button__inner) {
      border-radius: 10px 0 0 10px !important;
    }

    :deep(.el-radio-button:last-child .el-radio-button__inner) {
      border-radius: 0 10px 10px 0 !important;
    }

    :deep(.el-radio-button__original-radio:checked + .el-radio-button__inner) {
      background: var(--el-color-primary-light-9);
      color: var(--el-color-primary);
      box-shadow: none !important;
    }
  }

  .graph-data-view {
    height: 100%;
    min-height: 0;
  }

  .graph-canvas-wrap {
    position: relative;
    width: 100%;
    height: 100%;
    min-height: 0;
  }

  .node-list-view {
    height: 100%;
  }

  .graph-stats-bar {
    display: flex;
    gap: 16px;
    margin-bottom: 12px;
    font-size: 13px;
    color: var(--el-text-color-secondary);
  }

  .node-grid {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .node-chip {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    padding: 6px 10px;
    border-radius: 8px;
    border: 1px solid var(--el-border-color-lighter);
    background: var(--el-fill-color-blank);
  }

  .node-name {
    font-size: 13px;
    font-weight: 500;
  }

  .node-type {
    font-size: 11px;
    color: var(--el-text-color-secondary);
  }

  .spin {
    animation: spin 0.8s linear infinite;
  }

  .slide-fade-enter-active,
  .slide-fade-leave-active {
    transition: all 0.18s ease;
  }

  .slide-fade-enter-from,
  .slide-fade-leave-to {
    opacity: 0;
    transform: translateY(-6px);
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }

  @keyframes blink {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.2;
    }
  }
</style>
