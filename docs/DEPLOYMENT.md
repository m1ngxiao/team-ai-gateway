# Linux 部署

仓库包含完整服务源码，构建不需要下载另一份上游仓库或应用补丁。推荐使用 Linux、Git、Docker Engine 和 Docker Compose v2（支持 BuildKit）、Python 3.10+。以下命令面向普通的 rootful Docker；数据目录由容器 UID/GID `10001:10001` 持有。使用 rootless Docker 时需要自行适配 UID 映射，本模板不自动调整现有数据权限。

默认只启动 `gateway`，内含 Rust API 服务和嵌入式 Web 管理后台。`collector` 和 `dashboard` 是可选的 `dashboard` profile。所有宿主端口只监听 `127.0.0.1`；通过 SSH 转发访问后台，或另行配置 HTTPS 反向代理。

## 第一次启动

在仓库根目录运行。以下复制操作遇到已有配置会停止，不覆盖它：

```bash
test ! -e deploy/.env && test ! -L deploy/.env && (umask 077; cp deploy/.env.example deploy/.env)
chmod 600 deploy/.env
```

编辑 `deploy/.env`，然后创建数据目录、构建并启动：

```bash
sudo bash scripts/server.sh --env-file deploy/.env init
sudo bash scripts/server.sh --env-file deploy/.env build
sudo bash scripts/server.sh --env-file deploy/.env up
sudo bash scripts/server.sh --env-file deploy/.env status
```

`init` 只创建缺失的 `deploy/data/gateway`、`deploy/data/dashboard`、`deploy/private` 及其数据父目录。已有目录必须是普通目录、属于 UID 10001；私有目录须为 `700`。脚本不会递归修改所有者、覆盖数据库、重置密码，遇到不兼容的目录会明确失败。初始化不需要运行 Docker。

默认地址：API 为 `http://127.0.0.1:48760/v1`，后台为 `http://127.0.0.1:48761`。远程服务器可在个人电脑建立：

```bash
ssh -N -L 48761:127.0.0.1:48761 user@your-server
```

然后访问个人电脑的 `http://127.0.0.1:48761`。先在“设置 → 访问控制”启用后台账号登录、创建管理员，再添加获授权的上游账号和平台 API Key。**空库不会自动启用后台登录保护，完成这一步之前不要公开后台。**

## 配置域名、代理与资源

| 配置 | 用途与默认值 |
| --- | --- |
| `PUBLIC_API_ORIGIN` | 客户端 API 地址，不含 `/v1`；默认 `http://127.0.0.1:48760` |
| `PUBLIC_ADMIN_ORIGIN` | 管理后台地址；默认 `http://127.0.0.1:48761` |
| `PUBLIC_STATS_ORIGIN` | 可选看板地址；默认 `http://127.0.0.1:48763` |
| `GATEWAY_PORT` / `ADMIN_PORT` / `STATS_PORT` | 宿主回环端口；默认 48760 / 48761 / 48763 |
| `UPSTREAM_PROXY` | 容器运行时上游代理；留空表示直连 |
| `HTTP_PROXY` / `HTTPS_PROXY` | 镜像构建使用的代理；不会作为构建凭据写入最终镜像环境 |
| `GATEWAY_CPUS` / `GATEWAY_MEMORY` | 主服务限制；默认 4 CPU / 8 GiB |
| `HTTP_WORKERS` / `STREAM_WORKERS` | 普通/流式工作线程的最小值与最大值；默认各 8 |
| `TOKIO_WORKER_THREADS` / `RAYON_NUM_THREADS` | 底层运行时线程；默认各 4 |
| `CARGO_BUILD_JOBS` | Rust 编译并发；默认 4，与运行时限制独立 |
| `DYNAMIC_OVERLOAD_ENABLED` | 动态容量备用策略开关；模板显式启用 `true` |
| `CODEX_LATEST_SYNC_INTERVAL_SECS` | 网关的 Codex 版本元数据检查周期，模板默认 86400 秒；新增模型由独立的每日模型同步任务验证并添加 |
| `COLLECTOR_*` / `DASHBOARD_*` | 可选组件 CPU 与内存限制，完整名称见 env 模板 |
| `SOURCE_REVISION` | 可选镜像来源标签；源码包可保留 `unknown` |

公网 origin 必须是 HTTPS，HTTP 仅允许回环主机。使用准确 origin，不带凭据、末尾 `/`、路径、查询或片段；API、后台和看板建议采用不同域名。Docker 构建将 `PUBLIC_*` 映射为前端 `NEXT_PUBLIC_*`，**修改域名后须重新构建并启动 gateway**。修改本地端口时同时修改相应 origin。`STATS_PORT` 改为其他端口时，还需由管理员在私有 `auth.json` 的 `origins` 中同步本地地址；默认初始化只加入 48763。

镜像默认普通 Docker bridge 网络，不依赖固定网段、宿主代理、systemd slice 或 CPU 编号。容器中的 `127.0.0.1` 指向容器自身；代理应填写容器可访问的地址。每个上游账号自己的代理设置可能覆盖全局代理，迁移时需要检查。构建代理与运行代理独立，脚本清除 shell 中与 Compose 同名的变量，仅使用显式 env 文件。

