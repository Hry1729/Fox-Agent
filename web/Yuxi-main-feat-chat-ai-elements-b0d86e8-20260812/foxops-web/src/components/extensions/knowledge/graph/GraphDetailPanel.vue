<template>
  <transition name="slide-fade-left">
    <div v-if="visible" class="detail-panel">
      <div class="panel-header">
        <span class="panel-title">{{ title }}</span>
        <button type="button" class="close-btn" @click="$emit('close')">
          <ArtSvgIcon icon="ri:close-line" />
        </button>
      </div>
      <div class="panel-body">
        <template v-if="item">
          <template v-if="type === 'node'">
            <div :class="rowClass(item.data?.label)">
              <span class="detail-label">名称</span>
              <span class="detail-value">{{ formatValue(item.data?.label) }}</span>
            </div>
            <div class="detail-row">
              <span class="detail-label">ID</span>
              <span class="detail-value detail-id">{{ item.id }}</span>
            </div>
            <div v-if="item.data?.visualLabel" class="detail-row">
              <span class="detail-label">类型</span>
              <span class="detail-value">{{ item.data.visualLabel }}</span>
            </div>
            <template v-if="item.data?.original?.labels?.length">
              <div class="detail-row">
                <span class="detail-label">标签</span>
                <span class="detail-value">
                  <ElTag
                    v-for="tag in item.data.original.labels"
                    :key="tag"
                    size="small"
                    class="tag"
                  >
                    {{ tag }}
                  </ElTag>
                </span>
              </div>
            </template>
            <template v-if="nodeProperties">
              <div
                v-for="(value, key) in nodeProperties"
                :key="String(key)"
                :class="rowClass(value)"
              >
                <span class="detail-label">{{ key }}</span>
                <span class="detail-value">{{ formatValue(value) }}</span>
              </div>
            </template>
          </template>

          <template v-else-if="type === 'edge'">
            <div :class="rowClass(item.data?.label)">
              <span class="detail-label">类型</span>
              <span class="detail-value">{{ formatValue(item.data?.label) }}</span>
            </div>
            <div class="detail-row">
              <span class="detail-label">起点</span>
              <span class="detail-value detail-id">{{ item.source }}</span>
            </div>
            <div class="detail-row">
              <span class="detail-label">终点</span>
              <span class="detail-value detail-id">{{ item.target }}</span>
            </div>
            <template v-if="edgeProperties">
              <div
                v-for="(value, key) in edgeProperties"
                :key="String(key)"
                :class="rowClass(value)"
              >
                <span class="detail-label">{{ key }}</span>
                <span class="detail-value">{{ formatValue(value) }}</span>
              </div>
            </template>
          </template>
        </template>
      </div>
    </div>
  </transition>
</template>

<script setup lang="ts">
  import { computed } from 'vue'

  const STACK_THRESHOLD = 50

  const props = defineProps<{
    visible: boolean
    item?: any
    type?: 'node' | 'edge' | null
  }>()

  defineEmits<{ close: [] }>()

  const title = computed(() => (props.type === 'node' ? '节点详情' : '关系详情'))

  const isOverThreshold = (value: unknown) =>
    typeof value === 'string' && value.length > STACK_THRESHOLD

  const rowClass = (value: unknown) =>
    isOverThreshold(value) ? 'detail-row detail-row--stack' : 'detail-row'

  const formatValue = (value: unknown) => {
    if (value == null) return '-'
    if (typeof value === 'object') return JSON.stringify(value)
    return String(value)
  }

  const nodeProperties = computed(() => {
    const properties = props.item?.data?.original?.properties
    return properties && typeof properties === 'object' ? properties : null
  })

  const edgeProperties = computed(() => {
    const properties = props.item?.data?.original?.properties
    if (!properties || typeof properties !== 'object') return null
    const hidden = new Set(['source_id', 'target_id', '_id', 'truncate'])
    return Object.fromEntries(
      Object.entries(properties).filter(([key]) => !hidden.has(key))
    )
  })
</script>

<style scoped>
  .detail-panel {
    position: absolute;
    top: 56px;
    left: 10px;
    z-index: 60;
    width: 280px;
    max-height: calc(100% - 70px);
    overflow-y: auto;
    border-radius: 8px;
    border: 1px solid var(--el-border-color-lighter);
    background: color-mix(in srgb, var(--el-bg-color) 92%, transparent);
    backdrop-filter: blur(16px);
    box-shadow: 0 4px 16px rgba(0, 0, 0, 0.08);
    font-size: 13px;
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
  }

  .close-btn {
    margin-left: auto;
    border: none;
    background: transparent;
    color: var(--el-text-color-secondary);
    cursor: pointer;
    padding: 2px;
  }

  .panel-body {
    padding: 10px 14px;
  }

  .detail-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    padding: 6px 0;
    border-bottom: 1px solid var(--el-border-color-extra-light);
  }

  .detail-row:last-child {
    border-bottom: none;
  }

  .detail-row--stack {
    flex-direction: column;
  }

  .detail-label {
    flex-shrink: 0;
    margin-right: 8px;
    margin-bottom: 2px;
    font-size: 12px;
    color: var(--el-text-color-secondary);
  }

  .detail-value {
    text-align: right;
    color: var(--el-text-color-primary);
    word-break: break-all;
  }

  .detail-row--stack .detail-value {
    text-align: left;
  }

  .detail-id {
    font-size: 11px;
    color: var(--el-text-color-secondary);
  }

  .tag {
    margin-left: 4px;
  }

  .slide-fade-left-enter-active {
    transition: all 0.25s ease-out;
  }

  .slide-fade-left-leave-active {
    transition: all 0.2s cubic-bezier(1, 0.5, 0.8, 1);
  }

  .slide-fade-left-enter-from,
  .slide-fade-left-leave-to {
    transform: translateX(-20px);
    opacity: 0;
  }
</style>
