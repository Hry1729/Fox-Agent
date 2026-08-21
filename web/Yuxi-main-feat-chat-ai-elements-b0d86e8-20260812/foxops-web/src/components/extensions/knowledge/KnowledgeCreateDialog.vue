<template>
  <ElDialog
    :model-value="modelValue"
    width="820px"
    top="5vh"
    destroy-on-close
    :show-close="false"
    class="knowledge-create-dialog extension-dialog"
    header-class="extension-dialog-header"
    body-class="extension-dialog-body"
    footer-class="extension-dialog-footer"
    @close="$emit('update:modelValue', false)"
  >
    <template #header>
      <ExtensionDialogHeader title="新建知识库" @close="$emit('update:modelValue', false)" />
    </template>
    <div v-loading="submitting" class="create-form">
      <ElForm label-position="top" class="kb-form-grid">
        <ElFormItem label="知识库类型" required class="span-2">
          <div class="type-grid">
            <button
              v-for="(info, key) in store.types"
              :key="key"
              type="button"
              class="type-card"
              :class="{ active: form.kb_type === key }"
              @click="form.kb_type = String(key)"
            >
              <strong>{{ (info as any).label || key }}</strong>
              <p>{{ (info as any).description || '无描述' }}</p>
            </button>
          </div>
        </ElFormItem>

        <ElFormItem
          label="知识库名称"
          required
          :class="{ 'span-2': !selectedType?.requires_embedding_model }"
        >
          <ElInput v-model="form.name" placeholder="新建知识库名称" />
        </ElFormItem>

        <ElFormItem v-if="selectedType?.requires_embedding_model" label="嵌入模型">
          <ElSelect
            v-model="form.embedding_model_spec"
            filterable
            clearable
            placeholder="请选择嵌入模型"
            style="width: 100%"
          >
            <ElOptionGroup
              v-for="(group, providerId) in embeddingModels"
              :key="providerId"
              :label="String(providerId)"
            >
              <ElOption
                v-for="model in group"
                :key="model.spec"
                :label="model.display_name || model.spec"
                :value="model.spec"
              />
            </ElOptionGroup>
          </ElSelect>
        </ElFormItem>

        <ElFormItem v-if="selectedType?.requires_embedding_model" label="分块策略">
          <ElSelect v-model="form.chunk_preset_id" style="width: 100%">
            <ElOption
              v-for="item in CHUNK_PRESET_OPTIONS"
              :key="item.value"
              :label="item.label"
              :value="item.value"
            />
          </ElSelect>
          <p class="hint">{{ getChunkPresetDescription(form.chunk_preset_id) }}</p>
        </ElFormItem>

        <ElFormItem
          v-for="field in createParamOptions"
          :key="field.key"
          :label="field.label || field.key"
          :required="field.required"
        >
          <ElInput
            v-if="field.type === 'password'"
            v-model="form.additional_params[field.key]"
            type="password"
            show-password
            :placeholder="field.placeholder"
          />
          <ElInputNumber
            v-else-if="field.type === 'number'"
            v-model="form.additional_params[field.key]"
            class="full"
          />
          <ElSwitch
            v-else-if="field.type === 'boolean'"
            v-model="form.additional_params[field.key]"
          />
          <ElSelect
            v-else-if="field.type === 'select'"
            v-model="form.additional_params[field.key]"
            style="width: 100%"
          >
            <ElOption
              v-for="opt in field.options || []"
              :key="opt.value"
              :label="opt.label"
              :value="opt.value"
            />
          </ElSelect>
          <ElInput
            v-else
            v-model="form.additional_params[field.key]"
            :placeholder="field.placeholder"
          />
          <p v-if="field.description" class="hint">{{ field.description }}</p>
        </ElFormItem>

        <ElFormItem label="知识库描述" class="span-2">
          <ElInput
            v-model="form.description"
            type="textarea"
            :rows="2"
            placeholder="描述越详细，智能体越容易选择到合适的工具"
          />
        </ElFormItem>

        <ElFormItem label="共享范围" class="span-2">
          <ShareScopeForm v-model="form.share_config" />
        </ElFormItem>
      </ElForm>
    </div>

    <template #footer>
      <ElButton @click="$emit('update:modelValue', false)">取消</ElButton>
      <ElButton type="primary" :loading="submitting" @click="submit">创建</ElButton>
    </template>
  </ElDialog>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { knowledgeApi } from '@/api/knowledge'
  import { modelProviderApi } from '@/api/models'
  import ShareScopeForm, {
    type ShareConfig
  } from '@/components/extensions/common/ShareScopeForm.vue'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'
  import { useKnowledgeStore } from '@/store/modules/knowledge'
  import { CHUNK_PRESET_OPTIONS, getChunkPresetDescription } from '@/utils/chunkPresets'

  const props = defineProps<{ modelValue: boolean }>()
  const emit = defineEmits<{ 'update:modelValue': [value: boolean]; created: [] }>()

  const store = useKnowledgeStore()
  const submitting = ref(false)
  const embeddingModels = ref<Record<string, Array<{ spec: string; display_name?: string }>>>({})

  const form = reactive({
    kb_type: '',
    name: '',
    description: '',
    embedding_model_spec: '',
    chunk_preset_id: 'general',
    additional_params: {} as Record<string, any>,
    share_config: { access_level: 'global', department_ids: [], user_uids: [] } as ShareConfig
  })

  const selectedType = computed(() => (form.kb_type ? (store.types[form.kb_type] as any) : null))

  const createParamOptions = computed(() => {
    const raw = selectedType.value?.create_params
    const params = Array.isArray(raw) ? raw : raw?.options || []
    return params.map((item: any) => ({
      key: item.key,
      label: item.label,
      type: item.type || 'text',
      required: !!item.required,
      placeholder: item.placeholder,
      description: item.description,
      options: item.options || [],
      default: item.default
    }))
  })

  const resetCreateParams = () => {
    form.additional_params = {}
    for (const field of createParamOptions.value) {
      if ('default' in field && field.default !== undefined) {
        form.additional_params[field.key] = field.default
      } else if (field.type === 'boolean') {
        form.additional_params[field.key] = false
      } else if (field.type === 'number') {
        form.additional_params[field.key] = undefined
      } else {
        form.additional_params[field.key] = ''
      }
    }
  }

  const loadEmbeddingModels = async () => {
    try {
      const result: any = await modelProviderApi.getModelsV2('embedding')
      const data = result?.data || result
      const providers = data?.providers || data || {}
      const mapped: Record<string, Array<{ spec: string; display_name?: string }>> = {}
      Object.entries(providers).forEach(([providerId, value]: [string, any]) => {
        if (Array.isArray(value)) mapped[providerId] = value
        else mapped[providerId] = value?.models || []
      })
      embeddingModels.value = mapped
      if (!form.embedding_model_spec) {
        for (const models of Object.values(mapped)) {
          if (models?.[0]?.spec) {
            form.embedding_model_spec = models[0].spec
            break
          }
        }
      }
    } catch {
      embeddingModels.value = {}
    }
  }

  const submit = async () => {
    if (!form.kb_type || !selectedType.value) {
      ElMessage.warning('请选择知识库类型')
      return
    }
    if (!form.name.trim()) {
      ElMessage.warning('请填写知识库名称')
      return
    }
    for (const field of createParamOptions.value) {
      if (
        field.required &&
        (form.additional_params[field.key] == null || form.additional_params[field.key] === '')
      ) {
        ElMessage.warning(`请填写 ${field.label || field.key}`)
        return
      }
    }
    if (selectedType.value?.requires_embedding_model && !form.embedding_model_spec) {
      ElMessage.warning('请选择嵌入模型')
      return
    }

    submitting.value = true
    try {
      const additional_params: Record<string, unknown> = {}
      for (const field of createParamOptions.value) {
        const value = form.additional_params[field.key]
        additional_params[field.key] = typeof value === 'string' ? value.trim() : value
      }
      if (selectedType.value?.requires_embedding_model) {
        additional_params.chunk_preset_id = form.chunk_preset_id || 'general'
      }
      const share = form.share_config || { access_level: 'global' }
      await knowledgeApi.createDatabase({
        kb_type: form.kb_type,
        database_name: form.name.trim(),
        description: form.description?.trim() || '',
        embedding_model_spec: selectedType.value?.requires_embedding_model
          ? form.embedding_model_spec || undefined
          : undefined,
        additional_params,
        share_config: {
          access_level: share.access_level || share.level || 'global',
          department_ids:
            (share.access_level || share.level) === 'department' ? share.department_ids || [] : [],
          user_uids: (share.access_level || share.level) === 'user' ? share.user_uids || [] : []
        }
      })
      ElMessage.success('创建成功')
      emit('created')
      emit('update:modelValue', false)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '创建失败')
    } finally {
      submitting.value = false
    }
  }

  watch(
    () => form.kb_type,
    () => {
      resetCreateParams()
    }
  )

  watch(
    () => props.modelValue,
    async (open) => {
      if (!open) return
      if (!Object.keys(store.types).length) await store.loadTypes()
      if (!form.kb_type) form.kb_type = Object.keys(store.types)[0] || ''
      resetCreateParams()
      await loadEmbeddingModels()
    }
  )

  onMounted(async () => {
    if (!Object.keys(store.types).length) await store.loadTypes()
    if (!form.kb_type) form.kb_type = Object.keys(store.types)[0] || ''
    await loadEmbeddingModels()
  })
</script>

<style scoped>
  .kb-form-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    column-gap: 14px;
    row-gap: 0;
  }
  .kb-form-grid :deep(.el-form-item) {
    margin-bottom: 8px;
  }
  .kb-form-grid :deep(.el-form-item__label) {
    margin-bottom: 2px !important;
    line-height: 1.2;
    height: auto;
  }
  .span-2 {
    grid-column: 1 / -1;
  }
  .type-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
    gap: 8px;
    width: 100%;
  }
  .type-card {
    text-align: left;
    border: 1px solid var(--el-border-color);
    border-radius: 8px;
    padding: 8px 10px;
    background: var(--el-bg-color);
    cursor: pointer;
  }
  .type-card.active {
    border-color: var(--el-color-primary);
    background: var(--el-color-primary-light-9);
  }
  .type-card p {
    margin: 4px 0 0;
    color: var(--el-text-color-secondary);
    font-size: 12px;
    line-height: 1.35;
  }
  .hint {
    margin: 2px 0 0;
    color: var(--el-text-color-secondary);
    font-size: 12px;
    line-height: 1.35;
  }
  .full {
    width: 100%;
  }
</style>
