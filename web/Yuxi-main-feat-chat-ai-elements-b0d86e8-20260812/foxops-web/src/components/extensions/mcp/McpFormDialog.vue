<template>
  <ElDialog
    :model-value="modelValue"
    width="820px"
    top="5vh"
    :close-on-click-modal="false"
    destroy-on-close
    :show-close="false"
    class="mcp-form-dialog extension-dialog"
    header-class="extension-dialog-header"
    body-class="extension-dialog-body"
    footer-class="extension-dialog-footer"
    @close="$emit('update:modelValue', false)"
  >
    <template #header>
      <ExtensionDialogHeader :title="dialogTitle" @close="$emit('update:modelValue', false)" />
    </template>

    <ElForm label-position="top" class="mcp-form-grid">
      <ElFormItem label="MCP 标识" required>
        <ElInput v-model="form.slug" :disabled="editMode" placeholder="如 my-mcp" />
      </ElFormItem>
      <ElFormItem label="MCP 名称" required>
        <ElInput v-model="form.name" placeholder="展示名称" />
      </ElFormItem>

      <ElFormItem label="描述" class="span-2">
        <ElInput v-model="form.description" placeholder="描述" />
      </ElFormItem>

      <ElFormItem label="传输类型" required>
        <ElSelect v-model="form.transport" style="width: 100%">
          <ElOption value="streamable_http" label="streamable_http" />
          <ElOption value="sse" label="sse" />
          <ElOption value="stdio" label="stdio" />
        </ElSelect>
      </ElFormItem>
      <ElFormItem label="图标">
        <ElInput v-model="form.icon" maxlength="2" placeholder="emoji" />
      </ElFormItem>

      <template v-if="form.transport === 'streamable_http' || form.transport === 'sse'">
        <ElFormItem label="MCP URL" required class="span-2">
          <ElInput v-model="form.url" placeholder="https://example.com/mcp" />
        </ElFormItem>
        <ElFormItem label="HTTP 请求头" class="span-2">
          <ElInput
            v-model="form.headersText"
            type="textarea"
            :rows="2"
            placeholder='JSON，如 {"Authorization":"Bearer xxx"}'
          />
        </ElFormItem>
        <div class="mcp-timeout-tags-row span-2">
          <ElFormItem label="HTTP 超时（秒）">
            <ElInputNumber v-model="form.timeout" :min="1" :max="300" style="width: 100%" />
          </ElFormItem>
          <ElFormItem label="SSE 读取超时（秒）">
            <ElInputNumber v-model="form.sse_read_timeout" :min="1" :max="300" style="width: 100%" />
          </ElFormItem>
          <ElFormItem label="标签">
            <ElSelect
              v-model="form.tags"
              multiple
              filterable
              allow-create
              default-first-option
              style="width: 100%"
              placeholder="输入后回车"
            />
          </ElFormItem>
        </div>
      </template>

      <template v-if="form.transport === 'stdio'">
        <ElFormItem label="命令" required>
          <ElInput v-model="form.command" placeholder="npx 或可执行路径" />
        </ElFormItem>
        <ElFormItem label="参数">
          <ElSelect
            v-model="form.args"
            multiple
            filterable
            allow-create
            default-first-option
            style="width: 100%"
            placeholder="输入后回车"
          />
        </ElFormItem>
        <ElFormItem label="环境变量" class="span-2">
          <McpEnvEditor v-model="form.env" />
        </ElFormItem>
      </template>

      <ElFormItem v-if="form.transport === 'stdio'" label="标签" class="span-2">
        <ElSelect
          v-model="form.tags"
          multiple
          filterable
          allow-create
          default-first-option
          style="width: 100%"
          placeholder="输入后回车"
        />
      </ElFormItem>
    </ElForm>

    <template #footer>
      <ElButton @click="$emit('update:modelValue', false)">取消</ElButton>
      <ElButton type="primary" :loading="submitting" @click="submit">保存</ElButton>
    </template>
  </ElDialog>
</template>

