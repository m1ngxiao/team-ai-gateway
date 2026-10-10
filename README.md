# Team AI Gateway

面向团队的自托管 AI 中转服务。集中管理 OpenAI / Claude 账号池和第三方 API 上游，为成员分配平台 API Key，通过 Web 后台维护模型、路由与用量，并提供可选的只读统计看板。

仓库包含完整 Rust 后端、Next.js 管理界面、Python 看板和服务器运维工具，直接从本仓库构建。当前项目版本 **0.1.0**，支持 **Linux / Web** 部署；基于 [CodexManager](https://github.com/qxcnm/Codex-Manager) v0.6.0，保留上游 MIT 版权。目前没有本项目发布的 GHCR 镜像或桌面安装包。

## 已支持的功能

| 功能 | 当前实现 |
| --- | --- |
| Web 管理后台 | 管理账号、第三方 API 上游、成员平台 Key、模型目录、价格、路由、分组及用量 |
| 独立账号池 | OpenAI 与 Claude.ai 订阅账号分别授权、管理和轮转；每个 Key 明确选择上游池 |
| 客户端接口 | OpenAI 风格 Responses / Chat Completions、Anthropic Messages 兼容入口及按 Key 权限返回的模型列表 |
| 模型配置 | 内置 GPT-6.1 Sol、GPT-6 Luna 等模型配置；支持自定义模型、上游模型映射、启停和价格管理 |
| 调度与容量备用 | 保留账号调度和会话绑定；在满足安全重放条件的容量错误下尝试备用账号，支持流式处理与断连归因 |
| 订阅额度 | 展示 OpenAI 额度；Claude 支持最近采集的 5 小时 / 7 天额度、手动刷新、自动轮询与限流退避 |
| 只读统计看板 | 独立登录，展示脱敏账号、额度、Key、成员和分组统计；模型从数据库可见目录动态展示 |
| 美国代理自动选择 | 可选 Mihomo 运维工具：仅在经过出口认证的美国节点中选择低延迟线路，默认每 180 秒检测、30ms 容差；手动更新订阅时保留分组和选择，失败回滚 |
| 每日模型同步 | 可选 systemd 任务：检查最新稳定 Codex CLI 目录，验证账户权限、官方价格及真实流式工具调用后添加新模型，保留已有模型配置 |
| 服务器维护 | Docker Compose、启动检查、私有目录初始化、SQLite 一致备份、升级 / 回退说明及可选 Cloudflare Tunnel 模板 |

模型目录条目、Key 可见模型和上游实际调用能力各有条件，需要启用的模型与路由、匹配的 Key 权限及有效上游账号。**内置模型名称不代表每个账号都能调用。** Claude 订阅账号池支持 Messages 和 Responses，暂不支持 Chat Completions；完整范围见 [客户端接入](docs/CLIENTS.md)。

模型同步与代理工具需要单独安装，默认 Compose 不启动它们。同步仅添加通过验证的新模型，不自动删除旧模型或改写管理员设置；验证消耗少量真实用量。Claude 额度查询使用非公开接口，过期时显示未知，见 [配置说明](docs/CONFIGURATION.md)。

## 在服务器上部署

### 1. 准备服务器与配置

需要 Linux、Git、Docker Engine + Compose v2（支持 BuildKit）、Python 3.10+，以及能下载构建依赖和访问上游的网络。默认运行资源配置为 4 CPU / 8 GiB，首次源码构建需要额外内存和磁盘；可在 env 文件调整。

```bash
git clone https://github.com/m1ngxiao/team-ai-gateway.git
cd team-ai-gateway
test ! -e deploy/.env && test ! -L deploy/.env && (umask 077; cp deploy/.env.example deploy/.env)
chmod 600 deploy/.env
```

编辑 `deploy/.env`：

| 配置 | 如何填写 |
| --- | --- |
| `PUBLIC_API_ORIGIN` | 客户端 API 域名，不含 `/v1`；本地使用可保留默认值 |
| `PUBLIC_ADMIN_ORIGIN` / `PUBLIC_STATS_ORIGIN` | 后台 / 看板地址，公网填写 HTTPS origin |
| `UPSTREAM_PROXY` | 可选运行代理，必须能从 Docker 容器访问；留空直连 |
| `HTTP_PROXY` / `HTTPS_PROXY` | 可选镜像构建代理，与运行代理独立 |
| CPU、内存与线程参数 | 按服务器资源调整，完整名称见 [配置模板](deploy/.env.example) |
| `SOURCE_REVISION` | 可填 `git rev-parse HEAD` 的结果，记录镜像对应源码 |

容器中的 `127.0.0.1` 是容器自身。宿主回环代理不能直接填为容器代理地址，见 [部署文档](docs/DEPLOYMENT.md#配置域名代理与资源)。

### 2. 构建并启动主服务

在仓库根目录运行：

```bash
sudo bash scripts/server.sh --env-file deploy/.env init
sudo bash scripts/server.sh --env-file deploy/.env build
sudo bash scripts/server.sh --env-file deploy/.env up
sudo bash scripts/server.sh --env-file deploy/.env status
```

| 用途 | 默认地址 |
| --- | --- |
| 管理后台 | `http://127.0.0.1:48761` |
| 客户端 API Base URL | `http://127.0.0.1:48760/v1` |
| 可选统计看板 | `http://127.0.0.1:48763` |

端口默认只绑定宿主回环地址。远程访问后台时，在个人电脑运行：

```bash
ssh -N -L 48761:127.0.0.1:48761 user@your-server
```

打开 `http://127.0.0.1:48761`，先在“设置 → 访问控制”启用后台登录并创建管理员，再添加获授权的上游、配置模型路由、创建平台 Key。空库不会自动完成登录保护，开放公网后台前需完成此步骤。普通账号调度默认回落为 `ordered`，需要均衡轮转可在后台选 `balanced`。

公网使用 HTTPS 反向代理或可选 Tunnel；API、后台、看板建议使用不同域名。完整步骤见 [Linux 部署](docs/DEPLOYMENT.md)。

### 3. 按需启用组件

| 组件 | 安装入口 |
| --- | --- |
| 只读用量看板 | [初始化私有配置并启用 dashboard profile](docs/DEPLOYMENT.md#启用只读看板) |
| 美国节点自动选择 | [Mihomo 安装、节点认证、自动组与订阅维护](deploy/proxy/MAINTENANCE.md) |
| 每日模型同步 | [私有配置、首次验证及 systemd timer 安装](deploy/model-sync/README.md) |
| Cloudflare Tunnel | [HTTPS 接入](docs/DEPLOYMENT.md#https-接入) · [配置模板](deploy/cloudflared.yml.example) |

美国节点来自操作者的私有认证清单，不根据节点名猜测国家。30ms 延迟容差减少来回切换；延迟检测不等同于下载带宽测试。订阅刷新是手动命令，没有默认订阅刷新定时器。

模型同步以最新稳定 Codex CLI 的公开目录为候选，与实际账户权限交叉验证。后台读取网关目录，看板读取数据库目录，模型增加后动态展示。客户端自定义的本地模型目录文件仍需分发更新并重启客户端，服务器不能自动修改同事电脑的配置。

## 日常运行与更新

```bash
sudo bash scripts/server.sh --env-file deploy/.env check
sudo bash scripts/server.sh --env-file deploy/.env status
sudo bash scripts/server.sh --env-file deploy/.env stop
sudo bash scripts/server.sh --env-file deploy/.env up
```

部署了看板时添加 `--dashboard`。`up` 使用已有镜像，不会隐式构建。更新源码后先备份，再构建、检查并切换；保留 `deploy/data`、`deploy/private` 和本地 env，不要为升级重新初始化看板密码，见 [备份与升级](docs/BACKUP_AND_UPGRADE.md)。

| 变更 | 是否需要构建镜像 |
| --- | --- |
| 同步新增模型、调整模型路由 / 价格 | 不需要，数据动态读取 |
| Mihomo 选择节点或更新订阅 | 不需要，由独立代理工具维护 |
| 修改后端 / 管理界面源码或 `PUBLIC_*_ORIGIN` | 需要重新构建 gateway |
| 修改看板 / 采集器源码 | 需要重建共享 dashboard 镜像并更新两个组件 |
| 修改宿主同步 / 代理脚本 | 更新脚本和配置；修改 systemd unit 后重新加载 |

## 文档与源码

| 目标 | 文档 |
| --- | --- |
| 服务器安装、域名、代理、看板 | [Linux 部署](docs/DEPLOYMENT.md) |
| API 与客户端接入 | [客户端配置](docs/CLIENTS.md) |
| 参数、账号池与额度 | [配置参考](docs/CONFIGURATION.md) |
| 调度、备用、并发与额度边界 | [调度说明](docs/SCHEDULING.md) |
| 自动模型 / 代理维护 | [模型同步](deploy/model-sync/README.md) · [代理维护](deploy/proxy/MAINTENANCE.md) |
| 备份、迁移和回退 | [备份与升级](docs/BACKUP_AND_UPGRADE.md) |
| 开发、架构和测试 | [架构](ARCHITECTURE.md) · [开发](DEVELOPMENT.md) · [测试](TESTING.md) |
| 来源与变更 | [上游说明](UPSTREAM.md) · [更新记录](CHANGELOG.md) · [第三方声明](THIRD_PARTY_NOTICES.md) |

`apps/` 是管理界面，`crates/` 是后端，`services/dashboard/` 是看板，`scripts/` 与 `deploy/` 提供维护工具和部署模板。`apps/src-tauri/` 保留兼容源码，桌面构建与自动更新不属于当前支持的发布目标。

## 参与与许可

贡献流程见 [贡献指南](CONTRIBUTING.md)，认证与数据泄露问题见 [安全说明](SECURITY.md)。实际 Key、账号 token、订阅链接、数据库及服务器私有配置应保存在仓库之外。

项目采用 [MIT License](LICENSE)，保留 `Copyright (c) 2026 hongshun.gao`。Team AI Gateway 是独立衍生项目，不代表上游作者或模型服务提供商。部署者负责所接入账号、数据与服务的使用授权。
