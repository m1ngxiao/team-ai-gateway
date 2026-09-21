# 贡献指南

欢迎围绕可复用的 Linux/Web 团队网关提交改进。Contributions should include a reproducible problem, a focused change, and relevant validation.

提交前先阅读 [架构](ARCHITECTURE.md)、[开发](DEVELOPMENT.md) 和 [测试](TESTING.md)。本仓库直接维护集成源码，不通过构建时 patch 或另一个上游 checkout 发布。

## 提交内容

- 用脱敏配置和最小输入描述问题，写出预期与实际结果。
- 在对应边界修改代码：页面在 `apps/src`，存储迁移在 `crates/core`，网关行为在 `crates/service`，看板在 `services/dashboard`。
- 说明是否影响流式响应、重试、会话绑定、权限或数据库兼容性，并更新相关文档。
- 运行与变更相关的验证，报告命令、结果以及未运行项目的原因；不要把跳过的检查描述成通过。

网关协议改动应覆盖 streaming/non-streaming、Responses/Chat Completions、工具调用和中断等实际受影响场景。部署或鉴权变动应验证失败路径以及已有数据不会被覆盖。纯文案改动无需添加重复实现逻辑的测试。

## 兼容性

保留 RPC、数据库与 `CODEXMANAGER_*` 命名的兼容性，除非变更明确包含迁移方案。更新依赖时一并提交锁文件，不移除固定 WebSocket fork 而只依赖同名发行包。桌面兼容源码仍在，但不能把 Linux/Web 验证结果作为桌面安装包的验证结论。

任何新设置应有默认值、配置入口、持久化或启动时语义说明；会改变已存在数据的迁移要有备份和回退说明。避免把某台机器的网段、CPU 编号、代理或运行目录写进通用默认值。

## 发布材料

不要提交数据库、WAL、账号授权、平台 Key、本地 `.env`、看板密码、Tunnel 凭据或带真实请求内容的日志。发布检查扫描暂存区，使用方法见 [测试文档](TESTING.md)。公开问题中的安全敏感内容按 [SECURITY.md](SECURITY.md) 处理。

贡献代码应有权按本项目 MIT 许可分发；保留来源署名，并在引入第三方代码时同步许可说明。
