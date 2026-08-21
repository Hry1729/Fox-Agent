<template>
  <Sources v-if="hasSources" class="refs chat-ai-elements">
    <SourcesTrigger :count="sourceCount">
      <ArtSvgIcon icon="ri:book-open-line" class="btn-icon" />
      <span class="btn-label">来源 {{ sourceCount }}</span>
    </SourcesTrigger>

    <SourcesContent class="sources-panel">
      <!-- 知识库来源（按文件分组） -->
      <ElCollapse v-if="knowledgeChunks.length" v-model="activeFiles">
        <ElCollapseItem
          v-for="group in fileGroupList"
          :key="group.filename"
          :name="group.filename"
        >
          <template #title>
            <span class="file-title">
              <ArtSvgIcon icon="ri:file-text-line" class="file-icon" />
              <span class="file-name">{{ group.filename }}</span>
              <span class="chunk-count">{{ group.chunks.length }} chunks</span>
            </span>
          </template>

          <div
            v-for="(chunk, index) in group.chunks"
            :key="chunkKey(chunk, index)"
            class="chunk-item"
            :class="{ 'high-relevance': typeof chunk.score === 'number' && chunk.score > 0.5 }"
          >
            <div class="chunk-meta">
              <span class="chunk-index">#{{ index + 1 }}</span>
              <span v-if="typeof chunk.score === 'number'" class="score-item">
                相似度 {{ (chunk.score * 100).toFixed(0) }}%
              </span>
            </div>
            <div class="chunk-preview">{{ previewText(chunk.content) }}</div>
          </div>
        </ElCollapseItem>
      </ElCollapse>

      <!-- Web 搜索来源 -->
      <div v-if="webSources.length" class="web-sources">
        <div class="web-sources-title">
          <ArtSvgIcon icon="ri:global-line" class="web-icon" />
          <span>网络搜索</span>
        </div>
        <Source
          v-for="(source, index) in webSources"
          :key="`web-${index}`"
          :href="source.url"
          :title="source.title"
          class="web-source-item"
        >
          <div class="web-source-meta">
            <span class="web-source-index">#{{ index + 1 }}</span>
            <span v-if="typeof source.score === 'number'" class="score-item">
              {{ (source.score * 100).toFixed(0) }}%
            </span>
          </div>
          <div class="web-source-title">{{ source.title }}</div>
          <div class="web-source-url">{{ source.url }}</div>
        </Source>
      </div>
    </SourcesContent>
  </Sources>
</template>

<script setup>
import { computed, ref } from 'vue'
import { Source, Sources, SourcesContent, SourcesTrigger } from '@/components/ai-elements/sources'

const props = defineProps({
  sources: {
    type: Object,
    default: () => ({})
  }
})

const activeFiles = ref([])

const resolveChunks = (input) => {
  if (Array.isArray(input)) return input
  if (input && typeof input === 'object') {
    if (Array.isArray(input.chunks)) return input.chunks
    if (Array.isArray(input.data?.chunks)) return input.data.chunks
  }
  return []
}

const knowledgeChunks = computed(() => {
  const raw = Array.isArray(props.sources?.knowledgeChunks)
    ? props.sources.knowledgeChunks
    : resolveChunks(props.sources)

  return raw
    .filter((item) => item && typeof item === 'object' && item.content)
    .map((item) => {
      const metadata = item.metadata && typeof item.metadata === 'object' ? item.metadata : {}
      const source =
        metadata.source ||
        metadata.file_name ||
        metadata.filename ||
        metadata.title ||
        item.file_name ||
        item.filename ||
        '未知来源'
      return {
        ...item,
        score: typeof item.score === 'number' ? item.score : metadata.score,
        metadata: { ...metadata, source }
      }
    })
})

const webSources = computed(() => {
  const raw = Array.isArray(props.sources?.webSources) ? props.sources.webSources : []
  return raw.filter((item) => item && item.url && item.title)
})

const hasSources = computed(() => knowledgeChunks.value.length > 0 || webSources.value.length > 0)
const sourceCount = computed(() => knowledgeChunks.value.length + webSources.value.length)

