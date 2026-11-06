# OpenMausBot

来源为 [milind-soni/OpenMausBot](https://github.com/milind-soni/OpenMausBot/tree/58378e85449ca49f3370d72e6c70cece4fee413f)，许可及版权见本目录 LICENSE 和 NOTICE。

Fox 的专题整理参考 server/memory-organize.ts 和 server/memory-upkeep.ts 的分工。模型指定待移条目及专题，Host 负责移动原文。Fox 按 SQLite 条目标识操作，对用户手写的条目先请求确认，并对专题文件检查内容哈希。
