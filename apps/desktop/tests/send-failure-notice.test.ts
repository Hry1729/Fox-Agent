import { describe, expect, test } from 'bun:test'
import { sendNotStartedNotice } from '../src/features/chat/send-failure-notice'

describe('sendNotStartedNotice', () => {
  test('模型连接失败进模型服务，上一轮忙碌和项目文件夹各有一句人话', () => {
    const connection = sendNotStartedNotice('model.connection_required', '需要连接', { developerUi: false })
    expect(connection.title).toBe('还没有可用的模型连接')
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

  test('未登录类带上后端名，有对话时打开对话，没有对话时去 Agent 设置', () => {
    const named = sendNotStartedNotice('agent_backend_missing_credentials', '未登录', { backend: 'deepseek', conversationId: 'c-1', developerUi: false })
    expect(named.title).toBe('dsh 还没有登录')
    expect(named.workspaceView).toBe('chat')
    expect(named.entityId).toBe('c-1')
    expect(named.description).not.toContain('agent_backend_missing_credentials')

    const installed = sendNotStartedNotice('agent_backend_not_installed', '没有安装', { backend: 'pi', developerUi: false })
    expect(installed.title).toBe('pi 还没有登录')
    expect(installed.workspaceView).toBe('settings-ai')
    expect(installed.entityId).toBeUndefined()
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
})
