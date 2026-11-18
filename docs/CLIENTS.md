# 客户端接入

客户端需要管理员分配的**平台 API Key**、API Base URL 和允许使用的模型名称。Use the API endpoint and a platform key; administrator and dashboard credentials are separate.

## 地址与协议

| 客户端类型 | 配置 |
| --- | --- |
| OpenAI 兼容客户端 | Base URL 默认 `http://127.0.0.1:48760/v1`；公网示例 `https://api.example.com/v1` |
| Responses | `POST /v1/responses`，可使用 SSE |
| Chat Completions | `POST /v1/chat/completions`，可使用 SSE |
| Anthropic 兼容客户端 | API origin 为 `https://api.example.com`，请求路径 `/v1/messages` |
| 模型列表 | `GET /v1/models`，实际结果由当前 Key 和模型配置决定 |

不同客户端有的要求填写含 `/v1` 的 Base URL，有的自动添加路径；以最终请求路径为准，避免出现 `/v1/v1`。不要使用后台 `48761` 或看板 `48763` 端口作为模型 API。

协议兼容不保证所有提供方专属功能均可透传。管理员需要给 Key 配置正确的协议、模型和路由。`upstreamProvider=claude` 且 `rotationStrategy=account_rotation` 的 Key 使用 Claude 订阅账号池；`aggregate_api_rotation` 使用原有 Claude API Key 聚合上游。OpenAI 与 Claude 账号池互不回落。上游池设置见[配置参考](CONFIGURATION.md#平台-key-的上游池)。

### Claude 订阅账号池的接口范围

上表列的是中转站整体入口。Claude 订阅账号池 Key 当前只支持以下接口：

| 接口 | 说明 |
| --- | --- |
| `POST /v1/messages` | Anthropic Messages 请求；支持流式响应 |
| `POST /v1/responses` | Responses 请求；由中转站转换为 Claude Messages，支持流式响应 |
| `POST /v1/messages/count_tokens` | 中转站本地估算 Token 数，不是 Claude 官方精确计数 |
| `GET /v1/models` | 返回当前 Key 可见的 Claude 账号池模型 |

这个池**不支持** `POST /v1/chat/completions`。接入时应使用发送 Messages 或 Responses 请求的客户端。Claude API Key 聚合上游的接口范围由其上游协议能力决定。

订阅账号请求会在原有系统指令前加入 Claude Code 的兼容性系统文本块。客户端的系统指令仍按原顺序保留，但上游看到的系统上下文会多出这一块；依赖系统提示完全原样透传的客户端需要注意。

### Claude Code Auto Mode 的 server-side safeguards

原生 Claude 路由会原样保留请求体中的未知字段、重复的 `anthropic-beta` 值和其他非凭据 Claude Code 请求头；非流式响应与 SSE 帧中的 `safeguard_results` 也按原始 JSON/事件顺序返回。`tool_use.id` 和 `tool_result.tool_use_id` 不参与改写。

OpenAI/Codex 账号池默认启用 safeguards 仿真。网关先对 OpenAI 上游剥离 Anthropic 私有 `safeguards` 字段，再用当前账号的上游通道调用分类模型，最后把 Anthropic 形状的 `safeguard_results` 注入非流式响应或流式 `message_delta`。分类请求默认复用当前模型；可在后台环境变量中设置 `CODEXMANAGER_CLAUDE_SAFEGUARDS_CLASSIFIER_MODEL` 指定单独模型。设置 `CODEXMANAGER_CLAUDE_SAFEGUARDS_EMULATION_ENABLED=0` 可以关闭仿真，此时 OpenAI 路由恢复明确的 capability 拒绝。

这是协议与能力仿真，不是 Anthropic 原生 classifier。分类器超时、上游失败、输出无法解析或工具调用在 `max_tokens` 处被截断时，网关写入 `unavailable`，不会伪造 `evaluated` 结果。原生 Claude 路由仍直接使用 Anthropic 返回的真实分类结果。

### Claude Code context management 与 `/compact`

`/compact` 不应被当成 `/v1/responses/compact` 路径推断。Claude Code 会在原生 Messages 请求中使用 `context_management`，可能包含 `clear_tool_uses_20250919` 和 `compact_20260112`。OpenAI-compatible 路由现在由网关执行这些编辑：

- `clear_tool_uses_20250919` 按工具调用顺序清除较旧的 `tool_result`，保留最近的完成结果，并可按 `clear_tool_inputs` 清空对应工具输入。
- `compact_20260112` 默认在估算输入超过 150,000 tokens 时调用摘要模型；摘要成功后会丢弃摘要点之前的原始历史，注入 `Previous conversation summary`，并在响应中返回 `compaction`、`context_management.applied_edits` 和 compaction iteration usage。
- 已有客户端 `compaction` 块会作为历史边界处理，不会作为未知 block 发送给 OpenAI。摘要调用受 60 秒超时保护，失败时主请求继续，但通过 `applied_edits[].error = summary_call_failed` 明确报告未压缩。

摘要模型默认为当前请求模型，可通过 `CODEXMANAGER_CLAUDE_COMPACT_SUMMARY_MODEL` 单独指定；`CODEXMANAGER_CLAUDE_COMPACT_SUMMARY_MAX_TOKENS` 默认 4096。`CODEXMANAGER_CLAUDE_CONTEXT_MANAGEMENT_ENABLED=0` 可关闭仿真。token 数使用网关的本地估算器，生产分类/计费仍以各上游返回的 usage 为准。

对 `providerType=codex` 的聚合 API，网关会在 Claude Code 的 `/v1/messages` 请求上主动做 Messages→Responses 转换，因此不再依赖中转上游自己实现 Anthropic 私有字段。`compatible` 类型仍按原生 Messages 透传，避免把明确声明为多协议的 Anthropic 端点错误改写；如果该候选实际只提供 OpenAI Responses，应把它配置为 `codex`。

### Claude Code Artifact 兼容

Claude Code 内置 Artifact 要求 claude.ai Pro、Max、Team 或 Enterprise 登录，并且模型提供方必须是 Anthropic API；网关 token、API Key、Bedrock、Vertex 和 Foundry 都不能启用内置 Artifact。因此 GPT 平台 Key 不能启用客户端原生 Artifact，即使模型协议转换完全正确。

网关提供自托管 Artifact/MCP 兼容能力：

| 接口 | 用途 |
| --- | --- |
| `POST /v1/artifacts` | 发布或更新 HTML Artifact，平台 Key 鉴权 |
| `GET /v1/artifacts` | 列出当前 Key 拥有的 Artifact |
| `GET /v1/artifacts/{slug}` | 读取元数据 |
| `DELETE /v1/artifacts/{slug}` | 删除 |
| `POST /v1/code/mcp/ccr-artifacts` | 供 Claude Code HTTP MCP 使用的 JSON-RPC 端点 |

发布请求示例：

```bash
curl --fail-with-body "$TEAM_GATEWAY_ORIGIN/v1/artifacts" \
  -H "Authorization: Bearer $TEAM_GATEWAY_API_KEY" \
  -H 'Content-Type: application/json' \
  --data '{"content":"<!doctype html><title>Review</title><h1>Review</h1>","favicon":"📄"}'
```

把 MCP 端点加入 Claude Code：

```bash
claude mcp add --transport http team-artifacts \
  https://codex.example.com/v1/code/mcp/ccr-artifacts \
  --header "Authorization: Bearer $TEAM_GATEWAY_API_KEY"
```

实际路径以 API origin 为基准，应为 `${TEAM_GATEWAY_ORIGIN}/v1/code/mcp/ccr-artifacts`。MCP 首选与 Claude Code 同形的单工具 `Artifact`：`action` 支持 `publish`（默认）、`read`、`list`、`delete`；同时保留 `publish_artifact`、`read_artifact`、`list_artifacts` 和 `delete_artifact` 兼容别名。远程 MCP 无法读取客户端本机路径，因此发布参数是完整 `content`，不是 `file_path`；模型应先用客户端 Read 工具读取本地 HTML/Markdown。页面保存在 `CODEXMANAGER_ARTIFACTS_DIR`，默认是数据库目录下的 `artifacts/`；返回链接包含随机 view token。

Artifact 工具 schema 中的 `\0` 和 Unicode property regex 会在发送给严格 OpenAI schema 校验器前移除。客户端仍在本地校验文件路径和参数，这一清理只避免上游拒绝整个模型请求。

`GET /v1/models` 同时返回 OpenAI 与 Claude Code 可读取的模型字段，包括 `type=model`、`context_window`、`max_input_tokens`、`display_name` 和分页字段；`GET /v1/models/{id}` 也由网关本地响应，避免 Claude Code 为确认上下文窗口访问不存在的上游路径。

如果没有启用自托管 Artifact MCP，并希望避免模型尝试客户端内置 Artifact，可在 Claude Code 设置中关闭：

```json
{"enableArtifact": false}
```

## 用 curl 检查连接

先在当前终端读取自己的平台 Key，输入不会回显：

```bash
read -rs -p 'Platform API Key: ' TEAM_GATEWAY_API_KEY
printf '\n'
TEAM_GATEWAY_ORIGIN=http://127.0.0.1:48760
curl --fail-with-body "$TEAM_GATEWAY_ORIGIN/v1/models" \
  -H "Authorization: Bearer $TEAM_GATEWAY_API_KEY"
```

从返回列表选择允许的模型；下面的 `YOUR_MODEL` 需要替换。SSE 示例：

```bash
curl --fail-with-body -N "$TEAM_GATEWAY_ORIGIN/v1/responses" \
  -H "Authorization: Bearer $TEAM_GATEWAY_API_KEY" \
  -H 'Content-Type: application/json' \
  --data '{"model":"YOUR_MODEL","input":"Reply with a short greeting.","stream":true}'
```

发送 Anthropic Messages 请求的 Key（包括 Claude 订阅账号池 Key）可使用：

```bash
curl --fail-with-body -N "$TEAM_GATEWAY_ORIGIN/v1/messages" \
  -H "x-api-key: $TEAM_GATEWAY_API_KEY" \
  -H 'anthropic-version: 2023-06-01' \
  -H 'Content-Type: application/json' \
  --data '{"model":"YOUR_MODEL","max_tokens":64,"messages":[{"role":"user","content":"Hello"}],"stream":true}'
unset TEAM_GATEWAY_API_KEY
```

这些生成请求会实际调用配置的上游并消耗相应额度。部署健康检查不会替你执行它们。

## 常见问题

- **浏览器能登录后台，客户端却得到 HTML 登录页：** 使用了后台域名，或 API 域名前设置了交互式 Access 登录。使用专门的 API origin。
- **模型不可见或不可用：** 检查平台 Key 的模型、账号分组和路由；不要根据其他成员的模型列表推断自己的权限。
- **429：** 可能是平台 Key 累计 Token 限额已耗尽，也可能来自上游限流；查看错误正文和管理员日志定位。
- **503 或容量错误：** 候选账号、额度、并发和安全重放条件可能不足。动态备用不能解决所有上游同时拥堵。
- **流式响应中途失败：** 已交付内容的请求不会透明换号重放；客户端需要决定是否发起新的完整请求。

客户端重试会形成新的请求。使用有限退避，不要在容量失败时无限并发重试。并发、会话和预算限制见 [调度说明](SCHEDULING.md)。

自动压缩通常仍使用模型生成接口，不能从 `/compact` 命令推断 HTTP 路径或流式模式。Messages 接到 OpenAI 账号池时会转换成 Responses；上游 SSE 注释心跳转换为 Messages `ping`，不产生摘要或用量。若上游在 HTTP 200 后尚未发出首个 SSE 事件，Messages 转换流也会按保活间隔先发送 `ping`，避免 Cloudflare 等入口因首帧等待时间过长返回 524；该保活不伪造模型内容，也不计入首响应或用量。转换流遇到读取失败、空闲超时、异常 EOF 或失败事件时发送 `error` 并记录交付失败，不再生成成功的 `message_stop`；正常完成时立即发送结束事件，输出上限结束映射为 `max_tokens`。

启用 `CODEXMANAGER_GATEWAY_TRACE_STDOUT=1` 并允许 `info` 级别日志后，`MESSAGES_INGRESS` 即时记录转换前的 `client_stream`（含 `omitted`）、`max_tokens` 与可用的压缩提示字段，使用同一个 `trace_id` 对齐后续 `REQUEST_START`、`ATTEMPT_RESULT`、`BRIDGE_RESULT`。会话 ID 仅记录 SHA-256 摘要前 16 位，不记录会话正文。支持 gateway hint headers 的 Claude Code 在自定义 `ANTHROPIC_BASE_URL` 下可按该版本说明开启 `CLAUDE_CODE_GATEWAY_HINT_HEADERS=1`；缺少提示字段不能排除压缩请求，仍需客户端版本、失败时间和调试日志进行关联。此日志不测量 TCP 连接时间、首字节或流内最大间隔，`first_response_ms` 也不能当作首个有效模型内容时间。
