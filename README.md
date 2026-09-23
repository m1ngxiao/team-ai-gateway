# Team AI Gateway

面向团队的自托管 AI 中转服务：统一管理获授权的账号池和第三方 API 上游，为成员分配独立平台 API Key，通过 Web 后台配置模型与路由，并可选部署只读用量看板。

**Self-hosted AI gateway for teams, with account pools, per-member API keys, a web console, and an optional read-only usage dashboard.**

当前项目版本为 **0.1.0**，基于 [CodexManager](https://github.com/qxcnm/Codex-Manager) v0.6.0，保留上游 MIT 版权。本仓库包含完整、已集成改进的源码，面向 Linux/Web 部署；目前提供本地源码构建，不提供已发布的 GHCR 镜像或桌面安装包。

## 提供什么

- Web 管理后台：账号池、第三方 API 上游、平台 Key、模型目录、路由和用量。
- OpenAI 风格 Responses / Chat Completions，以及 Anthropic Messages 兼容入口；实际可用模型和功能由 Key、路由及上游决定。
- 保留正常账号调度与会话绑定，在可安全重放的容量错误下选择动态备用账号；包含流式语义判断和下游断连归因修复。
- 可选只读看板：独立登录，展示脱敏账号、额度、Key 与分组统计；采集器无网络且只读访问源数据库。
- 通用 Docker Compose、启动检查、安全目录初始化、SQLite 一致备份及可选 Cloudflare Tunnel 模板。

客户端使用平台 Key，管理员管理上游授权。OpenAI 与 Claude 订阅账号分别保存在独立账号池；平台 Key 必须选择其中一个池。Claude 账号通过后台一次性授权码登录，账号登录后默认停用；管理员配置模型路由并启用账号，再发送真实请求验证。服务端不会替客户端读写本机 `.codex` 配置。详细配置见 [配置参考](docs/CONFIGURATION.md#平台-key-的上游池)，调度边界见 [调度说明](docs/SCHEDULING.md)。

## 快速开始

需要 Linux、Docker Engine + Compose v2、Python 3.10+，以及可访问构建依赖和所需上游的网络。首次构建比日常运行需要更多资源。

```bash
git clone https://github.com/m1ngxiao/team-ai-gateway.git
cd team-ai-gateway
test ! -e deploy/.env && test ! -L deploy/.env && (umask 077; cp deploy/.env.example deploy/.env)
chmod 600 deploy/.env
```

编辑 `deploy/.env` 后运行：

```bash
sudo bash scripts/server.sh --env-file deploy/.env init
sudo bash scripts/server.sh --env-file deploy/.env build
sudo bash scripts/server.sh --env-file deploy/.env up
sudo bash scripts/server.sh --env-file deploy/.env status
```

默认只启动主服务，地址如下：

| 用途 | 默认地址 |
| --- | --- |
| 管理后台 | `http://127.0.0.1:48761` |
| 客户端 API Base URL | `http://127.0.0.1:48760/v1` |
| 可选只读看板 | `http://127.0.0.1:48763` |

端口只绑定宿主回环地址。远程部署通过 SSH 转发访问后台。**先启用后台自身的账号登录并建立管理员，再开放公网入口**；新建空库不会自动完成这一步。然后添加获授权的上游、配置模型路由并为成员建立平台 Key。新库的普通账号调度默认回落为 `ordered`，需要均衡轮转时在后台选择 `balanced`；容量错误动态备用由部署模板独立启用。

看板需要单独初始化私密配置，再通过 `--dashboard` 启用；完整可执行步骤见 [部署文档](docs/DEPLOYMENT.md)。公网地址使用 HTTPS，修改 `PUBLIC_*_ORIGIN` 后重新构建主服务。已有数据升级时不要重建看板密码，见 [备份与升级](docs/BACKUP_AND_UPGRADE.md)。

## 文档

| 目标 | 文档 |
| --- | --- |
| 安装、域名、代理、只读看板 | [Linux 部署](docs/DEPLOYMENT.md) |
| 客户端 API 接入 | [客户端配置](docs/CLIENTS.md) |
| 配置层次和环境变量 | [配置参考](docs/CONFIGURATION.md) |
| 账号选择、容量备用、并发与额度 | [调度与边界](docs/SCHEDULING.md) |
| 数据备份、迁移、回退 | [备份与升级](docs/BACKUP_AND_UPGRADE.md) |
| 源码结构与运行边界 | [架构](ARCHITECTURE.md) |
| 本地开发和验证 | [开发](DEVELOPMENT.md) · [测试](TESTING.md) |
| 来源、变更及许可 | [上游说明](UPSTREAM.md) · [更新记录](CHANGELOG.md) · [第三方声明](THIRD_PARTY_NOTICES.md) |

`apps/src-tauri/` 作为兼容源码保留，其桌面自动更新默认仍指向上游。当前发布目标是本仓库的 Linux/Web 服务，升级通过源码构建与数据迁移流程完成。

## 参与与许可

欢迎带有清晰复现步骤和验证结果的改进，参见 [贡献指南](CONTRIBUTING.md)。涉及认证、数据泄露或访问控制的问题请按 [安全说明](SECURITY.md) 处理，勿在公开问题中提交数据库、token 或完整请求日志。

项目采用 [MIT License](LICENSE)，保留 `Copyright (c) 2026 hongshun.gao`。Team AI Gateway 是独立衍生项目，不代表上游作者或模型服务提供商。部署者负责所接入账号、数据与服务的使用授权。
