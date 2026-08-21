import { ElMessage } from 'element-plus'

/**
 * 统一错误处理工具（搬自 Yuxi utils/errorHandler.js，antd message -> ElMessage）
 */
export class ErrorHandler {
  /**
   * 处理通用错误
   */
  static handleError(error, context = '操作', options = {}) {
    const {
      showMessage = true,
      logToConsole = true,
      customMessage = null,
      severity = 'error'
    } = options

    if (logToConsole) {
      console.error(`${context}失败:`, error)
    }

    if (showMessage) {
      const displayMessage = customMessage || this.getErrorMessage(error, context)

      switch (severity) {
        case 'warning':
          ElMessage.warning(displayMessage)
          break
        case 'info':
          ElMessage.info(displayMessage)
          break
        case 'error':
        default:
          ElMessage.error(displayMessage)
          break
      }
    }

    return error
  }

  /**
   * 获取错误消息
   */
  static getErrorMessage(error, context) {
    if (error?.message) {
      return `${context}失败: ${error.message}`
    }
    return `${context}失败`
  }

  /**
   * 处理网络请求错误
   */
  static handleNetworkError(error, context = '网络请求') {
    let customMessage = null

    if (error?.code === 'NETWORK_ERROR') {
      customMessage = '网络连接失败，请检查网络设置'
    } else if (error?.status === 401) {
      customMessage = '认证失败，请重新登录'
    } else if (error?.status === 403) {
      customMessage = '权限不足，无法执行此操作'
    } else if (error?.status === 404) {
      customMessage = '请求的资源不存在'
    } else if (error?.status >= 500) {
      customMessage = '服务器错误，请稍后重试'
    }

    return this.handleError(error, context, { customMessage })
  }

  /**
   * 处理聊天相关错误
   */
  static handleChatError(error, operation) {
    const contextMap = {
      send: '发送消息',
      create: '创建对话',
      delete: '删除对话',
      rename: '重命名对话',
      load: '加载对话',
      export: '导出对话',
      stream: '流式处理',
      fetch: '获取',
      save: '保存',
      initialize: '初始化',
      update: '更新'
    }

    const context = contextMap[operation] || operation
    return this.handleError(error, context)
  }

  /**
   * 处理验证错误
   */
  static handleValidationError(message) {
    return this.handleError(new Error(message), '输入验证', {
      severity: 'warning',
      customMessage: message
    })
  }

  /**
   * 处理异步操作错误
   */
  static async handleAsync(asyncFn, context, options = {}) {
    try {
      return await asyncFn()
    } catch (error) {
      this.handleError(error, context, options)
      throw error
    }
  }

  /**
   * 创建错误处理装饰器
   */
  static createHandler(context, options = {}) {
    return (error) => this.handleError(error, context, options)
  }
}

/** 快捷方法 */
export const handleChatError = ErrorHandler.handleChatError.bind(ErrorHandler)
export const handleNetworkError = ErrorHandler.handleNetworkError.bind(ErrorHandler)
export const handleValidationError = ErrorHandler.handleValidationError.bind(ErrorHandler)
export const handleAsync = ErrorHandler.handleAsync.bind(ErrorHandler)

export default ErrorHandler
