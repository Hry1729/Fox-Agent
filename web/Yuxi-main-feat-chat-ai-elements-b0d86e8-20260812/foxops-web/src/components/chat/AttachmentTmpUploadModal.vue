<template>
  <ElDialog
    :model-value="open"
    title="添加附件"
    width="560px"
    :close-on-click-modal="false"
    @update:model-value="val => $emit('update:open', val)"
  >
    <ElUpload
      drag
      multiple
      :show-file-list="false"
      :before-upload="handleBeforeUpload"
      :disabled="confirming"
      class="attachment-dropzone"
    >
      <ArtSvgIcon icon="ri:upload-cloud-2-line" class="text-3xl text-g-400" />
      <p class="dropzone-title">点击或拖拽文件到此处上传</p>
      <p class="dropzone-desc">支持任意文件格式 ≤ 5 MB；PDF 和图片可选解析为 Markdown。</p>
    </ElUpload>

    <div v-if="fileItems.length" class="attachment-list">
      <div v-for="item in fileItems" :key="item.localId" class="attachment-item">
        <div class="attachment-item-content">
          <div class="attachment-name-row">
            <ArtSvgIcon icon="ri:file-3-line" class="text-base text-g-500" />
            <span class="attachment-name" :title="item.fileName">{{ item.fileName }}</span>
            <span class="attachment-size">{{ formatFileSize(item.fileSize) }}</span>
            <ElButton size="small" text :disabled="confirming" @click="removeItem(item.localId)">
              <ArtSvgIcon icon="ri:close-line" />
            </ElButton>
          </div>
          <div class="attachment-status-row">
            <ElTag :type="getStatusType(item.status)" size="small" effect="plain">
              {{ getStatusLabel(item.status) }}
            </ElTag>
            <span v-if="item.error" class="attachment-error">{{ item.error }}</span>
            <span v-else-if="item.parseError" class="attachment-error">{{ item.parseError }}</span>
          </div>
          <!-- 解析选项（仅 PDF/图片） -->
          <div v-if="item.parseSupported && item.status !== 'uploading' && item.status !== 'error'" class="parse-section">
            <ElSelect
              v-model="item.selectedParseMethod"
              placeholder="选择解析方式"
              size="small"
              :disabled="item.status === 'parsing' || confirming"
              @change="handleParseMethodChange(item.localId, $event)"
            >
              <ElOption
                v-for="method in item.parseMethods"
                :key="method"
                :label="getMethodLabel(method)"
                :value="method"
              />
            </ElSelect>
            <ElButton
              v-if="item.selectedParseMethod && item.status !== 'parsing' && item.status !== 'parsed'"
              size="small"
              type="primary"
              plain
              :disabled="confirming"
              @click="handleStartParse(item.localId)"
            >解析</ElButton>
            <span v-if="item.status === 'parsing'" class="text-xs text-g-500">解析中...</span>
            <span v-if="item.status === 'parsed'" class="text-xs text-success">已解析</span>
          </div>
        </div>
      </div>
    </div>

    <template #footer>
      <ElButton @click="handleCancel">取消</ElButton>
      <ElButton type="primary" :loading="confirming" :disabled="confirmDisabled" @click="handleConfirm">
        添加附件
      </ElButton>
    </template>
  </ElDialog>
</template>

<script setup>
import { ref, computed, watch } from 'vue'
import { ElMessage } from 'element-plus'
import { threadApi } from '@/api/thread'

const MAX_FILE_SIZE = 5 * 1024 * 1024 // 5MB

const props = defineProps({
  open: { type: Boolean, default: false },
  threadId: { type: String, default: '' },
  ensureThread: { type: Function, default: null }
})

const emit = defineEmits(['update:open', 'added'])

const fileItems = ref([])
const confirming = ref(false)
let localIdSeed = 0

const busy = computed(() =>
  fileItems.value.some((item) => ['uploading', 'parsing'].includes(item.status))
)
const confirmableItems = computed(() =>
  fileItems.value.filter((item) => ['uploaded', 'parsed'].includes(item.status))
)
const confirmDisabled = computed(() => busy.value || confirmableItems.value.length === 0)

watch(
  () => props.open,
  (open) => {
    if (!open) {
      fileItems.value = []
      confirming.value = false
    }
  }
)

const getErrorMessage = (error, fallback = '操作失败') =>
  error?.response?.data?.detail || error?.message || fallback

