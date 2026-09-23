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
