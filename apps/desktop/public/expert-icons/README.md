# Fox 专家头像库

将专家头像放在当前目录，推荐使用正方形的 PNG、WebP 或 SVG 文件。

新增头像后，在 `index.json` 中登记：

```json
{
  "id": "frontend-expert",
  "name": "前端专家",
  "src": "/expert-icons/frontend-expert.png"
}
```

头像路径使用 `/expert-icons/文件名`，文件会随桌面端一起构建发布。
