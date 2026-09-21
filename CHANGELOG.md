# 更新记录

Changes describe this project's releases, independently of the inherited Rust crate version.

## 0.1.0

首个独立源码发行版，来源为 CodexManager v0.6.0。

- 集成团队 Web 界面、服务端客户端 profile 禁用行为、可配置 API/后台/看板 origin。
- 集成容量错误动态备用、语义流判断、有限重试预算、成功交付后的会话迁移与下游断连健康归因修复。
- 提供独立只读看板和无网络 collector，展示脱敏账号、Key、分组与模型统计。
- 提供直接从仓库源码构建的 Docker 镜像定义、通用 Compose、可选看板 profile 和安全初始化检查。
- 整理开发、配置、调度、客户端、备份、升级与许可文档，并配置适用于本项目的验证入口。

本版本提供 Linux/Web 源码构建流程。根 Rust workspace、Web 前端及本地项目镜像版本为 `0.1.0`，数据库以 CodexManager v0.6.0 为兼容基线。保留的桌面兼容源码不属于本次发布目标。不将上游桌面发布历史或其他部署的验收记录计为本项目发行结果。
