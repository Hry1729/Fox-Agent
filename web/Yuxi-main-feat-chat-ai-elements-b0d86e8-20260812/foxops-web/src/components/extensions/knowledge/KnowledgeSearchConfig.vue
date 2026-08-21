<template>
  <div class="knowledge-search-config" v-loading="loading">
    <ElAlert
      v-if="error"
      type="error"
      :closable="false"
      :title="error"
      class="mb"
      show-icon
    >
      <template #default>
        <ElButton type="primary" size="small" @click="load">重新加载</ElButton>
      </template>
    </ElAlert>

    <ElEmpty v-else-if="!loading && !visibleFields.length" description="暂无可配置参数" />

    <ElForm v-else label-position="top" class="config-form">
      <ElFormItem v-for="field in visibleFields" :key="field.key">
        <template #label>
          <span class="field-label">
            {{ field.label || field.key }}
            <ElTooltip v-if="field.description" :content="field.description" placement="top">
              <ArtSvgIcon icon="ri:question-line" class="help-icon" />
            </ElTooltip>
          </span>
        </template>

        <ElSelect
          v-if="field.type === 'select'"
          v-model="localMeta[field.key]"
          style="width: 100%"
          @change="emitUpdate"
        >
          <ElOption
            v-for="opt in field.options || []"
            :key="String(opt.value)"
            :label="opt.label"
            :value="opt.value"
          />
        </ElSelect>

        <ElSelect
          v-else-if="field.type === 'boolean'"
          :model-value="booleanSelectValue(field.key)"
          style="width: 100%"
          @change="(value) => updateBoolean(field.key, value)"
        >
          <ElOption label="启用" value="true" />
          <ElOption label="关闭" value="false" />
        </ElSelect>

        <ElInputNumber
          v-else-if="field.type === 'number'"
          v-model="localMeta[field.key]"
          class="full"
          :min="field.min ?? 0"
          :max="field.max ?? 100"
          :step="field.step ?? 1"
          controls-position="right"
          @change="emitUpdate"
        />

        <ElInput v-else v-model="localMeta[field.key]" @change="emitUpdate" />

        <p v-if="field.description" class="hint">{{ field.description }}</p>
      </ElFormItem>
    </ElForm>
  </div>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { knowledgeApi } from '@/api/knowledge'

  export interface QueryParamField {
    key: string
    label?: string
    type?: string
    description?: string
    options?: Array<{ label: string; value: string | number | boolean }>
    default?: unknown
    min?: number
    max?: number
    step?: number
    depend_on?: [string, unknown] | string[]
  }

  const props = defineProps<{ modelValue?: Record<string, unknown>; kbId: string }>()
  const emit = defineEmits<{
    'update:modelValue': [value: Record<string, unknown>]
    saved: [value: Record<string, unknown>]
  }>()

  const loading = ref(false)
  const saving = ref(false)
  const error = ref('')
  const fields = ref<QueryParamField[]>([])
  const localMeta = reactive<Record<string, any>>({})

  const isDependencySatisfied = (field: QueryParamField) => {
    const dependency = field.depend_on
    if (!dependency || dependency.length < 2) return true
    const [key, expected] = dependency
    return localMeta[key] === expected
  }

  const visibleFields = computed(() =>
    fields.value.filter((field) => field.key !== 'include_distances' && isDependencySatisfied(field))
  )

  const booleanSelectValue = (key: string) => String(Boolean(localMeta[key]))

  const emitUpdate = () => {
    emit('update:modelValue', { ...localMeta })
  }

  const updateBoolean = (key: string, value: string | number | boolean) => {
    localMeta[key] = value === true || value === 'true'
    emitUpdate()
  }

  const applyFields = (options: QueryParamField[]) => {
    fields.value = options.filter((item) => item?.key)

    const supportedKeys = new Set(fields.value.map((item) => item.key))
    Object.keys(localMeta).forEach((key) => {
      if (key !== 'include_distances' && !supportedKeys.has(key)) delete localMeta[key]
    })

    fields.value.forEach((field) => {
      if (field.default !== undefined) {
        localMeta[field.key] =
          field.type === 'boolean' ? Boolean(field.default) : field.default
      } else if (!(field.key in localMeta)) {
        localMeta[field.key] = field.type === 'boolean' ? false : undefined
      }
    })

    // 检索测试默认携带距离分数
    localMeta.include_distances = true

    const saved = localStorage.getItem(`search-config-${props.kbId}`)
    if (saved) {
      try {
        const savedConfig = JSON.parse(saved)
        fields.value.forEach((field) => {
          if (savedConfig[field.key] === undefined) return
          if (field.type === 'boolean' && typeof savedConfig[field.key] === 'string') {
            savedConfig[field.key] = savedConfig[field.key] === 'true'
          }
          localMeta[field.key] = savedConfig[field.key]
        })
        if (savedConfig.include_distances !== undefined) {
          localMeta.include_distances = Boolean(savedConfig.include_distances)
        }
      } catch {
        // ignore broken local cache
      }
    }

    if (props.modelValue) {
      Object.entries(props.modelValue).forEach(([key, value]) => {
        if (key === 'include_distances' || supportedKeys.has(key)) localMeta[key] = value
      })
    }

    localMeta.include_distances = true
    emitUpdate()
  }

  const load = async () => {
    if (!props.kbId) {
      fields.value = []
      return
    }
    loading.value = true
    error.value = ''
    try {
      const result: any = await knowledgeApi.getQueryParams(props.kbId)
      const options = result?.params?.options || result?.options || []
      if (!Array.isArray(options) || !options.length) {
        fields.value = []
        error.value = '当前知识库未返回可配置检索参数'
        return
      }
      applyFields(options)
    } catch (err) {
      error.value = (err as Error)?.message || '加载检索参数失败'
      fields.value = []
    } finally {
      loading.value = false
    }
  }

  const save = async () => {
    if (!props.kbId) {
      ElMessage.error('无法保存配置：缺少知识库 ID')
      return false
    }
    saving.value = true
    try {
      localMeta.include_distances = true
      const payload = { ...localMeta }
      const result: any = await knowledgeApi.updateQueryParams(props.kbId, payload)
      if (result?.message && result.message !== 'success') {
        throw new Error(result.message)
      }
      localStorage.setItem(`search-config-${props.kbId}`, JSON.stringify(payload))
      emitUpdate()
      emit('saved', payload)
      ElMessage.success('检索参数已保存')
      return true
    } catch (err) {
      ElMessage.error((err as Error)?.message || '保存失败')
      return false
    } finally {
      saving.value = false
    }
  }

  watch(() => props.kbId, load, { immediate: true })

  defineExpose({ save, load, saving })
