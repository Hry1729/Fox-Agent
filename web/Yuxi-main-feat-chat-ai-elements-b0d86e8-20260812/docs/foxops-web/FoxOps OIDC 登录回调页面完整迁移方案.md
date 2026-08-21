# FoxOps OIDC 登录回调页面完整迁移方案

## 1. 迁移目标

迁移 Yuxi `/auth/oidc/callback` 的授权码交换和登录态恢复流程。该页视觉简单，但属于安全关键路径，必须保持独立路由和明确的加载、成功、失败状态。

## 2. 页面状态

- 加载：Spinner 与“正在处理登录…”，禁止重复提交 code。
- 失败：错误标题、可读错误摘要、“返回登录页”。
- 成功：成功结果与“正在跳转”，避免用户误关造成困惑。
- 页面不显示授权码、access token、完整响应或后端堆栈。

## 3. 回调流程

1. 若已经登录，直接安全返回首页/redirect，避免重复交换。
2. 从 query 读取一次性 `code`；缺失时进入失败态。
3. 调用 `/api/auth/oidc/exchange-code`。
4. 交换成功后立即用 `router.replace` 清除 URL 中 code。
5. 将 access token 和 uid、username、phone、avatar、role、department 写入统一 user store 和持久化层。
6. 读取并删除 `sessionStorage.oidc_redirect`，经站内路径清洗。
7. 若进入 Chat，先初始化 agent store；初始化失败仍可进入 Chat 并由页面重试。
8. 导航到最终目标，使用 replace 避免后退回一次性回调页。

## 4. 错误与安全

- 处理 provider 返回 error、缺 code、code 过期/已使用、网络失败、交换响应不完整。
- 交换失败清除可能的半成品 token 和用户状态。
- redirect 必须是站内路径；缺失时默认首页或 Chat。
- 不用固定 `setTimeout` 掩盖状态同步；仅在确需展示成功反馈时使用短、可清理延时。
- 刷新失败页不会重复使用已经成功交换的 code。

## 5. 验收清单

- [ ] code 只交换一次，成功后 URL 被清理。
- [ ] 用户所有关键字段和 token 统一写入 store/持久化。
- [ ] 原始 redirect 安全恢复并在读取后删除。
- [ ] 缺 code、过期、网络失败和 provider error 都有明确失败页。
- [ ] 日志和 UI 不泄露 code/token。

