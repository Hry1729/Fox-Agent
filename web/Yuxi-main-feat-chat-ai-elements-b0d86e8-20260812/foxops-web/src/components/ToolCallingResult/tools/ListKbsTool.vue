<template>
  <BaseToolCall :tool-call="toolCall" :hide-params="true">
    <template #header>
      <div class="sep-header">
        <span class="note">{{ operationLabel }}</span>
        <span class="separator">|</span>
        <span class="description">{{ headerSummary }}</span>
      </div>
    </template>
    <template #result="{}">
      <div class="list-kbs-result">
        <div v-if="resultState === 'loading'" class="kb-state">正在获取知识库列表…</div>
        <div v-else-if="resultState === 'unavailable'" class="kb-state is-warning">
          知识库结果暂未返回
        </div>
        <template v-else>
          <div class="kb-count">共 {{ kbList.length }} 个知识库</div>
          <div v-if="kbList.length" class="kb-list">
            <div v-for="kb in kbList" :key="kb.name" class="kb-item">
              <div class="kb-name">{{ kb.name }}</div>
              <div class="kb-description">{{ kb.description || '无描述' }}</div>
            </div>
          </div>
        </template>
      </div>
    </template>
  </BaseToolCall>
</template>

<script setup>
  import { computed } from 'vue'
  import BaseToolCall from '../BaseToolCall.vue'
  import { normalizeKbListResult } from './listKbsResult.js'

  const props = defineProps({
    toolCall: {
      type: Object,
      required: true
    }
  })

  const toolName = computed(() => props.toolCall.name || props.toolCall.function?.name || '知识库')

  const operationLabel = computed(() => `${toolName.value} 列表`)

  const kbList = computed(() => {
    const resultContent = props.toolCall.tool_call_result?.content
    return normalizeKbListResult(resultContent)
  })

  const resultState = computed(() => {
    if (props.toolCall.tool_call_result == null) {
      return props.toolCall.status === 'success' ? 'unavailable' : 'loading'
    }
    return kbList.value.length > 0 ? 'ready' : 'empty'
  })

  const headerSummary = computed(() => {
    if (resultState.value === 'loading') return '正在获取知识库'
    if (resultState.value === 'unavailable') return '知识库结果未返回'

    const names = kbList.value.map((kb) => kb?.name).filter(Boolean)
    if (!names.length) return '暂无可访问知识库'

    const previewNames = names.slice(0, 3).join('，')
    const remainingCount = names.length - 3
    return remainingCount > 0
      ? `${names.length}个知识库：${previewNames} 等${remainingCount}个`
      : `${names.length}个知识库：${previewNames}`
  })
</script>

<style scoped lang="less">
  .list-kbs-result {
    background: var(--gray-0);
    border-radius: 8px;
    padding: 8px;

    .kb-count {
      font-size: 12px;
      color: var(--gray-700);
      margin-bottom: 12px;
    }

    .kb-state {
      padding: 4px;
      font-size: 12px;
      color: var(--gray-600);

      &.is-warning {
        color: var(--el-color-warning-dark-2, #b88230);
      }
    }

    .kb-list {
      display: flex;
      flex-direction: column;
      gap: 8px;
    }

    .kb-item {
      padding: 10px 12px;
      background: var(--gray-10);
      border-radius: 6px;
      border: 1px solid var(--gray-100);

      .kb-name {
        font-size: 13px;
        font-weight: 500;
        color: var(--gray-700);
        margin-bottom: 4px;
      }

      .kb-description {
        font-size: 12px;
        color: var(--gray-600);
      }
    }
  }
</style>