const fileGroupList = computed(() => {
  const groups = new Map()
  for (const chunk of knowledgeChunks.value) {
    const filename = chunk.metadata.source
    if (!groups.has(filename)) {
      groups.set(filename, { filename, chunks: [] })
    }
    groups.get(filename).chunks.push(chunk)
  }
  return Array.from(groups.values()).sort((a, b) => a.filename.localeCompare(b.filename))
})

const chunkKey = (chunk, index) => {
  return `${chunk.metadata.source}-${chunk.metadata.chunk_id || index}`
}

const previewText = (text = '') => {
  const content = String(text)
  return content.length <= 100 ? content : `${content.substring(0, 100)}...`
}
</script>

<style lang="less" scoped>
.refs {
  margin-top: 8px;

  .btn-icon {
    font-size: 14px;
  }

  .btn-label {
    font-size: 13px;
    font-weight: 500;
  }

  .sources-panel {
    width: 100%;
    max-width: 100%;
    background: var(--art-gray-100);
    border: 1px solid var(--default-border);
    border-radius: 8px;
    padding: 4px 12px;

    .web-sources {
      margin-top: 8px;
      padding-top: 8px;
      border-top: 1px solid var(--default-border);
    }
    .web-sources-title {
      display: flex;
      align-items: center;
      gap: 4px;
      font-size: 12px;
      font-weight: 600;
      color: var(--art-gray-600);
      margin-bottom: 6px;
    }
    .web-icon {
      font-size: 14px;
    }
    .web-source-item {
      display: block;
      padding: 6px 4px;
      border-bottom: 1px solid var(--default-border);
      text-decoration: none;
      color: inherit;
      transition: background 0.15s;
      &:last-child {
        border-bottom: none;
      }
      &:hover {
        background: var(--art-gray-200);
      }
    }
    .web-source-meta {
      display: flex;
      align-items: center;
      gap: 6px;
      margin-bottom: 2px;
    }
    .web-source-index {
      font-size: 11px;
      color: var(--art-gray-600);
      background: var(--art-gray-200);
      border-radius: 4px;
      padding: 1px 5px;
      min-width: 24px;
      text-align: center;
    }
    .web-source-title {
      font-size: 13px;
      color: var(--art-gray-800);
      font-weight: 500;
      line-height: 1.4;
    }
    .web-source-url {
      font-size: 11px;
      color: var(--art-gray-500);
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }

    :deep(.el-collapse) {
      border: none;
    }

    :deep(.el-collapse-item__header) {
      background: transparent;
      border-bottom: 1px solid var(--default-border);
      height: 36px;
      font-size: 13px;
    }

    :deep(.el-collapse-item__wrap) {
      background: transparent;
      border-bottom: none;
    }

    :deep(.el-collapse-item__content) {
      padding: 0;
    }

    .file-title {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      min-width: 0;
      flex: 1;

      .file-icon {
        font-size: 14px;
        color: var(--art-gray-500);
        flex-shrink: 0;
      }

      .file-name {
        color: var(--art-gray-700);
        font-size: 13px;
        white-space: nowrap;
        overflow: hidden;
        text-overflow: ellipsis;
      }

      .chunk-count {
        font-size: 11px;
        color: var(--art-gray-500);
        flex-shrink: 0;
      }
    }

    .chunk-item {
      padding: 6px 4px;
      border-bottom: 1px solid var(--default-border);

      &:last-child {
        border-bottom: none;
      }

      &.high-relevance {
        background: var(--art-gray-200);
      }

      .chunk-meta {
        display: flex;
        align-items: center;
        gap: 6px;
        margin-bottom: 4px;

        .chunk-index {
          font-size: 11px;
          color: var(--art-gray-600);
          background: var(--art-gray-200);
          border-radius: 4px;
          padding: 1px 5px;
          min-width: 24px;
          text-align: center;
        }

        .score-item {
          font-size: 11px;
          color: var(--art-gray-600);
          background: var(--art-gray-200);
          border-radius: 4px;
          padding: 1px 5px;
          white-space: nowrap;
        }
      }

      .chunk-preview {
        font-size: 12px;
        color: var(--art-gray-700);
        line-height: 1.5;
        display: -webkit-box;
        -webkit-line-clamp: 2;
        line-clamp: 2;
        -webkit-box-orient: vertical;
        overflow: hidden;
      }
    }
  }
}
</style>
