# Fox 消息网关

QQ、微信、钉钉、飞书机器人、企业微信机器人、Telegram、Discord、元宝和 MQTT 共用此网关。每个网关进程对应一个平台账号。网关运行期间接收文本，Fox 可查询最近 200 条消息并回复原会话。收件记录保存在进程内，重启后从新消息开始。

## 启动

管理员在网关主机安装 uv，设置平台配置 `FOX_CHANNEL_CONFIG` 和随机访问令牌 `FOX_CHANNEL_TOKEN`。以下命令在 Fox 仓库目录中执行。

```powershell
$env:FOX_CHANNEL_CONFIG = '{"bot_token":"平台签发的机器人令牌"}'
$env:FOX_CHANNEL_TOKEN = python -c "import secrets; print(secrets.token_urlsafe(32))"
uv run --script services/connector-channels/gateway.py telegram --port 8765
```

在「插件 → 连接器 → Telegram」填写 `http://127.0.0.1:8765/mcp` 和刚生成的访问令牌，然后测试连接。在另一个端口运行第二个平台时，为其设置独立凭证。团队托管场景使用 HTTPS 反向代理和租户独立的网关实例。仅向对应租户发放令牌。已启用 DNS 重绑定检查的 MCP 服务需要由反向代理向本地后端传递环回 Host。

| 桌面条目 | 网关参数 | 平台配置字段 |
| --- | --- | --- |
| QQ 机器人 | `qq` | `app_id`、`secret`，按账号配置填写 `token` |
| 微信 | `weixin` | iLink 账号的 `token` 和 `bot_uin` |
| 钉钉 | `dingtalk` | `app_key`、`app_secret`，启用 Stream 模式 |
| 飞书机器人 | `feishu` | `app_id`、`app_secret`，订阅接收消息事件并开通长连接 |
| 企业微信机器人 | `wecom` | 智能机器人的 `bot_id`、`secret` |
| Telegram | `telegram` | `bot_token`，可用 `allowed_user_ids` 限制来源 |
| Discord | `discord` | `bot_token`，私信需配置 `allowed_user_ids` |
| 元宝 | `yuanbao` | `app_key`、`app_secret`，按平台绑定结果配置服务地址 |
| MQTT | `mqtt` | `host`、`port`、`subscribe_topic`、`publish_topic` 及代理服务器要求的 TLS 和账号配置 |

在测试账号上发送一条文本，让 Fox 调用 `recent_messages` 读取它，再明确要求 Fox 用 `reply` 回复。需要平台账号授权、管理员批准或平台侧事件订阅的步骤由该账号的管理员操作。平台接口由 [Octop Gateway](https://github.com/TencentCloud/octop-gateway) 提供。

开发者可运行 `uv run --script services/connector-channels/test_mqtt.py`。该测试启动本机 MQTT 代理和虚拟设备，通过 HTTP MCP 检查鉴权、消息读取和回复。
