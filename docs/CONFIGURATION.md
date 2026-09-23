# 配置参考

配置分为镜像构建、容器运行和后台持久化设置。Keep build-time public origins separate from runtime credentials and stored application settings.

## Compose 部署设置

复制 [deploy/.env.example](../deploy/.env.example) 为本地 `deploy/.env`，权限设为 600，通过 `--env-file` 显式传给 `scripts/server.sh`。已有文件不要覆盖。部署脚本清除 shell 中与 Compose 同名的变量，避免终端残留值覆盖指定文件。

| 变量 | 生效位置 | 说明 |
| --- | --- | --- |
| `PUBLIC_API_ORIGIN` / `PUBLIC_ADMIN_ORIGIN` / `PUBLIC_STATS_ORIGIN` | 构建时 | 三个对外 origin；映射到前端 `NEXT_PUBLIC_*`，改变后重建 gateway |
| `GATEWAY_PORT` / `ADMIN_PORT` / `STATS_PORT` | 容器创建时 | 宿主回环监听端口，容器内部仍为 48760 / 48761 / 48763 |
| `HTTP_PROXY` / `HTTPS_PROXY` | 构建时 | 拉取依赖使用的代理 |
| `UPSTREAM_PROXY` | 运行时 | 映射到 `CODEXMANAGER_UPSTREAM_PROXY_URL` 和标准代理变量；空值直连 |
| `GATEWAY_CPUS` / `GATEWAY_MEMORY` | 容器创建时 | 主服务 CPU、内存限制 |
| `HTTP_WORKERS` / `STREAM_WORKERS` | 服务启动时 | 映射到各自 `CODEXMANAGER_HTTP_*_MIN/MAX`，模板固定每类线程数量 |
| `TOKIO_WORKER_THREADS` / `RAYON_NUM_THREADS` | 服务启动时 | 运行时线程数量 |
| `CARGO_BUILD_JOBS` | 构建时 | Rust 编译并发 |
| `DYNAMIC_OVERLOAD_ENABLED` | 服务启动时 | 映射到 `CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED`，模板默认 true |
| `COLLECTOR_CPUS` / `COLLECTOR_MEMORY` | 容器创建时 | collector 资源限制 |
| `DASHBOARD_CPUS` / `DASHBOARD_MEMORY` | 容器创建时 | dashboard 资源限制 |
| `TZ` | 运行时 | 容器系统时区；不会改变看板采集器当前 UTC+8 统计日界线 |
| `SOURCE_REVISION` | 构建时 | 镜像来源标签；不决定源码从哪里获取 |

三个默认 origin 分别是 `http://127.0.0.1:48760`、`http://127.0.0.1:48761`、`http://127.0.0.1:48763`。公网 origin 使用 HTTPS，不带路径、末尾 `/`、凭据、query 或 fragment。本地改端口应同步 origin；看板还需要同步私有 `auth.json` 中的允许 origin。

未在 Compose 的 `environment` 中映射的任意变量，仅写入 `.env` 不会自动进入容器。需要高级 `CODEXMANAGER_*` 设置时，可使用后台提供的设置入口，或审查后明确加入 Compose 环境配置。不要使用未记录的旧部署 override 文件绕过启动检查。

## 应用设置与兼容环境变量

账号、平台 Key、模型、路由、访问控制等通过后台管理并持久化到 SQLite。部分设置有对应启动环境变量；已有非空进程环境值会覆盖相应持久化启动配置。迁移时要同时核对数据库设置和容器环境。

内置模型目录包含 `gpt-6-sol`，默认路由到 OpenAI 账号池。账号轮转 Key 的 `/v1/models` 采用账号上游提供的模型目录；客户端看到该模型仍取决于账号权限与上游灰度。新增模型本身不会改变账号的访问资格。

### 平台 Key 的上游池

管理员创建或编辑平台 Key 时，`upstreamProvider` 选择上游来源，取值为 `openai` 或 `claude`。它与客户端使用的 `protocolType` 是两个独立设置；仅将协议设为 Anthropic Messages 不会改变账号池。

| `upstreamProvider` | 当前路由范围 |
| --- | --- |
| `openai` | OpenAI 账号池及其现有的聚合 API 路由策略；不能绑定 Claude 聚合上游 |
| `claude` | `account_rotation` 使用独立的 Claude.ai 订阅账号池；原有 `aggregate_api_rotation` 仍使用 Claude API Key 聚合上游。两种策略都不会回落到 OpenAI 账号池；不支持跨池混合轮转 |

Claude 订阅账号在后台「Claude 账号池」页面发起登录。管理员打开授权链接，使用 Claude.ai Pro、Max 或 Team 账号授权，再将网页显示的 `CODE#STATE` 一次性授权码粘贴回后台。登录后账号默认停用；先在模型目录中为 Claude 模型配置 `account_pool`、`source_id=claude` 路由，再启用账号并发送真实请求验证。若启用了分销计费，模型还须配置价格；该价格用于平台内部计量，不表示订阅账号按 API 价格结算。Claude 账号的授权令牌写入私有 SQLite 数据库，备份时须按凭据保护。

以下是 Claude 订阅账号池平台 Key 的后台管理 RPC 请求结构；这不是客户端生成请求，不应发送到公开模型 API 地址：

```json
{
  "id": 1,
  "method": "apikey/create",
  "params": {
    "name": "Claude 订阅账号池示例",
    "upstreamProvider": "claude",
    "rotationStrategy": "account_rotation",
    "protocolType": "anthropic_native",
    "modelSlug": "claude-sonnet-5"
  }
}
```

