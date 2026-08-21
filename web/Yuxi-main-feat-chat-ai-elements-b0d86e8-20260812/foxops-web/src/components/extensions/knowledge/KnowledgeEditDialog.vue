<template>
  <ElDialog
    :model-value="modelValue"
    width="640px"
    destroy-on-close
    :show-close="false"
    class="knowledge-edit-dialog extension-dialog"
    header-class="extension-dialog-header"
    body-class="extension-dialog-body"
    footer-class="extension-dialog-footer"
    @close="$emit('update:modelValue', false)"
  >
    <template #header>
      <ExtensionDialogHeader title="编辑知识库" @close="$emit('update:modelValue', false)" />
    </template>
    <ElForm label-position="top" class="compact-form">
      <ElFormItem label="知识库名称" required>
        <ElInput v-model="form.name" />
      </ElFormItem>
      <ElFormItem label="知识库描述">
        <ElInput v-model="form.description" type="textarea" :rows="2" />
      </ElFormItem>
      <ElFormItem v-if="!isConnector" label="自动生成问题">
        <ElSwitch v-model="form.auto_generate_questions" />
      </ElFormItem>
      <ElFormItem v-if="!isConnector" label="分块策略">
        <ElSelect v-model="form.chunk_preset_id" style="width: 100%">
          <ElOption
            v-for="item in CHUNK_PRESET_OPTIONS"
            :key="item.value"
            :label="item.label"
            :value="item.value"
          />
        </ElSelect>
      </ElFormItem>
      <template v-if="isDify">
        <ElFormItem label="Dify API URL">
          <ElInput v-model="form.dify_api_url" />
        </ElFormItem>
        <ElFormItem label="Dify Token">
          <ElInput v-model="form.dify_token" type="password" show-password />
        </ElFormItem>
        <ElFormItem label="Dataset ID">
          <ElInput v-model="form.dify_dataset_id" />
        </ElFormItem>
      </template>
      <template v-if="isNotion">
        <ElFormItem label="Notion Token">
          <ElInput v-model="form.notion_token" type="password" show-password />
        </ElFormItem>
        <ElFormItem label="Data Source ID">
          <ElInput v-model="form.notion_data_source_id" />
        </ElFormItem>
        <ElFormItem label="Notion API Version">
          <ElInput v-model="form.notion_version" />
        </ElFormItem>
      </template>
      <ElFormItem label="共享范围">
        <ShareScopeForm v-model="form.share_config" />
      </ElFormItem>
    </ElForm>
    <template #footer>
      <ElButton @click="$emit('update:modelValue', false)">取消</ElButton>
      <ElButton type="primary" :loading="saving" @click="save">保存</ElButton>
    </template>
  </ElDialog>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { knowledgeApi, type KnowledgeDatabase } from '@/api/knowledge'
  import ShareScopeForm, {
    type ShareConfig
  } from '@/components/extensions/common/ShareScopeForm.vue'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'
  import { CHUNK_PRESET_OPTIONS } from '@/utils/chunkPresets'

  const props = defineProps<{ modelValue: boolean; database?: KnowledgeDatabase | null }>()
  const emit = defineEmits<{ 'update:modelValue': [value: boolean]; saved: [] }>()

  const saving = ref(false)
  const form = reactive({
    name: '',
    description: '',
    auto_generate_questions: false,
    chunk_preset_id: 'general',
    dify_api_url: '',
    dify_token: '',
    dify_dataset_id: '',
    notion_token: '',
    notion_data_source_id: '',
    notion_version: '',
    share_config: {
      access_level: 'global',
      department_ids: [],
      user_uids: []
    } as ShareConfig
  })

  const kbType = computed(() => String(props.database?.kb_type || ''))
  const isDify = computed(() => kbType.value.toLowerCase().includes('dify'))
  const isNotion = computed(() => kbType.value.toLowerCase().includes('notion'))
  const isConnector = computed(() => isDify.value || isNotion.value)

  watch(
    () => [props.modelValue, props.database],
    () => {
      if (!props.modelValue || !props.database) return
      const db = props.database
      const extra = (db.additional_params || {}) as Record<string, any>
      form.name = db.name || ''
      form.description = db.description || ''
      form.auto_generate_questions = !!(db.auto_generate_questions ?? extra.auto_generate_questions)
      form.chunk_preset_id = extra.chunk_preset_id || 'general'
      form.dify_api_url = extra.dify_api_url || ''
      form.dify_token = extra.dify_token || ''
      form.dify_dataset_id = extra.dify_dataset_id || ''
      form.notion_token = extra.notion_token || ''
      form.notion_data_source_id = extra.notion_data_source_id || ''
      form.notion_version = extra.notion_version || ''
      form.share_config = (db.share_config as ShareConfig) || {
        access_level: 'global',
        department_ids: [],
        user_uids: []
      }
    },
    { immediate: true }
  )

  const save = async () => {
    if (!props.database?.kb_id) return
    if (!form.name.trim()) {
      ElMessage.warning('请填写名称')
      return
    }
    saving.value = true
    try {
      const additional_params: Record<string, unknown> = {
        ...(props.database.additional_params || {})
      }
      if (!isConnector.value) {
        additional_params.chunk_preset_id = form.chunk_preset_id
        additional_params.auto_generate_questions = form.auto_generate_questions
      }
      if (isDify.value) {
        additional_params.dify_api_url = form.dify_api_url
        additional_params.dify_token = form.dify_token
        additional_params.dify_dataset_id = form.dify_dataset_id
      }
      if (isNotion.value) {
        additional_params.notion_token = form.notion_token
        additional_params.notion_data_source_id = form.notion_data_source_id
        additional_params.notion_version = form.notion_version
      }
      await knowledgeApi.updateDatabase(props.database.kb_id, {
        name: form.name.trim(),
        description: form.description,
        auto_generate_questions: form.auto_generate_questions,
        additional_params,
        share_config: form.share_config
      })
      ElMessage.success('已保存')
      emit('saved')
      emit('update:modelValue', false)
    } catch (error) {
      ElMessage.error((error as Error)?.message || '保存失败')
    } finally {
      saving.value = false
    }
  }
</script>

<style scoped>
  .compact-form :deep(.el-form-item) {
    margin-bottom: 8px;
  }
  .compact-form :deep(.el-form-item__label) {
    margin-bottom: 2px !important;
    line-height: 1.2;
    height: auto;
  }
</style>
