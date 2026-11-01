import { describe, expect, test } from 'bun:test'
import { sendNotStartedNotice } from '../src/features/chat/send-failure-notice'

describe('sendNotStartedNotice', () => {
  test('模型连接失败进模型服务，上一轮忙碌和项目文件夹各有一句人话', () => {
    // 13.37 P0-1: Fox without a model service uses the Host's own sentence.
    const connection = sendNotStartedNotice('model.connection_required', '需要连接', { developerUi: false })
    expect(connection.title).toBe('还没有配置模型服务')
    expect(connection.description).toBe('请先到「设置 → 模型服务」添加并测试一个供应商，再发送。')
    expect(connection.workspaceView).toBe('model-service')
    expect(connection.mergeKey).toBe('send-not-started:model.connection_required')
    expect(connection.description).not.toContain('model.connection_required')
    expect(connection.description).not.toContain('需要连接')

    const failed = sendNotStartedNotice('model.connection_failed', '超时', { developerUi: false })
    expect(failed.title).toBe('还没有可用的模型连接')
    expect(failed.workspaceView).toBe('model-service')

    const busy = sendNotStartedNotice('agent_backend_turn_busy', 'busy', { developerUi: false })
    expect(busy.title).toBe('上一轮还没结束')
    expect(busy.description).toContain('等它完成')
    expect(busy.description).not.toContain('agent_backend_turn_busy')

    const project = sendNotStartedNotice('agent_backend_project_required', 'raw', { developerUi: false })
    expect(project.title).toBe('项目文件夹不可访问')
    expect(project.workspaceView).toBe('settings-projects')

    const attachments = sendNotStartedNotice('agent_backend_attachments_unsupported', 'raw', { developerUi: false })
    expect(attachments.title).toBe('这个 Agent 不能带附件')
    expect(attachments.description).not.toContain('agent_backend_attachments_unsupported')

    const created = sendNotStartedNotice('run.create_failed', '应为目录但是文件', { developerUi: false })
    expect(created.title).toBe('发送未开始')
    expect(created.description).toBe('消息没有发出去，草稿已保留。')
    expect(created.description).not.toContain('run.create_failed')
  })

  test('UI 1103：外部 Agent 没配好只弹一条通知，带后端名，按钮去 设置 → Agent', () => {
    const named = sendNotStartedNotice('agent_backend_missing_credentials', '未登录', { backend: 'deepseek', conversationId: 'c-1', developerUi: false })
    expect(named.title).toBe('dsh 还没配置好')
    expect(named.workspaceView).toBe('settings-ai')
    expect(named.description).not.toContain('agent_backend_missing_credentials')
    expect(named.description).not.toContain('登录卡片')

    const model = sendNotStartedNotice('model.connection_required', '请先到「设置 → 模型服务」配置', { backend: 'codex', developerUi: false })
    expect(model.title).toBe('Codex 还没配置好')
    expect(model.workspaceView).toBe('settings-ai')

    const installed = sendNotStartedNotice('agent_backend_not_installed', '没有安装', { backend: 'pi', developerUi: false })
    expect(installed.title).toBe('pi 还没安装')
    expect(installed.workspaceView).toBe('settings-ai')
    expect(installed.entityId).toBeUndefined()

    const source = sendNotStartedNotice('agent_backend_pi_source_choice_required', 'raw', { backend: 'pi', developerUi: false })
    expect(source.title).toBe('Pi 的配置来源需要确认')
    expect(source.workspaceView).toBe('settings-ai')

    const disabled = sendNotStartedNotice('agent_backend_user_disabled', '该 Agent 已停用，可在设置中启用', { backend: 'deepseek', developerUi: false })
    expect(disabled.title).toBe('dsh 已停用')
    expect(disabled.description).toBe('该 Agent 已停用，可在设置中启用。')
    expect(disabled.workspaceView).toBe('settings-ai')

    // 多地址：用 Host 的那句人话（「用作 X 默认协议」），按钮去模型服务。
    const choose = sendNotStartedNotice('model.connection_choose_address', '当前供应商有多个地址都能给 DeepSeek 用：请到「设置 → 模型服务」，在要用的地址上打开「用作 DeepSeek 默认协议」后再发送', { backend: 'deepseek', developerUi: false })
    expect(choose.title).toBe('模型服务有多个地址')
    expect(choose.description).toContain('用作 DeepSeek 默认协议')
    expect(choose.workspaceView).toBe('model-service')
  })

  test('e2e-1101 的新错误码各有一句人话', () => {
    const busy = sendNotStartedNotice('run.conversation_busy', 'raw', { developerUi: false })
    expect(busy.title).toBe('上一条还在处理')
    expect(busy.description).toContain('等它结束')
    expect(busy.description).not.toContain('run.conversation_busy')
    const gone = sendNotStartedNotice('conversation.not_found', 'raw', { developerUi: false })
    expect(gone.title).toBe('这个对话已不存在')
    expect(gone.description).toContain('新建')
    // Fox 未配模型时 Host 在建 Run 前就拒绝：去模型服务。
    const fox = sendNotStartedNotice('model.connection_required', 'raw', { backend: 'fox', developerUi: false })
    expect(fox.title).toBe('还没有配置模型服务')
    expect(fox.workspaceView).toBe('model-service')
    expect(fox.description).toContain('设置 → 模型服务')
  })

  test('developerUi 为 false 时详情不含错误码，为 true 时含错误码和原始信息', () => {
    const hidden = sendNotStartedNotice('model.connection_required', 'provider missing', { developerUi: false })
    expect(hidden.description).not.toContain('model.connection_required')
    expect(hidden.description).not.toContain('provider missing')

    const shown = sendNotStartedNotice('model.connection_required', 'provider missing', { developerUi: true })
    expect(shown.description).toContain('model.connection_required')
    expect(shown.description).toContain('provider missing')
    expect(shown.workspaceView).toBe('model-service')
  })

  test('未知码用固定的一句话，不把原始异常放进普通详情', () => {
    const notice = sendNotStartedNotice('unknown', 'ECONNREFUSED 127.0.0.1', { developerUi: false })
    expect(notice.title).toBe('发送未开始')
    expect(notice.description).toBe('消息没有发出去，草稿已保留。')
    expect(notice.description).not.toContain('ECONNREFUSED')
    expect(notice.mergeKey).toBe('send-not-started:unknown')
  })

  test('13.35：改过模型配置后未重新测试连接，提示去模型服务重新测试，不再像没反应', () => {
    const notice = sendNotStartedNotice('model.connection_unverified', 'raw', { developerUi: false })
    expect(notice.title).toBe('模型连接需要重新测试')
    expect(notice.workspaceView).toBe('model-service')
    expect(notice.description).toContain('重新测试连接')
    expect(notice.description).not.toContain('model.connection_unverified')
    const choose = sendNotStartedNotice('model.connection_choose_address', 'raw', { developerUi: false })
    expect(choose.title).toBe('还没有可用的模型连接')
    expect(choose.description).toContain('用作默认协议')
  })

  test('13.37 B3-1：dsh / Pi / Codex 的地址没重新测试，同样提示去模型服务重新测试，不再说「还没配置好」', () => {
    for (const backend of ['deepseek', 'pi', 'codex', 'fox']) {
      const notice = sendNotStartedNotice('model.connection_unverified', 'model.connection_unverified: 模型服务的配置改过了', { backend, developerUi: false })
      expect(notice.title).toBe('模型连接需要重新测试')
      expect(notice.workspaceView).toBe('model-service')
      expect(notice.description).toBe('模型服务的配置改过了，当前地址还没有通过连接测试：请到「设置 → 模型服务」重新测试连接，通过后再发送。')
      expect(notice.description).not.toContain('还没配置好')
      expect(notice.description).not.toContain('model.connection_unverified')
    }
  })
})
