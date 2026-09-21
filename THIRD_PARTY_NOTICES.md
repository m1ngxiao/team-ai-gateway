# 第三方声明

This project includes and depends on third-party software. Preserve the applicable licenses when redistributing source or binaries.

## CodexManager

本项目包含源自 [CodexManager](https://github.com/qxcnm/Codex-Manager) v0.6.0 的代码与资源，基线提交见 [UPSTREAM.md](UPSTREAM.md)。原版权声明为：

```text
Copyright (c) 2026 hongshun.gao
```

上游采用 MIT 许可，完整原文保留于根目录 [LICENSE](LICENSE)。本项目的修改并不撤销上游声明。再分发时保留适用的版权与许可文本。

## 构建依赖

Rust 依赖由 `Cargo.toml` 和 `Cargo.lock` 记录；前端由 `apps/package.json` 和 `apps/pnpm-lock.yaml` 记录；看板由 `services/dashboard/requirements.txt` 记录。主要组件包括 Rust/Tokio/Axum/SQLx、Next.js/React/Tauri、FastAPI/Uvicorn/Pydantic；各依赖以其自身许可为准。

WebSocket 依赖使用固定的公开 fork，来源和提交见 [UPSTREAM.md](UPSTREAM.md)。`crates/rusqlite` 是仓库内的兼容实现，不能与同名 crates.io 发行包视为可以直接互换。

镜像基于 Node、Rust、Debian 与 Python 官方基础镜像，并包含其操作系统和运行时包。当前文档是来源说明，不是完整的传递依赖许可证清单或 SBOM；再分发镜像或二进制时应按最终实际依赖收集所需声明。

## 名称与服务

OpenAI、Anthropic、Claude、Cloudflare、CodexManager 等名称用于说明协议、来源或可选集成，不表示相关权利人认可本项目。使用第三方服务需遵守该服务的适用条款；开源代码许可不授予账号或服务访问权限。
