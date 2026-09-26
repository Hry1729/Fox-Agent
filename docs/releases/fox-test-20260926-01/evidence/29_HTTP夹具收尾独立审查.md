# HTTP 夹具最后收尾

用户授权继续处理上一轮默认并行全量中的 HTTP 连接重置失败；不包含 main 集成或 push。GPT-6 Sol xhigh 子智能体实现，协调者独立审查与最终全量。

## 固定提交与改动

ed104ea670315b3dc4661470d4428c84d3e5101c，父 d3b44335c95ccc776f797d33e002cc9f085b3f39。
只改 kernel_coordinator/tests.rs 的测试 HTTP fixture 并新增 http_model_fixture_tests.rs，合计154增/5删。无生产逻辑、预算、Pi、schema、审批或性能阈值改动。

读取时仅当累计0字节、且遇到EOF/ConnectionReset/ConnectionAborted时继续接受连接，不推进请求计数、脚本响应或response hook。同一个45秒accept循环截止不刷新。现有单连接read timeout独立存在，不把它夸成任何情况下严格45秒总上限。
一旦收到任何请求字节，断开继续报错；部分正文断开继续报错；完整请求后的响应写入失败仍报错。不会将不完整请求静默当成未发生。

## 证据

开发者：
- http-model-fixture-d3b4433-01：4/0。
- wake-contention-http-fixture-d3b4433-01：原失败Host wake用例1/0，约9.53秒。
两者sourceHead=d3b4433加本卡测试源码WIP，之后固定为ed104ea，不冒充提交后执行。

协调者固定提交独立运行：

```text
python team-run.py review-http-fixture-ed104ea 120 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --no-default-features --lib http_model_fixture_tests:: -- --nocapture
```

退出0，1.32秒，4/0。实际socket零字节关闭后唯一有效请求取得第一条回复、hook恰一次；部分头55字节/正文63字节断开均产生预期panic且hook不触发。reset/aborted的零字节/非零字节分类由生产测试夹具调用的同一分类函数直接验证，不假称实际OS RST注入。
旧版对空EOF会panic仅为源码推断，未另跑旧版红例。

最终全量命令：

```text
python team-run.py review-final-full-ed104ea 300 cargo test --manifest-path apps/desktop/src-tauri/Cargo.toml --offline --no-default-features --lib
```

统一环境：默认Rust并行，LIBSQLITE3_FLAGS=SQLITE_DEFAULT_MEMSTATUS=0、ZVEC_AUTO_BUILD=0，真实zvec DLL、独立FOX_DATA_DIR及短TMP，复用构建缓存。日志及JSON保存在team-evidence。
最终全量退出0，164.37秒，1320 passed / 0 failed / 27 ignored。此前失败的真实Host wake用例及本轮C4/F3/P95用例在这次默认并行运行均通过。结论：当前提交测试收尾PASS；不抹去历史失败，不等于现场根因已排他确认。

## 归因与交付边界

旧全量10054日志没有已收字节数，因此本卡确认并修复零字节连接断开会误杀夹具的机制，不能排他断言历史10054必属这一分支。
P95历史慢态及历史F3具体现场的排他归因仍开放；本轮不伪造其根因关闭。C4仍只证明减少两个status工具调用，不证明减少模型轮次或真实费用。
原WIP保留、不清理共享临时目录、不reset/stash/clean、不push或合main；真实桌面、真实Provider、安装包及部署不在本轮。

最终核对：三份未跟踪文档SHA-256与28号记录完全一致；main仍fee9d4e0bb67a3b7378413c784c6d1877633bf10，集成树仍fbaf7ce32a5ce928b2712e4299ec6580da36143f。工作树仅剩原两份schema状态标记和未跟踪docs，无本轮未提交源码。
