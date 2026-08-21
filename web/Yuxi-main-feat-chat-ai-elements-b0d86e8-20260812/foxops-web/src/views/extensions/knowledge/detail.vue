<template>
  <div
    class="page-content knowledge-detail !border-0 !bg-transparent !shadow-none !p-0"
    :style="{ height: containerMinHeight }"
  >
    <!-- 标题卡片：名称 + 导航 + 操作 -->
    <div class="detail-title-card">
      <div class="title-main">
        <ElButton text class="back-btn" @click="goBack">
          <ArtSvgIcon icon="ri:arrow-left-line" />
          返回
        </ElButton>
        <div class="title-texts">
          <h1>{{ database?.name || kbId }}</h1>
          <p v-if="database?.kb_id || kbId">{{ database?.kb_id || kbId }}</p>
        </div>
        <nav class="detail-nav">
          <button
            v-for="tab in visibleTabs"
            :key="tab.name"
            type="button"
            class="nav-item"
            :class="{ active: activeTab === tab.name }"
            @click="switchTab(tab.name)"
          >
            {{ tab.label }}
          </button>
        </nav>
      </div>
      <div class="title-actions">
        <ElButton @click="copyId">复制 ID</ElButton>
        <ElButton :disabled="!database" @click="editVisible = true">编辑</ElButton>
        <ElButton type="danger" :disabled="!database" @click="removeDatabase">删除</ElButton>
      </div>
    </div>

    <!-- 内容区 -->
    <div
      v-loading="store.detailLoading"
      class="detail-body"
      :class="{
        'is-query-tab': activeTab === 'query',
        'is-fill-tab': isLayoutFillTab
      }"
    >
      <div v-if="detailError" class="detail-error" role="alert">
        <span>{{ detailError }}</span>
        <ElButton link type="primary" @click="reload">重新连接</ElButton>
      </div>
      <template v-if="kbId">
        <!-- 检索测试：保持三卡片，不加外层内容卡 -->
        <div v-show="activeTab === 'query'" class="detail-content-plain">
          <KnowledgeQueryPanel v-if="visitedTabs.has('query')" :kb-id="kbId" />
        </div>

        <!-- RAG 评估：独立内容区 -->
        <div v-show="activeTab === 'evaluation'" class="detail-content-plain">
          <KnowledgeEvaluationPanel
            v-if="isMilvus && visitedTabs.has('evaluation')"
            :kb-id="kbId"
            @goto-benchmarks="switchTab('benchmarks')"
          />
        </div>

        <!-- 评估基准：独立内容区 -->
        <div v-show="activeTab === 'benchmarks'" class="detail-content-plain">
          <KnowledgeBenchmarkPanel
            v-if="isMilvus && visitedTabs.has('benchmarks')"
            :kb-id="kbId"
            @goto-evaluation="switchTab('evaluation')"
          />
        </div>

        <!-- 其余 Tab：统一大内容卡片 -->
        <div
          v-show="!isPlainContentTab"
          class="detail-content-card"
          :class="{ 'is-fill': isFlushContentTab }"
        >
          <KnowledgeDocumentTable
            v-if="isMilvus && visitedTabs.has('filetable')"
            v-show="activeTab === 'filetable'"
            :kb-id="kbId"
          />
          <KnowledgeGraphPanel
            v-if="isMilvus && visitedTabs.has('graph')"
            v-show="activeTab === 'graph'"
            :kb-id="kbId"
          />
          <KnowledgeMindMapPanel
            v-if="isMilvus && visitedTabs.has('mindmap')"
            v-show="activeTab === 'mindmap'"
            :kb-id="kbId"
          />
        </div>
      </template>
      <ElEmpty v-else-if="!store.detailLoading" description="缺少知识库 ID" />
    </div>

    <KnowledgeEditDialog v-model="editVisible" :database="database" @saved="reload" />
  </div>
</template>

