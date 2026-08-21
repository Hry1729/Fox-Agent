<template>
  <component
    :is="currentRenderer"
    v-if="currentRenderer"
    :tool-call="toolCall"
    :appearance="appearance"
    :default-expanded="defaultExpanded"
  />
  <BaseToolCall
    v-else-if="!isHidden"
    :tool-call="toolCall"
    :appearance="appearance"
    :default-expanded="defaultExpanded"
  />
</template>

<script setup>
  import { computed } from 'vue'
  import BaseToolCall from './BaseToolCall.vue'
  import { getToolCallId, isHiddenToolCall } from './toolRegistry'

  // 仅注册运行时无副作用的、依赖最小（仅 vue + BaseToolCall）的渲染器。
  // 缺失依赖的渲染器统一回退到 BaseToolCall。
  import CalculatorTool from './tools/CalculatorTool.vue'
  import ChartTool from './tools/ChartTool.vue'
  import EditFileTool from './tools/EditFileTool.vue'
  import ExecuteTool from './tools/ExecuteTool.vue'
  import GetMindmapTool from './tools/GetMindmapTool.vue'
  import GlobTool from './tools/GlobTool.vue'
  import GrepTool from './tools/GrepTool.vue'
  import ListDirectoryTool from './tools/ListDirectoryTool.vue'
  import ListKbsTool from './tools/ListKbsTool.vue'
  import MysqlDescribeTableTool from './tools/MysqlDescribeTableTool.vue'
  import MysqlListTablesTool from './tools/MysqlListTablesTool.vue'
  import MysqlQueryTool from './tools/MysqlQueryTool.vue'
  import ReadFileTool from './tools/ReadFileTool.vue'
  import SearchFileContentTool from './tools/SearchFileContentTool.vue'
  import WriteFileTool from './tools/WriteFileTool.vue'

  const props = defineProps({
    toolCall: {
      type: Object,
      required: true
    },
    appearance: {
      type: String,
      default: 'card'
    },
    defaultExpanded: {
      type: Boolean,
      default: false
    }
  })

  const toolId = computed(() => getToolCallId(props.toolCall))

  const TOOL_RENDERERS = {
    bash: ExecuteTool,
    calculator: CalculatorTool,
    chart: ChartTool,
    cmd: ExecuteTool,
    edit_file: EditFileTool,
    execute: ExecuteTool,
    get_mindmap: GetMindmapTool,
    glob: GlobTool,
    grep: GrepTool,
    list_directory: ListDirectoryTool,
    list_kbs: ListKbsTool,
    ls: ListDirectoryTool,
    mysql_describe_table: MysqlDescribeTableTool,
    mysql_list_tables: MysqlListTablesTool,
    mysql_query: MysqlQueryTool,
    read_file: ReadFileTool,
    replace: EditFileTool,
    run_shell_command: ExecuteTool,
    search_file_content: SearchFileContentTool,
    write_file: WriteFileTool
  }

  const currentRenderer = computed(() => TOOL_RENDERERS[toolId.value] || null)
  const isHidden = computed(() => isHiddenToolCall(props.toolCall))
</script>
