# FoxOps CLI 授权页面完整迁移方案

## 1. 迁移目标

迁移 Yuxi `/auth/cli/authorize`，让已登录用户确认 CLI 设备授权会话。页面必须清晰显示正在授权的客户端和 user code，防止用户在不知情情况下批准错误请求。

## 2. 路由与权限

- 路由 `/auth/cli/authorize?user_code=...`，必须登录。
- 未登录时保存完整站内 URL，登录成功后返回本页。
- 缺少或格式无效的 `user_code` 直接显示错误，不请求后端。

## 3. 页面内容

- 标题“授权 Yuxi/FoxOps CLI”按品牌配置显示。
- 加载会话时显示 skeleton/spinner。
- 会话摘要：醒目的 user code、客户端/设备信息、申请权限、创建/过期时间（以后端字段为准）。
- 安全提醒：只有用户本人发起 CLI 登录时才批准，不向他人分享 code。
- 主按钮“确认授权”，审批中防重复；可提供“取消/返回”但不能把关闭当作批准。
- 成功后显示“授权成功，可返回终端”，不自动暴露 token。

## 4. 数据流程

- `GET /api/auth/cli/session/{user_code}` 获取会话。
- `POST /api/auth/cli/session/{user_code}/approve` 批准会话。
- 后端路径以现有 `authApi.getCLIAuthSession`、`approveCLIAuthSession` 为准。
- 处理不存在、已过期、已批准、已拒绝和与当前用户不匹配等状态。
- 批准成功后无需前端轮询 token；CLI 端通过自己的设备流程取 token。

## 5. 安全与验收

- [ ] 页面必须登录，登录后能回到原 user_code。
- [ ] user_code 缺失/无效/过期不会出现可点击批准按钮。
- [ ] 会话信息和安全提醒足以让用户核对请求。
- [ ] 批准幂等、防重复，成功页不展示 token。
- [ ] 错误状态可返回，不把后端内部信息暴露给用户。

