# 团队中转站 Web 开发

`apps/` 提供团队中转站的管理界面：账号池、平台 API Key、模型与路由、请求日志和用量管理。发布目标是 Linux / Docker 上运行的 Web 服务。`src-tauri/` 和相关测试保留上游桌面兼容实现，本项目当前不发布桌面安装包，也不承诺桌面功能可用性。

## 工作区

- `src/app/`：Next.js App Router 页面。
- `src/components/`、`src/hooks/`：界面组件与交互逻辑。
- `src/lib/api/`：带类型的 API 客户端和 Web / Tauri transport。
- `src/lib/gateway/public-origins.ts`：共享公开地址配置与校验。
- `src/lib/i18n/`：中文、英文、韩文和俄文文案。
- `tests/`：Node 回归测试与 Playwright 端到端测试。
- `src-tauri/`：保留的桌面兼容源代码。
- `out/`：构建生成的静态页面，不提交到版本库。

技术栈为 Next.js 16、React 19、TypeScript、Tailwind CSS v4、Base UI、TanStack Query 和 Zustand。Next.js 使用静态导出与 `trailingSlash: true`。

## 本地开发

使用 Node.js 22 和 `package.json` 中固定的 pnpm 10.30.3。在仓库根目录运行：

```sh
corepack enable
pnpm -C apps install --frozen-lockfile
pnpm -C apps dev
```

开发页面默认位于 `http://localhost:3000`。完整管理功能需要先启动后端服务与 Web 壳，详见根目录部署说明。Next 开发服务器会把 `/api/runtime`、`/api/rpc`、`/api/events/*` 和登录会话接口代理到 `http://localhost:48761`。可以通过 `CODEXMANAGER_DEV_WEB_ORIGIN` 指向另一套开发 Web 服务；此变量只决定开发代理目标，不决定用户复制的公开 API 地址。

生产静态文件由 `codexmanager-web` 提供，必须保留其运行时、RPC 与登录会话接口。`next start` 不支持本项目的静态导出模式，不能替代 Web 壳。

## 公开服务地址

三个公开地址在 **构建时** 写入浏览器静态资源。平台接入页与客户端 API 地址生成逻辑共用同一配置，当前管理站的浏览器 origin 不会覆盖 API 地址。

| 前端构建变量 | Docker 构建参数 | 本地默认值 |
| --- | --- | --- |
| `NEXT_PUBLIC_API_ORIGIN` | `PUBLIC_API_ORIGIN` | `http://127.0.0.1:48760` |
| `NEXT_PUBLIC_ADMIN_ORIGIN` | `PUBLIC_ADMIN_ORIGIN` | `http://127.0.0.1:48761` |
| `NEXT_PUBLIC_STATS_ORIGIN` | `PUBLIC_STATS_ORIGIN` | `http://127.0.0.1:48763` |

配置应为 HTTPS origin；本地开发允许 HTTP loopback（`localhost`、`127.0.0.0/8` 或 `[::1]`）。不得含凭据、路径、查询参数或 fragment，允许末尾单个 `/`。显式空值和无效值会使构建失败；只有未设置的变量使用本地默认值。校验错误显示变量名，不打印输入内容。

示例：

```sh
NEXT_PUBLIC_API_ORIGIN=https://api.example.test \
NEXT_PUBLIC_ADMIN_ORIGIN=https://admin.example.test \
NEXT_PUBLIC_STATS_ORIGIN=https://stats.example.test \
pnpm -C apps build
```

Docker 构建把 `PUBLIC_*` 参数映射到上述 `NEXT_PUBLIC_*` 变量。修改公开域名后需要重新构建镜像或前端资源；仅修改容器启动环境不会改写已导出的页面。这些变量是公开链接，不是监听地址、管理认证配置或密钥，不要填写任何秘密。

## 页面与兼容边界

- 接入页描述共享 Docker 网关，展示 API、管理员后台和团队看板地址。
- 每位使用者通过自己的平台 API Key 调用模型；模型目录与路由根据服务端和密钥配置生效。
- 模型管理不提供写入或下载 `~/.codex/models_cache.json` 的入口。
- Skills 页面说明由客户端安装和管理 Skills / 插件；网关不修改客户端登录、配置目录或历史记录。
- 内部 API / RPC 命令、`CODEXMANAGER_*` 配置和数据库兼容标识保留，界面使用“团队中转站”品牌。

## 验证与贡献

```sh
pnpm -C apps test:runtime
pnpm -C apps build
pnpm -C apps lint
```

`test:runtime` 使用 Node 原生测试器，覆盖配置校验、API 客户端、会话与权限、页面约束、翻译完整性和保留的桌面兼容链路。`build` 进行生产静态导出和类型检查。端到端测试另用 `pnpm -C apps test:e2e`，需要安装 Playwright 浏览器并按相应配置启动服务。

`dev:desktop` 仅启动供 Tauri 使用的前端开发服务器；`build:desktop` 当前与 `build` 一样执行静态导出，均不生成桌面安装包。

修改前阅读本目录 [AGENTS.md](AGENTS.md)。新增接口应使用统一 transport 与 `src/lib/api/` 封装，保留服务端权限校验；新增界面文案需补齐所有语言。根目录 [README.md](../README.md) 提供整体架构和部署入口。
