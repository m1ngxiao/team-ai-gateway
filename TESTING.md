# 测试与验证

所有命令从仓库根目录执行，除非明确指定其他目录。先完成 [开发环境准备](DEVELOPMENT.md#工具与依赖)。标准测试使用临时数据库、合成凭据、进程内客户端或回环地址上的模拟上游，不要求真实账号，不调用真实模型。依赖和工具链下载需要网络，因此“本地模拟测试”并不表示首次安装可以断网。

## 管理界面与 Rust workspace

按顺序执行：

```bash
pnpm -C apps install --frozen-lockfile
pnpm -C apps exec node --test --test-concurrency=1
pnpm -C apps run build
test -s apps/out/index.html
cargo test --locked --release --workspace -- --test-threads=1
```

前端运行时测试覆盖 API/RPC 传输、运行模式、公开地址校验、路由与客户端状态处理。静态构建检查生产导出是否成功。Rust 完整 workspace 测试覆盖存储迁移、账号与 Key 权限、协议转换、流式响应、路由与重试、统计、启动器及 Web 代理。

Web 测试需要实际导出的 `apps/out`；不要用空目录替代，也不要在 UI 未构建时宣称嵌入资源验证成功。Rust 测试使用单测试线程以避免共享环境状态干扰；不添加过滤条件或跳过失败测试来获得通过结果。完整测试耗时依机器而异，CI 为前端与 Rust 作业预留 90 分钟，限制 Cargo 并行编译为 2。

调试期间可以选择有关测试，但提交结果须明确命令及过滤条件。例如仅检查某个 crate 不等同于完成上述 workspace 检查。测试数量会随代码变化；以对应提交的实际日志为准。

## 团队看板

Python 3.12 是本项目验证版本；服务使用 `asyncio.timeout`，不要使用 Python 3.10 的测试结果替代运行环境验证。

```bash
.venv/bin/python -m pip install -r services/dashboard/requirements-dev.txt
PYTHONPATH=services/dashboard .venv/bin/python -m pytest -q services/dashboard/tests
node --test --test-concurrency=1 services/dashboard/tests/test_frontend.mjs
```

覆盖只读 SQLite 列白名单、快照脱敏、账号/Key/模型展示、按当前 Key 分组汇总、时间窗口、登录与 CSRF、Origin 和 Tunnel 校验、会话失效、初始化文件保护，以及密码轮换时采集配置不变。Node 测试覆盖页面渲染、排序、视图切换和只读交互。

## 部署与发布输入

```bash
.venv/bin/python -m pytest -q scripts/tests
bash -n scripts/server.sh
docker compose --env-file deploy/.env.example \
  -f deploy/compose.yml --profile dashboard config --quiet
```

部署测试验证回环监听、UID/GID、只读挂载、采集器禁网、明确的配置文件选择，以及初始化不覆盖数据、不跟随符号链接。Compose 命令只解析公共示例，不构建镜像、不启动容器。CI 还把解析结果交给 `scripts/deployment.py` 的 `validate_config`，核对实际模板与这些约束一致。

发布扫描读取 Git index 中全部文件的 blob，包括已提交文件；它不使用尚未暂存的工作区内容替代扫描。准备提交时先检查差异，再暂存要发布的文件并执行：

```bash
git diff --cached --stat
python3 scripts/check-publication.py
```

扫描会拒绝未跟踪的非忽略文件、存在但未暂存的关键源码入口、敏感文件、凭据模式、私有目录路径和未经审阅的二进制。报告仅含路径、行号和类别。发布者可以在命令行补充组织私有域名与 IPv4 网段；这份名单留在仓库之外，不提交真实部署标识。例如以下值仅为示例：

```bash
python3 scripts/check-publication.py \
  --deny-domain gateway.example.internal \
  --deny-domain admin.example.internal \
  --deny-network 192.168.20.0/24
```

两个参数均可重复。域名规则覆盖该域名及其子域名，按完整域名边界匹配；网段规则仅匹配属于指定 CIDR 的 IPv4 地址。通用 CI 不传组织私有参数，仍执行所有默认凭据、路径与发布输入检查。

合成测试数据例外必须在 `scripts/publication-allowlist.json` 中绑定精确路径、类别与匹配文本的 SHA-256；静态图标绑定整个文件的 SHA-256，并接受路径、类型和大小检查。不存在对测试目录或所有二进制的统一豁免；内容变化、重复例外或失效例外均需要处理。

该检查是一层模式检测，不能证明文件不存在任何敏感内容。每次提交仍须人工审阅新增配置、示例、资源和差异；真实凭据应从拟发布内容中移除。不要为了让扫描通过而为真实凭据添加例外。

## 可选浏览器测试与真实环境验证

管理界面另有 Playwright 浏览器测试：

```bash
pnpm -C apps exec playwright install chromium
pnpm -C apps run test:e2e
```

这些测试启动本地静态服务器并使用测试中的模拟接口，需要可运行的 Chromium；它们不在当前基础 CI 的必跑列表。必要时使用 `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH` 指定已有浏览器。保留实际执行结果，不把运行时单元测试等同于浏览器验证。

真实上游连通性、模型调用、生产迁移和部署验收属于单独的人工操作，必须有明确授权、已获准使用的账号与目标环境。它们可能消耗额度或改动持久状态，不由测试脚本或 CI 自动触发。

## CI 范围

[CI 工作流](.github/workflows/ci.yml) 在 `main` 推送、pull request 和手动触发时运行三个作业：前端与完整 Rust workspace、团队看板、部署与发布检查。使用 Ubuntu 24.04 和开发指南中的版本。工作流仅授予仓库内容读取权限，不使用 `pull_request_target`，不读取生产凭据，不发布 release、不推送镜像、不自动部署。