<script setup lang="ts">
  import { ElMessage } from 'element-plus'
  import { mcpApi, type McpServer } from '@/api/mcp'
  import ExtensionDialogHeader from '@/components/extensions/common/ExtensionDialogHeader.vue'
  import McpEnvEditor from './McpEnvEditor.vue'

  const props = withDefaults(
    defineProps<{ modelValue: boolean; editMode?: boolean; editData?: McpServer | null }>(),
    { editMode: false, editData: null }
  )
  const emit = defineEmits<{ 'update:modelValue': [value: boolean]; submitted: [] }>()

  const dialogTitle = computed(() => (props.editMode ? '编辑 MCP' : '添加 MCP'))

  const submitting = ref(false)
  const form = reactive({
    slug: '',
    name: '',
    description: '',
    transport: 'streamable_http' as string,
    url: '',
    command: '',
    args: [] as string[],
    env: null as Record<string, string> | null,
    headersText: '',
    icon: '',
    tags: [] as string[],
    timeout: 30,
    sse_read_timeout: 60
  })

  const resetForm = () => {
    form.slug = ''
    form.name = ''
    form.description = ''
    form.transport = 'streamable_http'
    form.url = ''
    form.command = ''
    form.args = []
    form.env = null
    form.headersText = ''
    form.icon = ''
    form.tags = []
    form.timeout = 30
    form.sse_read_timeout = 60
  }

  const fillFromEdit = () => {
    if (!props.editData) return
    const data = props.editData
    form.slug = data.slug || ''
    form.name = data.name || ''
    form.description = data.description || ''
    form.transport = data.transport || 'streamable_http'
    form.url = data.url || ''
    form.command = data.command || ''
    form.args = [...(data.args || [])]
    form.env = data.env ? { ...data.env } : null
    form.icon = data.icon || ''
    form.tags = [...(data.tags || [])]
    form.timeout = Number(data.http_timeout || 30)
    form.sse_read_timeout = Number(data.sse_read_timeout || 60)
    if (typeof data.headers === 'string') form.headersText = data.headers
    else if (data.headers) form.headersText = JSON.stringify(data.headers, null, 2)
    else form.headersText = ''
  }

  watch(
    () => [props.modelValue, props.editData, props.editMode] as const,
    ([open]) => {
      if (!open) return
      if (props.editMode) fillFromEdit()
      else resetForm()
    }
  )

  const submit = async () => {
    if (!form.slug.trim() || !form.name.trim()) {
      ElMessage.warning('请填写标识和名称')
      return
    }
    let headers: Record<string, string> | undefined
    if (form.headersText.trim()) {
      try {
        const parsed = JSON.parse(form.headersText)
        if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
          ElMessage.warning('请求头必须是 JSON 对象')
          return
        }
        headers = parsed
      } catch {
        ElMessage.warning('请求头 JSON 格式不正确')
        return
      }
    }
    if (
      (form.transport === 'streamable_http' || form.transport === 'sse') &&
      !String(form.url || '').trim()
    ) {
      ElMessage.warning('请填写 MCP URL')
      return
    }
    if (form.transport === 'stdio' && !String(form.command || '').trim()) {
      ElMessage.warning('请填写启动命令')
      return
    }

    const payload: Record<string, unknown> = {
      slug: form.slug.trim(),
      name: form.name.trim(),
      description: form.description,
      transport: form.transport,
      icon: form.icon,
      tags: form.tags,
      url: form.url,
      headers,
      timeout: form.timeout,
      sse_read_timeout: form.sse_read_timeout,
      command: form.command,
      args: form.args,
      env: form.env
    }

    submitting.value = true
    try {
      if (props.editMode) {
        await mcpApi.updateServer(form.slug, payload)
        ElMessage.success('MCP 已更新')
      } else {
        await mcpApi.createServer(payload)
        ElMessage.success('MCP 已创建')
      }
      emit('update:modelValue', false)
      emit('submitted')
    } catch (error) {
      ElMessage.error((error as Error)?.message || '保存失败')
    } finally {
      submitting.value = false
    }
  }
</script>

<style scoped>
  .mcp-form-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    column-gap: 14px;
    row-gap: 0;
  }
  .mcp-form-grid :deep(.el-form-item) {
    margin-bottom: 8px;
  }
  .mcp-form-grid :deep(.el-form-item__label) {
    margin-bottom: 2px !important;
    line-height: 1.2;
    height: auto;
  }
  .span-2 {
    grid-column: 1 / -1;
  }
  .mcp-timeout-tags-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) minmax(0, 1.7fr);
    column-gap: 14px;
  }
  .mcp-timeout-tags-row :deep(.el-form-item) {
    margin-bottom: 8px;
  }
</style>
