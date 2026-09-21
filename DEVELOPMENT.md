# 开发指南

Team AI Gateway 直接维护仓库内的服务、管理界面和团队看板源码。当前支持 Linux/Web 部署；保留的 Tauri 代码用于兼容上游结构，桌面安装包不属于当前 CI 的验证范围。先阅读 [AGENTS.md](AGENTS.md)，修改前端时还需阅读 [apps/AGENTS.md](apps/AGENTS.md)。

## 工具与依赖

CI 使用 Ubuntu 24.04、Node.js 22、pnpm 10.30.3、Rust 1.98.0 和 Python 3.12。Rust 版本以根目录 `rust-toolchain.toml` 为准，前端包管理器版本以 `apps/package.json` 为准。Linux 构建需要 C/C++ 编译工具、pkg-config、OpenSSL 开发文件和 CMake；部署示例检查另外需要 Docker Compose v2。

在仓库根目录初始化开发依赖：

```bash
corepack enable
corepack prepare pnpm@10.30.3 --activate
pnpm -C apps install --frozen-lockfile
rustup toolchain install 1.98.0 --profile minimal --component rustfmt --component clippy
python3.12 -m venv .venv
.venv/bin/python -m pip install -r services/dashboard/requirements-dev.txt
```

保留 `Cargo.lock`、`apps/pnpm-lock.yaml` 和 Python 固定版本。依赖更新应作为可审阅的单独变更，不能通过去掉 `--locked` 或 `--frozen-lockfile` 绕过安装失败。首次下载工具链和依赖需要网络；测试使用本地模拟服务和临时数据，不需要真实上游账号。

## 目录与修改边界

| 目录 | 职责 |
| --- | --- |
| `apps/src` | 静态 Next.js 管理界面、类型化 API/RPC 调用和多语言文案 |
| `crates/core` | 存储、迁移、账号和用量数据结构 |
| `crates/rusqlite` | 本地 SQLite 兼容层 |
| `crates/service` | 网关、鉴权、路由、协议适配、账号池和请求统计 |
| `crates/web` | 嵌入式管理界面、Web 登录和服务代理 |
| `crates/start` | 启动服务与 Web 后台的 Linux 入口 |
| `services/dashboard` | 可选只读采集器和团队统计界面 |
| `deploy`、`scripts` | 可移植部署模板、验证与维护工具 |

新增或调整 RPC 时同步 Rust 分发、Web 传输封装和前端类型。迁移必须兼容已有数据库；修改鉴权、Key 权限、分组筛选、重试或协议转换时，补充对应行为测试。保持 `LICENSE` 和上游来源说明。

## 构建并运行本地服务

先构建 UI，再编译 Rust。`crates/web` 会将 `apps/out` 嵌入二进制，前端变更后必须重新构建两者。

```bash
pnpm -C apps run build
cargo build --locked --release \
  -p codexmanager-service -p codexmanager-web -p codexmanager-start
mkdir -p .dev/data .dev/private
CODEXMANAGER_DB_PATH="$PWD/.dev/data/codexmanager.db" \
CODEXMANAGER_RPC_TOKEN_FILE="$PWD/.dev/private/rpc-token" \
CODEXMANAGER_SERVICE_ADDR=127.0.0.1:48760 \
CODEXMANAGER_WEB_ADDR=127.0.0.1:48761 \
CODEXMANAGER_WEB_NO_OPEN=1 \
  ./target/release/codexmanager-start
```

管理后台为 `http://127.0.0.1:48761`，API 为 `http://127.0.0.1:48760/v1`。使用独立的开发数据目录，不引用生产数据库或凭据。上述数据与私有文件目录由 `.gitignore` 排除。后台访问控制的初始化与正式部署见 [部署文档](docs/DEPLOYMENT.md)。

需要前端热更新时，保持本地 Rust 服务运行，在另一终端执行：

```bash
CODEXMANAGER_DEV_WEB_ORIGIN=http://127.0.0.1:48761 \
  pnpm -C apps run dev --hostname 127.0.0.1
```

开发界面位于 `http://127.0.0.1:3000`，API/RPC 由 Next.js 开发代理转发到本地 Web 服务。单独启动 Next.js 不会启动网关。`NEXT_PUBLIC_API_ORIGIN`、`NEXT_PUBLIC_ADMIN_ORIGIN` 和 `NEXT_PUBLIC_STATS_ORIGIN` 是公开的构建期地址；不要在其中放入凭据。

团队看板的独立运行和初始化步骤见 [看板说明](services/dashboard/README.md)。统计按照 Key 当前分组归属历史用量；新增模型需要维护公开白名单。

## 提交变更

先运行与改动对应的快速测试，再完成 [TESTING.md](TESTING.md) 的完整检查。CI 不把历史代码的全量格式化或 Clippy 结果作为硬门槛；修改的代码仍应遵循周边风格，避免混入无关格式化。

提交前查看 `git diff` 和 `git diff --cached`，暂存准备发布的源码，然后运行 `python3 scripts/check-publication.py`。扫描输入是 Git index，工作区修改后需要重新暂存；真实密钥、私有文件、构建产物和数据库不得作为修复扫描失败的例外。Pull request 说明应写明问题、最终行为、执行的测试以及尚未验证的范围。

组织内部准备对外发布时，可通过重复的 `--deny-domain` 和 `--deny-network` 参数追加私有域名与 IPv4 网段检测。真实名单保存在仓库外；格式与通用示例见 [测试文档](TESTING.md#部署与发布输入)。