const formatFileSize = (size) => {
  if (!Number.isFinite(size)) return '未知大小'
  if (size < 1024) return `${size} B`
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KB`
  return `${(size / 1024 / 1024).toFixed(1)} MB`
}

const getStatusType = (status) => {
  const map = { uploading: 'info', uploaded: '', parsing: 'info', parsed: 'success', error: 'danger' }
  return map[status] || 'info'
}
const getStatusLabel = (status) => {
  const map = { uploading: '上传中', uploaded: '已上传', parsing: '解析中', parsed: '已解析', error: '失败' }
  return map[status] || status
}
const getMethodLabel = (method) => {
  const map = { disable: '不解析（使用文本层）', mineru: 'MinerU', paddle: 'PaddleOCR', rapidocr: 'RapidOCR' }
  return map[method] || method
}

const normalizeTmpUpload = (response) => ({
  tmpFileId: response.tmp_file_id,
  fileName: response.file_name,
  fileType: response.file_type,
  fileSize: response.file_size,
  bucketName: response.bucket_name,
  objectName: response.object_name,
  minioUrl: response.minio_url,
  parseSupported: response.parse_supported,
  parseMethods: response.parse_methods || [],
  selectedParseMethod: null
})

const updateItem = (localId, patch) => {
  fileItems.value = fileItems.value.map((item) =>
    item.localId === localId ? { ...item, ...patch } : item
  )
}

const uploadFile = async (file) => {
  const localId = `${Date.now()}-${localIdSeed++}`

  // 5MB 限制
  if (file.size > MAX_FILE_SIZE) {
    ElMessage.error(`${file.name} 超过 5MB 限制`)
    return
  }

  fileItems.value.push({
    localId,
    fileName: file.name,
    fileSize: file.size,
    status: 'uploading',
    error: null,
    parseError: null,
    parseSupported: false,
    parseMethods: [],
    selectedParseMethod: null
  })

  try {
    const response = await threadApi.uploadTmpAttachment(file)
    const normalized = normalizeTmpUpload(response)
    updateItem(localId, { ...normalized, status: 'uploaded' })
  } catch (error) {
    updateItem(localId, { status: 'error', error: getErrorMessage(error, '上传失败') })
  }
}

const handleBeforeUpload = (file) => {
  void uploadFile(file)
  return false // 阻止 ElUpload 自动上传
}

const handleParseMethodChange = (localId, method) => {
  updateItem(localId, {
    parsedObjectName: null,
    parsedMinioUrl: null,
    truncated: false,
    parseMethod: null,
    parseError: null,
    status: 'uploaded'
  })
}

const handleStartParse = async (localId) => {
  const item = fileItems.value.find((e) => e.localId === localId)
  if (!item || !item.objectName || !item.selectedParseMethod) return

  updateItem(localId, { status: 'parsing', parseError: null })
  try {
    const response = await threadApi.parseTmpAttachment({
      object_name: item.objectName,
      file_name: item.fileName,
      bucket_name: item.bucketName,
      parse_method: item.selectedParseMethod
    })
    updateItem(localId, {
      status: 'parsed',
      parsedObjectName: response.parsed_object_name,
      parsedMinioUrl: response.parsed_minio_url,
      truncated: response.truncated,
      parseMethod: response.parse_method
    })
    ElMessage.success('附件解析完成')
  } catch (error) {
    updateItem(localId, {
      status: 'uploaded',
      parseError: getErrorMessage(error, '解析失败')
    })
  }
}

const removeItem = (localId) => {
  fileItems.value = fileItems.value.filter((item) => item.localId !== localId)
}

const handleConfirm = async () => {
  if (confirmDisabled.value) return

  const attachments = confirmableItems.value.map((item) => ({
    file_name: item.fileName,
    file_type: item.fileType,
    bucket_name: item.bucketName,
    object_name: item.objectName,
    parsed_object_name: item.parsedObjectName || null,
    truncated: Boolean(item.truncated)
  }))

  confirming.value = true
  try {
    const threadId = props.threadId || (props.ensureThread ? await props.ensureThread() : '')
    if (!threadId) {
      ElMessage.error('创建对话失败，无法添加附件')
      return
    }

    const response = await threadApi.confirmTmpThreadAttachments(threadId, attachments)
    ElMessage.success('附件已添加')
    emit('added', response)
    emit('update:open', false)
  } catch (error) {
    ElMessage.error(getErrorMessage(error, '添加附件失败'))
  } finally {
    confirming.value = false
  }
}

const handleCancel = () => {
  emit('update:open', false)
}
</script>

<style scoped>
.attachment-dropzone {
  margin-bottom: 12px;
}
.attachment-dropzone :deep(.el-upload-dragger) {
  padding: 20px;
  text-align: center;
}
.dropzone-title {
  margin: 8px 0 4px;
  font-size: 14px;
  font-weight: 600;
  color: var(--art-gray-800);
}
.dropzone-desc {
  margin: 0;
  font-size: 12px;
  color: var(--art-gray-500);
}
.attachment-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  max-height: 300px;
  overflow-y: auto;
}
.attachment-item {
  padding: 8px 12px;
  border: 1px solid var(--art-gray-200);
  border-radius: 8px;
  background: var(--art-gray-50);
}
.attachment-name-row {
  display: flex;
  align-items: center;
  gap: 6px;
}
.attachment-name {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: 13px;
  font-weight: 500;
}
.attachment-size {
  font-size: 11px;
  color: var(--art-gray-400);
  flex-shrink: 0;
}
.attachment-status-row {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 4px;
}
.attachment-error {
  font-size: 11px;
  color: var(--el-color-danger);
}
.parse-section {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 6px;
}
</style>