<script setup lang="ts">
  import { ElMessage, ElMessageBox } from 'element-plus'
  import { useRoute, useRouter } from 'vue-router'
  import { knowledgeApi } from '@/api/knowledge'
  import KnowledgeBenchmarkPanel from '@/components/extensions/knowledge/KnowledgeBenchmarkPanel.vue'
  import KnowledgeDocumentTable from '@/components/extensions/knowledge/KnowledgeDocumentTable.vue'
  import KnowledgeEditDialog from '@/components/extensions/knowledge/KnowledgeEditDialog.vue'
  import KnowledgeEvaluationPanel from '@/components/extensions/knowledge/KnowledgeEvaluationPanel.vue'
  import KnowledgeGraphPanel from '@/components/extensions/knowledge/KnowledgeGraphPanel.vue'
  import KnowledgeMindMapPanel from '@/components/extensions/knowledge/KnowledgeMindMapPanel.vue'
  import KnowledgeQueryPanel from '@/components/extensions/knowledge/KnowledgeQueryPanel.vue'
  import { useAutoLayoutHeight } from '@/hooks/core/useLayoutHeight'
  import { useKnowledgeStore } from '@/store/modules/knowledge'
  import { useUserStore } from '@/store/modules/user'

  defineOptions({ name: 'ExtensionKnowledgeDetail' })

  const route = useRoute()
  const router = useRouter()
  const userStore = useUserStore()
  const store = useKnowledgeStore()
  const { containerMinHeight } = useAutoLayoutHeight()

  const kbId = computed(() => String(route.params.kbId || ''))
  const database = computed(() => store.currentDatabase)
  const editVisible = ref(false)
  const detailError = ref('')
  const activeTab = ref('query')
  const visitedTabs = ref(new Set<string>(['query']))

  const allTabs = [
    { name: 'filetable', label: '文件管理', milvusOnly: true },
    { name: 'query', label: '检索测试', milvusOnly: false },
    { name: 'graph', label: '知识图谱', milvusOnly: true },
    { name: 'mindmap', label: '知识导图', milvusOnly: true },
    { name: 'evaluation', label: 'RAG 评估', milvusOnly: true },
    { name: 'benchmarks', label: '评估基准', milvusOnly: true }
  ]

  const kbType = computed(() => String(database.value?.kb_type || '').toLowerCase())
  const isMilvus = computed(() => {
    if (!kbType.value) return true
    return (
      kbType.value.includes('milvus') ||
      kbType.value.includes('lightrag') ||
      (!kbType.value.includes('dify') && !kbType.value.includes('notion'))
    )
  })

  const visibleTabs = computed(() => allTabs.filter((tab) => !tab.milvusOnly || isMilvus.value))

  const isPlainContentTab = computed(() =>
    ['query', 'evaluation', 'benchmarks'].includes(activeTab.value)
  )

  const isLayoutFillTab = computed(() =>
    ['graph', 'mindmap', 'evaluation', 'benchmarks'].includes(activeTab.value)
  )

  const isFlushContentTab = computed(
    () => activeTab.value === 'graph' || activeTab.value === 'mindmap'
  )

  const switchTab = (name: string) => {
    activeTab.value = name
    if (!visitedTabs.value.has(name)) {
      const next = new Set(visitedTabs.value)
      next.add(name)
      visitedTabs.value = next
    }
  }

  const goBack = () => router.push('/extensions/knowledge')

  const reload = async () => {
    if (!userStore.isAdmin) {
      router.replace('/extensions/skills')
      return
    }
    detailError.value = ''
    try {
      await store.loadDatabase(kbId.value)
      const initial = isMilvus.value ? 'filetable' : 'query'
      activeTab.value = initial
      visitedTabs.value = new Set([initial])
    } catch (error) {
      detailError.value = (error as Error)?.message || '知识库连接失败'
      ElMessage.error(detailError.value)
    }
  }

  const copyId = async () => {
    try {
      await navigator.clipboard.writeText(kbId.value)
      ElMessage.success('已复制知识库 ID')
    } catch {
      ElMessage.error('复制失败')
    }
  }

  const removeDatabase = async () => {
    try {
      await ElMessageBox.confirm('确认删除该知识库？此操作不可恢复。', '删除知识库', {
        type: 'warning'
      })
      await knowledgeApi.deleteDatabase(kbId.value)
      ElMessage.success('已删除')
      goBack()
    } catch {
      // 取消
    }
  }

  watch(kbId, reload, { immediate: true })
  onBeforeUnmount(() => store.clearCurrent())
