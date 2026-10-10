# 团队只读用量看板

独立的 SQLite 采集器、FastAPI 服务与静态页面。看板分别展示 OpenAI 和 Claude.ai 订阅账号池、最近成功采集的额度、平台 Key、模型目录、成员与分组用量；不能管理账号、修改密钥或发起模型请求。默认品牌为 Team AI Gateway，部署时可编辑 `dashboard/static/index.html` 中的标题与品牌。

## 本地运行

Python 3.12；依赖按 `requirements.txt` 的固定版本安装。以下命令均在 `services/dashboard` 目录执行：

```sh
python3 -m venv .venv
.venv/bin/python -m pip install -r requirements.txt
.venv/bin/python -m dashboard.configure \
  --source-db /srv/team-ai-gateway/data/codexmanager.db \
  --private-dir ./private --username team --prompt-password
.venv/bin/python -m dashboard.collector \
  --config ./private/collector.json --output ./data/snapshot.json --once
```

初始化通过终端输入密码，只向私有文件写入登录信息。`private/auth.json` 保存密码哈希和允许的浏览器 Origin，`private/collector.json` 保存数据库路径、匿名 ID 盐和账号别名；`private/login.local.txt` 含明文登录信息，必须保密。不要提交 `private/`、数据库或快照。生成目录权限为 0700，私有文件为 0600。

首次初始化时，上述三个文件中任一个已存在（包括符号链接或不完整初始化遗留文件）都会直接拒绝操作，不读取数据库或覆盖已有配置。应先保留并恢复已有配置；`--rotate` 需要完整且非符号链接的 `auth.json`，仅轮换查看者登录信息，保留采集器的匿名 ID 盐与别名。

在两个终端分别启动采集器与服务：

```sh
.venv/bin/python -m dashboard.collector \
  --config ./private/collector.json --output ./data/snapshot.json
```

```sh
DASHBOARD_AUTH_FILE=./private/auth.json \
DASHBOARD_SNAPSHOT=./data/snapshot.json \
  .venv/bin/python -m uvicorn dashboard.server:create_app \
    --factory --host 127.0.0.1 --port 48763 --workers 1 \
    --no-proxy-headers --no-access-log
```

访问 `http://127.0.0.1:48763`。会话保存在进程内，使用单 worker；重启服务使既有会话失效。`--rotate --private-dir ./private --prompt-password` 可轮换查看者密码，随后只需重启看板服务。

采集器默认约每 60 秒更新；`--source-db` 可覆盖配置里的路径，供容器使用。连接以只读模式打开数据库，并按表和列白名单限制查询。运行时必须能读取 SQLite WAL/SHM 文件；容器应只读挂载数据库所在目录，不要只挂载主数据库文件。快照通过原子替换更新，故障时保留上一份结果。网页对 180 秒以上快照提示延迟，24 小时以上拒绝展示。

## 对外访问与配置

`--public-origin https://stats.example.com` 为 Cloudflare Tunnel 部署初始化明确的 HTTPS Origin，不能带路径、查询参数或末尾斜杠。仅在来源不可被公网直接访问、且经过受信 Tunnel 时使用此选项；服务会验证转发协议，并拒绝通过 HTTP 提交凭据。一般 HTTPS 反向代理可单独配置 `auth.json` 的 `origins`，不要随意启用 `cloudflare_tunnel_origins` 或通用代理头信任。

浏览器必须使用配置中的精确 Origin 和端口；默认只接受本机 48763 端口。更换端口时同步修改 `origins`。`DASHBOARD_AUTH_FILE` 默认 `/run/dashboard/auth.json`，`DASHBOARD_SNAPSHOT` 默认 `/data/snapshot.json`。Dockerfile 的构建上下文是本目录，固定运行 UID/GID 为 10001，挂载文件应授予对应的读取权限。网络入口、挂载路径和外部域名均由部署者提供。

登录后的快照会展示经过验证的账号邮箱、成员 Key 名称、分组名和用量，适用于受信团队内部。它不包含完整 API Key、账号登录 Token、代理地址、上游连接信息或模型路由凭据，但不应把快照当作公开匿名数据。

## 统计口径

- 时间窗口固定为 `Asia/Shanghai`：今日从当地午夜起，近 7 天为今日与之前 6 个自然日；“累计记录”为仍保留的可统计明细与汇总记录，不承诺覆盖已删除的全部历史。
- 分组用量按照 **Key 当前的账号分组筛选**归属。调整某个 Key 的分组后，该 Key 的历史用量随之归入新组；这不表示历史请求实际由该组的上游账号承载。删除或无法关联的 Key 保留在历史未归属项，未限制分组的 Key 单列展示。
- `usage_included`、小时汇总和累计汇总决定统计范围；金额取自数据库估算值，不能作为上游最终账单。输入、缓存、输出与总 Token 按记录字段展示，不应另行相加缓存输入而重复计算。
- 上游池用量按请求记录中的 `actual_source_kind` 归因，仅把 `openai_account` 和 `claude_subscription_account` 计入对应订阅账号池。聚合 API、无来源的旧请求和缺少来源字段的历史汇总列为“未归属”；三项之和等于全站总量。Key 页面显示的是 Key 当前配置的上游厂商，不能据此回填历史请求的实际账号池。
- 旧数据库没有 Claude 账号表、额度表或上游来源字段时，看板仍可读取；缺少额度表时 Claude 额度显示“未知”，缺少来源的记录归入“未归属”。新服务读取旧快照时，分池用量和 Claude 账号数显示“待更新”，直到新版采集器生成快照。
- 模型从网关数据库目录动态读取，仅导出 `visibility=list` 且 slug 通过格式校验的条目；启停和 API 支持状态保留，不自动启用隐藏或禁用模型。Key 绑定和用量名称使用同一可见目录，未知及隐藏模型用量归入 `other`，缺失价格保持未知。上游模型同步由独立的宿主任务维护，看板采集器仍无网络，不发起模型调用。
- 账号额度来自上游额度快照，时间与请求用量记录可能不同步；看板本身不会为刷新额度发起模型请求。Claude 的 5 小时和 7 天剩余百分比仅依据最近成功观测值计算，超过 60 分钟或相应窗口已重置时显示“未知”，并保留最后成功采集时间和查询错误类别。本站记录的 Token 用量不能换算为 Claude 订阅额度。

## 测试

```sh
.venv/bin/python -m pip install -r requirements-dev.txt
.venv/bin/python -m pytest -q tests
node --test tests/test_frontend.mjs
```

测试使用临时合成数据库和进程内 HTTP 客户端，不需要真实账号、网络模型请求或生产数据库。
