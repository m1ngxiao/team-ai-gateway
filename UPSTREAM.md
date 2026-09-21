# 上游与兼容性

Team AI Gateway 是基于 CodexManager 的独立衍生项目。This repository contains the integrated source tree and builds without fetching another upstream checkout.

| 项目 | 来源 |
| --- | --- |
| 上游仓库 | [qxcnm/Codex-Manager](https://github.com/qxcnm/Codex-Manager) |
| 基线版本 | `v0.6.0` |
| 基线提交 | [`964e11a93fdb29ce4dcb2f881f2ca99b4c82938f`](https://github.com/qxcnm/Codex-Manager/commit/964e11a93fdb29ce4dcb2f881f2ca99b4c82938f) |
| 上游许可 | MIT，`Copyright (c) 2026 hongshun.gao`，原文见 [LICENSE](LICENSE) |
| 本项目版本 | `0.1.0` |

本项目保留上游账号/Key 管理、模型路由、协议适配、存储迁移及 Web 管理基础，并集成团队界面、服务端 profile 限制、动态容量备用、交付与健康归因修复，以及独立只读看板和通用 Linux 部署流程。构建时不再应用外部 overlay。

## 保留的名称

Rust package 和二进制继续使用 `codexmanager-*`，兼容环境变量继续使用 `CODEXMANAGER_*`，数据库和 RPC 命名也保留。它们代表技术兼容性，不表示当前仓库由上游维护。根 Rust workspace、Web 前端与项目镜像采用本项目版本 `0.1.0`，上游来源版本仍记录为 `v0.6.0`。

`apps/src-tauri/` 保留上游桌面兼容源码、命令和资源，但不加入根 Rust workspace 的 Linux/Web 构建目标。本仓库当前不承诺 Windows/macOS 桌面安装包。桌面更新器仍默认指向 `qxcnm/Codex-Manager`，可受 `CODEXMANAGER_UPDATE_REPO` 覆盖；它不是本项目的发行或更新渠道。自行维护桌面发行时需单独审查更新来源和打包流程。

## 依赖固定

`Cargo.lock` 和 `apps/pnpm-lock.yaml` 是源码的一部分。根 `Cargo.toml` 固定使用以下 WebSocket fork，不能仅凭 crate 同名替换：

- `tokio-tungstenite`：`openai-oss-forks/tokio-tungstenite`，提交 `0e5b2d73aa18dd9f0a50ee9ff199d5aef7594186`。
- `tungstenite`：`openai-oss-forks/tungstenite-rs`，提交 `4fffad30fe373adbdcffab9545e9e9bf4f2fc19f`。

这些依赖仍需在首次构建时下载；“包含完整源码”指本项目自身源码可直接构建，并不表示所有外部依赖都已 vendoring。

## 后续同步

同步上游时对照固定基线审查差异，优先检查数据库迁移、权限与 RPC、协议转换、流式交付和调度。保留本项目行为回归，更新来源记录与锁文件；不直接恢复上游的桌面发布工作流或覆盖本项目部署默认值。
