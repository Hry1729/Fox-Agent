<template>
  <ElDialog
    :model-value="modelValue"
    width="600px"
    destroy-on-close
    :show-close="false"
    class="extension-dialog"
    header-class="extension-dialog-header"
    body-class="extension-dialog-body"
    footer-class="extension-dialog-footer"
    @close="$emit('update:modelValue', false)"
  >
    <template #header>
      <ExtensionDialogHeader :title="title" @close="$emit('update:modelValue', false)" />
    </template>
    <ElAlert
      v-if="pendingCount > 0"
      type="info"
      :closable="false"
      show-icon
      class="pending-alert"
      :title="`将提交 ${formattedPendingCount} 个待入库文件，任务会在后台按批处理，可在任务中心查看进度。`"
    />

    <p class="params-info">调整分块参数可以控制文本的切分方式，影响检索质量和文档加载效率。</p>

    <ElForm label-position="top" class="params">
      <ElFormItem>
        <template #label>
          <span class="field-label">
            分块策略
            <ElTooltip :content="getChunkPresetDescription(effectivePresetId)" placement="top">
              <ArtSvgIcon icon="ri:question-line" class="help-icon" />
            </ElTooltip>
          </span>
        </template>
        <ElSelect
          v-model="chunkPresetId"
          clearable
          placeholder="沿用知识库默认策略"
          style="width: 100%"
        >
          <ElOption
            v-for="item in CHUNK_PRESET_OPTIONS"
            :key="item.value"
            :label="item.label"
            :value="item.value"
          />
        </ElSelect>
        <p class="hint">留空时沿用知识库默认策略。</p>
      </ElFormItem>

      <div class="chunk-row">
        <ElFormItem>
          <template #label>
            <span class="field-label">
              最大 Token 数
              <ElTooltip content="每个文本片段的最大 token 数，留空时使用默认值 512" placement="top">
                <ArtSvgIcon icon="ri:question-line" class="help-icon" />
              </ElTooltip>
            </span>
          </template>
          <ElInputNumber
            v-model="parserConfig.chunk_token_num"
            :min="100"
            :max="10000"
            placeholder="默认 512"
            controls-position="right"
            style="width: 100%"
          />
        </ElFormItem>

        <ElFormItem>
          <template #label>
            <span class="field-label">
              重叠比例 (%)
              <ElTooltip
                content="相邻文本片段按 token 数计算的重叠比例，留空时使用默认值 0"
                placement="top"
              >
                <ArtSvgIcon icon="ri:question-line" class="help-icon" />
              </ElTooltip>
            </span>
          </template>
          <ElInputNumber
            v-model="parserConfig.overlapped_percent"
            :min="0"
            :max="99"
            placeholder="默认 0"
            controls-position="right"
            style="width: 100%"
          />
        </ElFormItem>

        <ElFormItem>
          <template #label>
            <span class="field-label">
              分隔符
              <ElTooltip content="支持 \\n、\\t 等转义字符。留空时使用默认分隔符 \\n" placement="top">
                <ArtSvgIcon icon="ri:question-line" class="help-icon" />
              </ElTooltip>
            </span>
          </template>
          <ElInput v-model="parserConfig.delimiter" placeholder="默认 \n，可输入 \n\n\n 或 ---" />
        </ElFormItem>
      </div>
    </ElForm>

    <template #footer>
      <ElButton @click="$emit('update:modelValue', false)">取消</ElButton>
      <ElButton type="primary" :loading="loading" @click="submit">确定</ElButton>
    </template>
  </ElDialog>
</template>

<script setup lang="ts">
  import {
    CHUNK_PRESET_OPTIONS,
    buildChunkParamsPayload,
    getChunkPresetDescription,
    isPlainObject
  } from '@/utils/chunkPresets'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'

  interface ParserConfigForm {
    chunk_token_num?: number
    overlapped_percent?: number
    delimiter?: string
  }

  const props = withDefaults(
    defineProps<{
      modelValue: boolean
      title?: string
      pendingCount?: number
      defaultPresetId?: string
      initialParams?: Record<string, unknown> | null
      loading?: boolean
    }>(),
    {
      title: '入库参数配置',
      pendingCount: 0,
      defaultPresetId: 'general',
      initialParams: null,
      loading: false
    }
  )

  const emit = defineEmits<{
    'update:modelValue': [value: boolean]
    confirm: [params: Record<string, unknown>]
  }>()

  const chunkPresetId = ref('')
  const parserConfig = reactive<ParserConfigForm>({
    chunk_token_num: undefined,
    overlapped_percent: undefined,
    delimiter: ''
  })

  const effectivePresetId = computed(() => chunkPresetId.value || props.defaultPresetId)

  const formattedPendingCount = computed(() =>
    Number(props.pendingCount || 0).toLocaleString('zh-CN')
  )

  const resetForm = (processingParams: Record<string, unknown> | null = null) => {
    chunkPresetId.value = processingParams?.chunk_preset_id
      ? String(processingParams.chunk_preset_id)
      : ''

    const rawConfig = isPlainObject(processingParams?.chunk_parser_config)
      ? (processingParams.chunk_parser_config as Record<string, unknown>)
      : {}

    parserConfig.chunk_token_num =
      rawConfig.chunk_token_num === undefined || rawConfig.chunk_token_num === null
        ? undefined
        : Number(rawConfig.chunk_token_num)
    parserConfig.overlapped_percent =
      rawConfig.overlapped_percent === undefined || rawConfig.overlapped_percent === null
        ? undefined
        : Number(rawConfig.overlapped_percent)
    parserConfig.delimiter = rawConfig.delimiter ? String(rawConfig.delimiter) : ''
  }

  const submit = () => {
    emit(
      'confirm',
      buildChunkParamsPayload(
        {
          chunk_preset_id: chunkPresetId.value || undefined,
          chunk_parser_config: { ...parserConfig }
        },
        { includeSizeOverlap: true }
      )
    )
  }

  watch(
    () => props.modelValue,
    (visible) => {
      if (visible) resetForm(props.initialParams)
    }
  )

  watch(
    () => props.initialParams,
    (params) => {
      if (props.modelValue) resetForm(params)
    }
  )
</script>

<style scoped>
  .pending-alert {
    margin-bottom: 12px;
  }
  .params-info {
    margin: 0 0 12px;
    font-size: 13px;
    line-height: 1.5;
    color: var(--el-text-color-secondary);
  }
  .params {
    margin-top: 0;
  }
  .hint {
    margin: 6px 0 0;
    font-size: 12px;
    color: var(--el-text-color-secondary);
    line-height: 1.5;
  }
  .chunk-row {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 12px;
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
  @media (max-width: 640px) {
    .chunk-row {
      grid-template-columns: 1fr;
    }
  }
</style>