</script>

<style scoped>
  .mb {
    margin-bottom: 12px;
  }
  .config-form {
    display: flex;
    flex-direction: column;
    gap: 0;
  }
  /* 收紧标题与控件、选项之间的间距 */
  .config-form :deep(.el-form-item) {
    margin-bottom: 10px;
  }
  .config-form :deep(.el-form-item__label) {
    margin-bottom: 2px !important;
    line-height: 1.3;
    padding-bottom: 0;
  }
  .config-form :deep(.el-form-item__content) {
    line-height: 1.3;
  }
  .field-label {
    display: inline-flex;
    align-items: center;
    gap: 4px;
  }
  .help-icon {
    font-size: 14px;
    color: var(--el-text-color-secondary);
    cursor: help;
  }
  .full {
    width: 100%;
  }
  /* 数字/文本输入统一左对齐 */
  .config-form :deep(.el-input__inner),
  .config-form :deep(.el-input-number .el-input__inner),
  .config-form :deep(.el-select .el-select__selected-item) {
    text-align: left;
  }
  .config-form :deep(.el-input-number) {
    width: 100%;
  }
  .config-form :deep(.el-input-number .el-input__wrapper) {
    padding-left: 11px;
  }
  .hint {
    margin: 2px 0 0;
    font-size: 12px;
    line-height: 1.4;
    color: var(--el-text-color-secondary);
  }
</style>
