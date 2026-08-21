<template>
  <ElDialog
    :model-value="modelValue"
    width="840px"
    destroy-on-close
    :show-close="false"
    class="extension-dialog extension-card-detail-dialog"
    header-class="extension-dialog-header"
    body-class="extension-dialog-body"
    @close="$emit('update:modelValue', false)"
  >
    <template #header>
      <ExtensionDialogHeader
        :title="tool?.name || '工具详情'"
        @close="$emit('update:modelValue', false)"
      />
    </template>
    <div v-if="tool" class="tool-detail">
      <section>
        <h4>描述</h4>
        <p>{{ tool.description || '无描述' }}</p>
      </section>
      <section v-if="tool.config_guide">
        <h4>配置说明</h4>
        <pre class="config-guide">{{ tool.config_guide }}</pre>
      </section>
      <section>
        <h4>分类</h4>
        <span class="category-tag" :class="`tag-${categoryColor}`">
          {{ categoryLabels[tool.category || ''] || tool.category || '-' }}
        </span>
      </section>
      <section>
        <h4>标签</h4>
        <div class="tags">
          <ElTag v-for="tag in tool.tags || []" :key="tag" size="small">{{ tag }}</ElTag>
          <span v-if="!tool.tags?.length" class="muted">无</span>
        </div>
      </section>
      <section v-if="tool.args?.length">
        <h4>参数</h4>
        <ElTable :data="tool.args" size="small" border>
          <ElTableColumn prop="name" label="参数名" min-width="120" />
          <ElTableColumn prop="type" label="类型" width="100" />
          <ElTableColumn label="必填" width="72" align="center">
            <template #default="{ row }">{{ row.required ? '是' : '否' }}</template>
          </ElTableColumn>
          <ElTableColumn prop="description" label="描述" min-width="220" show-overflow-tooltip />
        </ElTable>
      </section>
    </div>
  </ElDialog>
</template>

<script setup lang="ts">
  import type { SystemTool } from '@/api/tools'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'

  const props = defineProps<{ modelValue: boolean; tool?: SystemTool | null }>()
  defineEmits<{ 'update:modelValue': [value: boolean] }>()

  const categoryLabels: Record<string, string> = {
    buildin: '内置工具',
    knowledge: '知识库',
    mysql: 'MySQL',
    debug: '调试'
  }
  const categoryColors: Record<string, 'blue' | 'purple' | 'green' | 'orange'> = {
    buildin: 'blue',
    knowledge: 'purple',
    mysql: 'green',
    debug: 'orange'
  }
  const categoryColor = computed(
    () => categoryColors[props.tool?.category || ''] || ('blue' as const)
  )
</script>

<style scoped>
  .tool-detail {
    display: flex;
    flex-direction: column;
    gap: 14px;
    max-height: 68vh;
    overflow: auto;
  }
  section h4 {
    margin: 0 0 6px;
    font-size: 13px;
    color: var(--el-text-color-secondary);
  }
  section p {
    margin: 0;
    line-height: 1.6;
  }
  .config-guide {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
    font-size: 12px;
    line-height: 1.55;
  }
  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .muted {
    color: var(--el-text-color-secondary);
  }
  .category-tag {
    display: inline-flex;
    align-items: center;
    height: 22px;
    padding: 0 8px;
    border-radius: 4px;
    font-size: 12px;
  }
  .tag-blue {
    background: var(--el-color-primary-light-9);
    color: var(--el-color-primary);
  }
  .tag-purple {
    background: #f3e8ff;
    color: #7c3aed;
  }
  .tag-green {
    background: var(--el-color-success-light-9);
    color: var(--el-color-success);
  }
  .tag-orange {
    background: var(--el-color-warning-light-9);
    color: var(--el-color-warning);
  }
</style>
