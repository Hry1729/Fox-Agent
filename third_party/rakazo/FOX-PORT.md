# Rakazo

来源为 [elie222/rakazo](https://github.com/elie222/rakazo/tree/0963b8860c86507c1e24bd95ebcf2550a101e22c)，许可见本目录 LICENSE。

Fox 的 companion-memory-prompt.mjs 参考 packages/adapters/src/memory-context.ts 的 buildAgentMemoryPrompt。Fox 按联系人和项目授权选择资料，将常驻用量改为 Unicode 字符数，将可选文档改为专题目录和相关片段。重复条目和超限的处理参考 packages/adapters/src/memory-tools.ts，数据存储采用 Fox 的 Rust 仓储和 SQLite。
