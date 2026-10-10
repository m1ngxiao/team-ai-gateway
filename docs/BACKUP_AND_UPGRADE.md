# 备份、迁移与升级

主服务沿用 CodexManager v0.6.0 的数据库布局及兼容的服务名称。项目镜像版本 `0.1.0` 描述这个独立发行版，不代表可把数据库随意降级为任意旧版 CodexManager。数据库打开时可能自动执行迁移；回退应同时使用对应版本的镜像和一致的数据备份。

## 需要保存的内容

| 路径 | 说明 |
| --- | --- |
| `deploy/data/gateway/codexmanager.db` | 账号、授权 token、平台 Key、设置和用量；完整数据库是敏感备份 |
| `deploy/data/gateway/codexmanager.rpc-token` | 服务与 Web 通信身份；与数据库部署一并保管 |
| `deploy/private/auth.json` | 可选看板密码摘要与允许的 origin |
| `deploy/private/collector.json` | 脱敏 ID 密钥和自定义显示名称；保留可使公开 ID 稳定 |
| `deploy/private/login.local.txt` | 可选明文看板登录说明，仅管理员私密保存 |
| `deploy/.env` | 本地域名、端口、资源与代理配置，可能包含代理凭据 |
| Tunnel/反向代理配置及凭据 | 由宿主部署单独管理，可能位于仓库以外 |

`deploy/data/dashboard/snapshot.json` 可以重新生成，不必备份。日志是否保留按团队需求决定。不要复制镜像缓存、`target`、`node_modules` 或构建产物作为数据迁移步骤。

启用了宿主维护工具时，还需保存它们的私有配置：模型同步的探测 Key、状态目录及 systemd units；代理维护的订阅文件、完整 Mihomo 配置、美国出口认证清单和选择记录。这些路径由操作者配置，通常位于仓库之外。升级/迁移期间先停止模型同步 timer 与手动代理更新，恢复完成后再重新启用，避免维护任务与切换同时改动状态。

## 一致 SQLite 备份

`scripts/Backup-CodexManagerDatabase.py` 使用 SQLite backup API 和完整性检查，可以对运行中的数据库生成一致备份，不依赖直接复制 WAL。目标使用排他创建，已存在时失败，文件权限为 600；失败的部分文件保留供管理员检查。先创建仓库之外的私有目录，选择尚不存在的文件名：

```bash
sudo install -d -m 700 /var/backups/team-ai-gateway
sudo python3 scripts/Backup-CodexManagerDatabase.py \
  deploy/data/gateway/codexmanager.db \
  /var/backups/team-ai-gateway/before-upgrade-001.db
```

备份包含完整账号授权和 Key，终端只打印备份路径和完成状态。不要上传至 GitHub、问题单或共享日志。备份工具只备份数据库；私有 JSON、RPC token、env 及 Tunnel 配置需另行保存到受控位置。升级切换前应停止流量并停止服务，再记录其他文件和版本，避免跨文件备份时应用同时更新状态。

`scripts/Inspect-CodexManagerDatabase.py` 使用只读连接，输出完整性状态、有限表的数量和版本检查布尔值，不输出账号、Key 或设置内容：

```bash
sudo python3 scripts/Inspect-CodexManagerDatabase.py \
  /var/backups/team-ai-gateway/before-upgrade-001.db --require-v060
```

这是当前兼容版本的结构检查，不是任意 SQLite 数据库的通用检查器，也不验证上游 token 是否有效。`--require-empty` 仅用于真正的空库验收；不要用于已有数据迁移。

## 升级同一台服务器

1. 记录当前仓库提交、镜像 ID、本地配置和运行组件；把旧镜像标为独立回退名称，避免构建相同标签后丢失定位信息。
2. 保留 `deploy/data`、`deploy/private` 和本地 `.env`。获取新源码并阅读版本说明；不运行看板重新初始化或改密码。
3. 可先构建新镜像；这不会替换运行容器。若希望切换前完整验证，在独立空数据目录和独立端口测试，不把真实账号副本同时联网运行。
4. 暂停入口流量，等待当前请求结束，再停服务。部署了看板时使用 `sudo bash scripts/server.sh --env-file deploy/.env --dashboard stop`；只有主服务时去掉 `--dashboard`。
5. 完成最终一致备份和私有配置备份，确认旧镜像可用。
6. 运行相应 `check` 和 `up`。`up` 不会构建镜像，请先完成 `build`。主服务通过健康检查后，检查管理员登录、账号/Key 数量、历史统计和自己的客户端请求。