</script>

<style scoped>
  .knowledge-detail {
    display: flex;
    flex-direction: column;
    gap: 16px;
    min-height: 0;
    padding: 0;
    overflow: hidden;
    box-sizing: border-box;
    background: transparent !important;
    border: none !important;
    box-shadow: none !important;
  }

  .detail-title-card {
    flex-shrink: 0;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    background: var(--el-bg-color);
  }

  .title-main {
    display: flex;
    align-items: center;
    gap: 12px;
    min-width: 0;
    flex: 1;
  }

  .back-btn {
    flex-shrink: 0;
  }

  .title-texts {
    flex-shrink: 0;
    min-width: 0;
  }

  .title-texts h1 {
    margin: 0;
    font-size: 18px;
    font-weight: 600;
    line-height: 1.3;
    white-space: nowrap;
  }

  .title-texts p {
    margin: 2px 0 0;
    color: var(--el-text-color-secondary);
    font-size: 12px;
    white-space: nowrap;
  }

  .detail-nav {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 4px;
    margin-left: 8px;
    min-width: 0;
  }

  .nav-item {
    height: var(--el-component-size, 32px);
    padding: 0 14px;
    border: none;
    border-radius: var(--el-border-radius-base, 4px);
    background: transparent;
    color: var(--el-text-color-regular);
    font-size: 15px;
    line-height: 1;
    cursor: pointer;
    white-space: nowrap;
    box-sizing: border-box;
  }

  .nav-item:hover {
    background: var(--el-fill-color-light);
    color: var(--el-color-primary);
  }

  .nav-item.active {
    height: var(--el-component-size, 32px);
    background: var(--el-color-primary-light-9);
    color: var(--el-color-primary);
    font-size: 15px;
    font-weight: 600;
  }

  .title-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0;
    flex-shrink: 0;
  }

  .title-actions :deep(.el-button) {
    height: var(--el-component-size, 32px);
    margin-left: 0;
  }

  .title-actions :deep(.el-button + .el-button) {
    margin-left: 4px;
  }

  .detail-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .detail-error {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 12px;
    padding: 10px 14px;
    border: 1px solid var(--el-color-danger-light-7);
    border-radius: 8px;
    background: var(--el-color-danger-light-9);
    color: var(--el-color-danger);
    font-size: 13px;
  }

  .detail-body.is-query-tab,
  .detail-body.is-fill-tab {
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .detail-content-plain,
  .detail-content-card {
    min-height: 0;
  }

  .detail-body.is-query-tab .detail-content-plain,
  .detail-body.is-fill-tab .detail-content-card,
  .detail-body.is-fill-tab .detail-content-plain {
    flex: 1;
    height: 100%;
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .detail-body.is-fill-tab .detail-content-plain > * {
    flex: 1;
    min-height: 0;
  }

  .detail-content-card {
    padding: 16px;
    border: 1px solid var(--el-border-color-lighter);
    border-radius: 10px;
    background: var(--el-bg-color);
    box-sizing: border-box;
  }

  .detail-content-card.is-fill {
    padding: 0;
    overflow: hidden;
  }

  /* 内容卡内嵌套卡片去边框，避免双层卡片感 */
  .detail-content-card :deep(.file-table-card) {
    border: none;
    box-shadow: none;
    background: transparent;
  }

  .detail-content-card :deep(.graph-stage) {
    border: none;
    border-radius: 0;
  }

  .detail-content-card.is-fill :deep(.knowledge-graph-panel),
  .detail-content-card.is-fill :deep(.knowledge-mindmap-panel) {
    height: 100%;
  }

  .detail-content-card :deep(.knowledge-benchmark-panel),
  .detail-content-card :deep(.knowledge-evaluation-panel),
  .detail-content-plain :deep(.knowledge-benchmark-panel),
  .detail-content-plain :deep(.knowledge-evaluation-panel) {
    flex: 1;
    min-height: 0;
  }

  .detail-body.is-fill-tab .detail-content-card > * {
    flex: 1;
    min-height: 0;
  }

  .detail-content-card.is-fill :deep(.graph-stage),
  .detail-content-card.is-fill :deep(.mindmap-stage) {
    border: none;
    border-radius: 0;
  }
</style>