资源默认值是部署起点，不是容量承诺。构建可能比运行需要更多内存和磁盘，流式连接会长期占用工作线程。调整 CPU 时应同时评估线程和内存。动态策略是启动时读取的配置，改变后需重启服务；其状态保存在单进程内。

## 启用只读看板

先完成主服务首次启动，使 `codexmanager.db` 存在。构建可选镜像：

```bash
sudo bash scripts/server.sh --env-file deploy/.env build dashboard
```

使用同一镜像、UID 10001 和无网络容器初始化看板；下面只生成首次配置，不读取或打印现有凭据。将用户名 `viewer` 按需更改：

```bash
sudo test ! -e deploy/private/auth.json && \
sudo test ! -e deploy/private/collector.json && \
sudo test ! -e deploy/private/login.local.txt && \
sudo docker run --rm --network none --read-only \
  --user 10001:10001 --cap-drop ALL --security-opt no-new-privileges:true \
  --mount "type=bind,src=$PWD/deploy/data/gateway,dst=/source,readonly" \
  --mount "type=bind,src=$PWD/deploy/private,dst=/private" \
  team-ai-gateway-dashboard:0.1.0 \
  python -m dashboard.configure --private-dir /private \
  --source-db /source/codexmanager.db --username viewer
```

若看板要使用公网 HTTPS 地址，在该命令末尾添加 `--public-origin https://stats.example.com`。它会同时设置精确 origin 和 Tunnel origin。初始化程序发现已有 `auth.json` 会拒绝重建；不要为升级或迁移添加 `--rotate`，该选项用于主动换密码。

随机密码写入 `deploy/private/login.local.txt`，终端只显示完成消息。通过管理员的私密文件访问方式查看并交付它，勿放进日志或仓库。看板登录与后台账号、平台 Key 各自独立。`collector.json` 中记录的 `/source/codexmanager.db` 是容器路径，Compose 也显式使用这一源数据库。

```bash
sudo bash scripts/server.sh --env-file deploy/.env --dashboard check
sudo bash scripts/server.sh --env-file deploy/.env --dashboard up
sudo bash scripts/server.sh --env-file deploy/.env --dashboard status
```

看板访问 `http://127.0.0.1:48763`，远程可类似设置 SSH 转发。首次 snapshot 生成前看板可能暂时显示数据不可用。collector 没有网络，挂载源数据库为只读，只能写脱敏 snapshot；dashboard 只读该 snapshot，无法访问源数据库。

## 日常命令和启动检查

美国节点自动选择与每日模型同步是独立的可选宿主工具，不由 Compose 自动安装。代理工具需要现有 Mihomo 实例、私有订阅和经过测量的美国出口清单，见 [代理维护](../deploy/proxy/MAINTENANCE.md)。模型同步需要已有管理员 RPC token、实际探测 Key 与宿主数据目录，首次验证后启用每日 systemd timer，见 [模型同步](../deploy/model-sync/README.md)。模型与节点的日常运行数据更新不要求重新构建镜像；宿主代理接入容器时仍须使用容器可访问的代理地址。

```bash
sudo bash scripts/server.sh --env-file deploy/.env check
sudo bash scripts/server.sh --env-file deploy/.env status
sudo bash scripts/server.sh --env-file deploy/.env stop
sudo bash scripts/server.sh --env-file deploy/.env up
```

使用 `--dashboard` 可同时操作三服务；在命令末尾指定 `collector` 或 `dashboard` 可单独操作可选服务，例如 `stop dashboard`。`build collector` 实际构建共享的 dashboard 镜像。`up` 只使用已有本地镜像，不会拉取或隐式构建。

启动检查验证回环端口、UID、挂载位置和权限、collector 无网络、可选 profile、必要数据文件，以及根目录直接构建配置。它不要求 Git checkout 或固定上游提交，因此源码压缩包也可使用。`check` 不验证镜像内容、数据库业务数据或真实模型请求；容器健康检查也不代表上游账号一定可用。

部署数据及私密配置对普通宿主用户不可读，所以示例用 `sudo` 执行检查和启动。系统服务示例在 `deploy/team-ai-gateway.service.example`，需按实际仓库路径调整；可选看板初始化后再同时为 ExecStart/ExecStop 增加 `--dashboard`。

## HTTPS 接入

可使用自行管理的反向代理；`deploy/cloudflared.yml.example` 提供宿主机 Cloudflare Tunnel 的可选模板，另有独立 systemd 示例。部署脚本不会安装或启动 Tunnel。

模板只将列出的 `/v1` API 路径代理到 48760，其他 API hostname 路径返回 404；后台 hostname 要求 Access JWT 并继续使用应用自身登录。不要把交互式 Access 登录放在客户端 API 域名前。看板使用独立登录，配置其公网 origin 后再开放。若不部署看板，移除 stats ingress。域名、Tunnel ID、Access audience 均需替换，凭据留在仓库外的私有目录。

发布前检查未授权 API、后台和看板请求确实被拒绝，再使用自己的授权 Key 验证模型列表和流式请求。完整迁移与回退流程见 [备份与升级](BACKUP_AND_UPGRADE.md)。