gateway 内主服务与 Web 共享单个镜像并由 launcher 管理。collector 和 dashboard 共用另一个镜像；其 snapshot schema 需要匹配。后续若发行说明要求分阶段更新，应依说明执行，不能假定任意版本互相兼容。

新增的平台 Key `upstreamProvider` 字段默认是 `openai`。迁移时，旧 Key 若采用 `aggregate_api_rotation` 且固定了 Claude AggregateApi，会先标记为 `claude`，供管理员识别。旧版按请求路径选择候选池，而且显式固定的上游失败后可能继续尝试其他厂商；因此只要库中存在原生 Claude AggregateApi，所有聚合或混合轮转 Key 都会设为 disabled 并标记“需检查路由”。若没有原生 Claude，但同时存在 `compatible` 与 Codex 类上游，未固定活跃 Codex 类上游的聚合或混合 Key 也会标记，避免 `/v1/messages` 升级后尝试不同的候选。迁移保留 Key、密钥和其他配置，不会让上述请求静默转向另一厂商。账号轮转等未受影响的旧 Key 保持 `openai`。

升级验收时在后台检查标记“需检查路由”的 Key，包括升级前已 disabled 的 Key。管理员需在编辑页主动重新选择上游池并保存，才能启用；仅修改名称或成员直接点“启用”不会清除标记。继续使用 Claude API Key 上游时，选择 `claude` + `aggregate_api_rotation`，绑定启用的 Claude AggregateApi，并确认模型有 Claude 聚合路由。新建 Claude 订阅账号池 Key 时选择 `claude` + `account_rotation`，在模型目录配置 `account_pool/claude` 路由，并单独授权 Claude 账号。原本同时依赖 OpenAI 账号和 Claude 聚合的混合 Key，需明确选择单一上游池，或拆成两个平台 Key 分别使用。迁移不会把既有 API 上游凭据转换为 Claude.ai 订阅账号授权。

## 从已有部署迁移

先在目标服务器构建镜像、创建空数据目录并验证网络条件。源部署若使用旧路径 `deploy/linux/data/codexmanager`，迁入新路径 `deploy/data/gateway`；旧 `deploy/linux/private` 对应新 `deploy/private`。Docker 容器内数据库文件名仍是 `codexmanager.db`。

切换时停止旧入口、等待请求结束，并停止所有可能刷新 token 或写数据库的旧进程。生成最终 SQLite 备份，将其作为新目录中的 `codexmanager.db`，保留 RPC token 和看板配置；如果目标已经存在数据库，不覆盖它，应先另存并确认选用哪一份。**不要将新的备份数据库与另一份旧的 `-wal`/`-shm` 文件混用**；使用全新的停止状态目录进行恢复。数据及私有文件最终须归 UID/GID 10001，私有目录 700、私有文件 600；部署脚本不会代为修改已有数据权限。

重新检查代理、域名、账号专属代理和资源配置。服务器不会自动继承个人电脑 VPN。域名变化后重建 gateway；看板 `auth.json` 的允许 origin 也要同步，不通过重新初始化来更新它。仅启动目标实例并完成验收后再切入口，不同时运行两个含真实授权的 SQLite 副本。

## 回退

停止新的入口和所有写入进程，先私密保留升级后的数据库与配置。使用旧版本镜像和升级前的一致备份恢复到空目录；确认没有残留其他版本的 WAL，再启动旧版本。迁移后的新增记录不会自动回到旧备份，已经刷新过的上游授权也可能使旧备份失效，因此不能保证无损回退。需要再次人工授权时按账号提供方的流程处理。

如果仅回退看板，先停 collector 与 dashboard，使用相同版本的 collector 重新生成 snapshot，再启动相应 dashboard；主服务数据库不应为看板回退而恢复旧备份。