继续使用已有 Claude API Key 聚合上游时，可选择 `aggregate_api_rotation` 并指定已启用的 Claude `aggregateApiId`；它是上游 ID，不是 API Key 明文：

```json
{
  "id": 1,
  "method": "apikey/create",
  "params": {
    "name": "Claude API 聚合示例",
    "upstreamProvider": "claude",
    "rotationStrategy": "aggregate_api_rotation",
    "aggregateApiId": "agg_claude_example",
    "protocolType": "anthropic_native"
  }
}
```

修改已有 Key 时使用 `apikey/updateModel`，在 `params` 中传入 Key 的 `id` 和需要更改的字段。历史上未固定 Claude 上游的聚合 API 轮转 Key，需要管理员按下面的形式显式绑定：

```json
{
  "id": 2,
  "method": "apikey/updateModel",
  "params": {
    "id": "gk_example",
    "upstreamProvider": "claude",
    "confirmRouteReview": true,
    "rotationStrategy": "aggregate_api_rotation",
    "aggregateApiId": "agg_claude_example"
  }
}
```

`confirmRouteReview=true` 仅用于迁移时被标记“需检查路由”的 Key；必须由管理员显式传入 `upstreamProvider`，否则标记不会清除，也不能重新启用。旧客户端若在已配置 Claude 上游，或同时配置通用 `compatible` 与 Codex 上游的环境中创建未固定上游的聚合 Key，也须明确传入 `upstreamProvider`。切换到 OpenAI 账号池时须在同一次请求中设置 `upstreamProvider=openai` 和 `rotationStrategy=account_rotation`；切换到 Claude 订阅账号池时设置 `upstreamProvider=claude` 和 `rotationStrategy=account_rotation` 并清空 `aggregateApiId`。不兼容的组合会被拒绝。

Claude 订阅账号池 Key 的 `/v1/models` 只列出配置了启用的 `account_pool/claude` 路由、且池中有活跃账号的模型。Claude API 聚合 Key 仍只列出关联到启用的 Claude API 上游的模型；绑定的 `aggregateApiId` 是优先候选，同池其他 Claude API 上游仍可作为候选。各账号的实际模型权限以发起请求时的上游响应为准。

Claude 订阅账号池 Key 可使用 `POST /v1/messages`、`POST /v1/responses`、本地估算的 `POST /v1/messages/count_tokens` 和 `GET /v1/models`；不支持 `POST /v1/chat/completions`。客户端示例及接口说明见[客户端接入](CLIENTS.md#claude-订阅账号池的接口范围)。后台「协议类型」的路径兼容选项不会扩展这个池的接口范围。

| 兼容配置 | 用途 |
| --- | --- |
| `CODEXMANAGER_DB_PATH` | 数据库路径；模板固定 `/data/codexmanager.db` |
| `CODEXMANAGER_RPC_TOKEN_FILE` | 内部 RPC token 路径；模板固定 `/data/codexmanager.rpc-token` |
| `CODEXMANAGER_SERVICE_ADDR` / `CODEXMANAGER_WEB_ADDR` | 容器内部监听；模板通过宿主端口发布时限制为回环 |
| `CODEXMANAGER_ROUTE_STRATEGY` | `ordered` 或 `balanced`；也可通过后台持久化设置 |
| `CODEXMANAGER_ACCOUNT_MAX_INFLIGHT` | 所有账号共同使用的单账号总在途上限；源码默认 0 |
| `CODEXMANAGER_REQUEST_GATE_WAIT_TIMEOUT_MS` | 请求门等待限制；0 表示不额外配置该等待超时 |
| `CODEXMANAGER_UPSTREAM_CONNECT_TIMEOUT_SECS` | 上游连接超时；源码默认 15 秒 |
| `CODEXMANAGER_UPSTREAM_TOTAL_TIMEOUT_MS` | 整次上游请求总超时；源码默认 0，重试共享该期限 |
| `CODEXMANAGER_UPSTREAM_STREAM_TIMEOUT_MS` | 上游流处理超时；源码默认 300000 毫秒 |
| `CODEXMANAGER_DYNAMIC_OVERLOAD_ENABLED` | 启动时动态备用开关；源码默认 false，Compose 显式映射启用 |

以上是常用入口，不是全部上游设置的逐项索引。完整定义位于 `crates/service/src/app_settings` 和 `crates/service/src/gateway/core/runtime_config.rs`。调度和限额语义见 [SCHEDULING.md](SCHEDULING.md)；不要把提高工作线程数当成提高上游账号额度。

## 看板私有配置

使用 [部署步骤](DEPLOYMENT.md) 中的 `python -m dashboard.configure` 首次生成文件。配置保存在 `deploy/private`，不提交到仓库。

- `auth.json`：独立用户名、密码摘要、允许的精确 origin。公网 Tunnel origin 要同时存在于 `origins` 和 `cloudflare_tunnel_origins`。
- `collector.json`：脱敏标识密钥及显示别名；应在迁移时保留，避免公开 ID 改变。
- `login.local.txt`：仅供管理员保存的初始明文登录说明。

查看账号、上游账号和平台 API Key 相互独立。主动更改看板密码后重启 dashboard 使旧会话失效，升级和迁移不需要重新初始化。snapshot 可重新生成；统计日界线、分组归属和模型允许列表见 [架构](../ARCHITECTURE.md)。
