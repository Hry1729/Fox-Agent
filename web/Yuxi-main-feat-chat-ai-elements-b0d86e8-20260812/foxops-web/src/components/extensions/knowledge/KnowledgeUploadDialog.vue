<template>
  <ElDialog
    :model-value="modelValue"
    width="560px"
    destroy-on-close
    :show-close="false"
    class="extension-dialog"
    header-class="extension-dialog-header"
    body-class="extension-dialog-body"
    footer-class="extension-dialog-footer"
    @close="$emit('update:modelValue', false)"
  >
    <template #header>
      <ExtensionDialogHeader title="上传文件" @close="$emit('update:modelValue', false)" />
    </template>
    <ElUpload
      drag
      multiple
      :auto-upload="false"
      :file-list="fileList"
      :on-change="onChange"
      :on-remove="onRemove"
    >
      <div class="upload-tip">拖拽文件到此处，或点击选择</div>
    </ElUpload>
    <ElForm label-position="top" class="params">
      <ElFormItem label="分块策略">
        <ElSelect v-model="chunkPresetId" style="width: 100%">
          <ElOption
            v-for="item in CHUNK_PRESET_OPTIONS"
            :key="item.value"
            :label="item.label"
            :value="item.value"
          />
        </ElSelect>
      </ElFormItem>
    </ElForm>
    <template #footer>
      <ElButton @click="$emit('update:modelValue', false)">取消</ElButton>
      <ElButton type="primary" :loading="uploading" :disabled="!files.length" @click="submit">
        上传并添加
      </ElButton>
    </template>
  </ElDialog>
</template>

<script setup lang="ts">
  import type { UploadFile, UploadUserFile } from 'element-plus'
  import { ElMessage } from 'element-plus'
  import { knowledgeApi } from '@/api/knowledge'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'
  import { useTaskerStore } from '@/store/modules/tasker'
  import { CHUNK_PRESET_OPTIONS } from '@/utils/chunkPresets'

  const props = defineProps<{ modelValue: boolean; kbId: string }>()
  const emit = defineEmits<{ 'update:modelValue': [value: boolean]; uploaded: [] }>()

  const tasker = useTaskerStore()
  const uploading = ref(false)
  const files = ref<File[]>([])
  const fileList = ref<UploadUserFile[]>([])
  const chunkPresetId = ref('general')

  const onChange = (_file: UploadFile, list: UploadUserFile[]) => {
    fileList.value = list
    files.value = list.map((item) => item.raw).filter(Boolean) as File[]
  }

  const onRemove = (_file: UploadFile, list: UploadUserFile[]) => {
    fileList.value = list
    files.value = list.map((item) => item.raw).filter(Boolean) as File[]
  }

  const submit = async () => {
    uploading.value = true
    try {
      const items: unknown[] = []
      for (const file of files.value) {
        const uploaded: any = await knowledgeApi.uploadFile(file, props.kbId)
        items.push(uploaded?.url || uploaded?.file_url || uploaded)
      }
      const result: any = await knowledgeApi.addUploadedDocuments(props.kbId, items, {
        chunk_preset_id: chunkPresetId.value
      })
      const taskId = result?.task_id || result?.id
      if (taskId) await tasker.trackTask(String(taskId))
      ElMessage.success('上传成功')
      emit('uploaded')
      emit('update:modelValue', false)
      files.value = []
      fileList.value = []
    } catch (error) {
      ElMessage.error((error as Error)?.message || '上传失败')
    } finally {
      uploading.value = false
    }
  }
</script>

<style scoped>
  .upload-tip {
    padding: 24px 0;
    color: var(--el-text-color-secondary);
  }
  .params {
    margin-top: 12px;
  }
</style>
