# Fox 桌面连接器授权

桌面入口为「插件 → 连接器」。浏览器授权使用 PKCE，回调只监听本机。Fox 把用户令牌保存在系统凭证库中。对话调用远程平台时使用该用户的权限。

## 飞书文档

1. 在飞书开放平台为 Fox 注册并发布应用。为目标租户开通文档搜索、读取、创建、更新以及 `offline_access` 权限。将 `http://127.0.0.1:42181/oauth/callback` 登记为回调地址。
2. 在团队服务器配置 `FEISHU_APP_ID`、`FEISHU_APP_SECRET` 和 `FEISHU_REDIRECT_URI`。使用 `node feishu-broker.mjs` 启动服务。默认监听 `127.0.0.1:8787`。通过团队 HTTPS 服务公开 `/oauth/feishu/token`。
3. 构建桌面端时设置 `FOX_CONNECTOR_REGISTRATIONS_JSON`。其中 `CLIENT_ID` 和 `TOKEN_URL` 是可随安装包分发的公开配置。需要调整权限时，可在 `SCOPE` 中填写由空格分隔的已获批权限。

```json
{
  "FEISHU": {
    "CLIENT_ID": "填写已发布的应用 ID",
    "TOKEN_URL": "https://你的授权域名/oauth/feishu/token",
    "CALLBACK_PORT": "42181"
  }
}
```

本机开发可通过 `FOX_CONNECTOR_FEISHU_CLIENT_ID`、`FOX_CONNECTOR_FEISHU_TOKEN_URL`、`FOX_CONNECTOR_FEISHU_CALLBACK_PORT` 和 `FOX_CONNECTOR_FEISHU_SCOPE` 覆盖构建配置。应用密钥只保存在团队服务器上。

业务验收需要用测试租户完成一次真实授权，在 Fox 中创建测试文档、打开返回的文档链接、追加一段内容，再读回验证。飞书文档与飞书机器人使用独立凭证。

## 其他支持 OAuth 的平台

支持动态客户端注册的 MCP 服务由 Fox 与平台协商注册。要求预注册的平台使用目录中 `setup.registration` 对应的配置。向平台登记 Fox 的本机回调地址。公共客户端只需 `CLIENT_ID` 及平台要求的 `SCOPE`。

对于要求应用密钥的平台，通过 `node oauth-broker.mjs` 为每个应用启动独立的令牌交换服务。配置 `OAUTH_CLIENT_ID`、`OAUTH_CLIENT_SECRET`、`OAUTH_REDIRECT_URI`、`OAUTH_ISSUER_TOKEN_URL` 和平台要求的 `OAUTH_TOKEN_AUTH_METHOD`。桌面配置中填写该服务的 HTTPS `TOKEN_URL` 和平台固定的 `ISSUER_TOKEN_URL`。支持表单和 HTTP Basic 两种密钥提交方式。平台应用审批、授权范围和付费套餐需要在各平台开通。

## 本地验证

从 Fox 仓库运行 `node --test services/agent-runtime/test/connector-auth.test.mjs`，验证授权回调、令牌交换和刷新。网关的运行方式见 [消息网关说明](../connector-channels/README.md)。
