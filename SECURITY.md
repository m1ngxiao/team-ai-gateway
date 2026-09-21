# 安全说明

本项目处理上游授权、平台 API Key 和团队请求。Protect deployment data as credentials, and keep administration separate from public API access.

## 部署边界

默认 Compose 只发布宿主回环端口。空库不会自动启用后台账号登录；先通过本机或 SSH 转发完成管理员设置，再配置公网入口。API 域名只暴露必要 `/v1` 路径，不透传管理 RPC。可选 Cloudflare 模板为后台校验 Access JWT，仍需应用自身登录；其他 HTTPS 代理应建立相应访问边界。

主服务、collector 和 dashboard 以 UID/GID 10001 运行，丢弃 Linux capabilities。collector 无网络且只读挂载源数据库，dashboard 只读取脱敏 snapshot。看板账号有独立密码和会话，不能用作后台管理或模型请求凭据。

## 数据与升级

SQLite 数据库、RPC token、看板私有 JSON、本地 `.env` 与备份都可能包含敏感信息。日志和导出文件在分享前需人工确认内容；发布扫描不是完整的数据泄露防护。定期验证受控备份能够恢复，按 [备份与升级](docs/BACKUP_AND_UPGRADE.md) 操作。

只接入自己拥有或已获授权的账号与上游。请求会经网关发送给实际配置的上游；选择第三方 API 时，应明确其数据处理范围。平台 Key 限额与路由策略不构成账号提供方额度或风控策略的保证。

保留的 Tauri 桌面源码默认仍使用上游更新来源。当前发行不支持通过该更新器升级本项目，应按 Linux/Web 源码构建流程升级。

## 报告漏洞

请提供影响版本、受影响组件、前置条件和脱敏复现步骤。若仓库 [Security 页面](https://github.com/m1ngxiao/team-ai-gateway/security) 提供私密漏洞报告入口，优先使用；若未开放，只创建不含利用细节或敏感数据的联系请求，等待维护者提供私密渠道。

不要在公开 Issue、PR、附件或 CI 日志提交真实数据库、token、完整 Key、个人账号信息或可直接攻击在用部署的材料。本项目尚未承诺固定响应时限；修复与可用版本会在仓库中记录。
