import { defineStore } from 'pinia'
import { knowledgeApi, type KnowledgeDatabase, type KnowledgeTypeInfo } from '@/api/knowledge'
import { unwrapApiData, unwrapList } from '@/utils/apiData'

export const useKnowledgeStore = defineStore('knowledgeStore', () => {
  const databases = ref<KnowledgeDatabase[]>([])
  const types = ref<Record<string, KnowledgeTypeInfo>>({})
  const listLoading = ref(false)
  const listError = ref('')
  const typesLoading = ref(false)
  const currentDatabase = ref<KnowledgeDatabase | null>(null)
  const detailLoading = ref(false)
  let detailRequest: Promise<KnowledgeDatabase> | null = null
  let detailRequestKbId = ''
  let detailRequestVersion = 0

  const loadTypes = async () => {
    typesLoading.value = true
    try {
      const result: any = await knowledgeApi.getTypes()
      // Yuxi: { kb_types, message }
      types.value = result?.kb_types || unwrapApiData(result)?.kb_types || {}
    } finally {
      typesLoading.value = false
    }
  }

  const loadDatabases = async () => {
    listLoading.value = true
    listError.value = ''
    try {
      const result: any = await knowledgeApi.getDatabases()
      const list = result?.databases || unwrapList(result)
      databases.value = Array.isArray(list) ? list : []
      if (result?.message && !databases.value.length) {
        throw new Error(String(result.message))
      }
    } catch (error) {
      databases.value = []
      listError.value = (error as Error)?.message || '知识库服务连接失败'
      throw error
    } finally {
      listLoading.value = false
    }
  }

  const loadDatabase = (kbId: string) => {
    const normalizedKbId = String(kbId || '').trim()
    if (!normalizedKbId) return Promise.reject(new Error('缺少知识库 ID'))
    if (detailRequest && detailRequestKbId === normalizedKbId) return detailRequest

    const version = ++detailRequestVersion
    const currentId = String(currentDatabase.value?.kb_id || currentDatabase.value?.id || '')
    if (currentId !== normalizedKbId) {
      currentDatabase.value =
        databases.value.find(
          (item) => String(item?.kb_id || item?.id || '') === normalizedKbId
        ) || null
    }
    detailLoading.value = true
    detailRequestKbId = normalizedKbId

    const request = knowledgeApi
      .getDatabaseInfo(normalizedKbId)
      .then((result: any) => {
        const database = (result?.database ||
          (result?.kb_id ? result : result?.data || result)) as KnowledgeDatabase
        if (!database || typeof database !== 'object' || Array.isArray(database)) {
          throw new Error('知识库详情数据格式无效')
        }
        if (version === detailRequestVersion) currentDatabase.value = database
        return database
      })
      .finally(() => {
        if (detailRequest === request) {
          detailRequest = null
          detailRequestKbId = ''
          detailLoading.value = false
        }
      })

    detailRequest = request
    return request
  }

  const clearCurrent = () => {
    detailRequestVersion += 1
    currentDatabase.value = null
  }

  return {
    databases,
    types,
    listLoading,
    listError,
    typesLoading,
    currentDatabase,
    detailLoading,
    loadTypes,
    loadDatabases,
    loadDatabase,
    clearCurrent
  }
})
