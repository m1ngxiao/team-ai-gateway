import type { MessageCatalog } from "../types";

export const EN_API_KEYS_MESSAGES: MessageCatalog = {
  "Gateway access": "Gateway access",
  项目: "Project",
  "Token / 金额": "Token / Amount",
  已花费: "Spent",
  不限额: "Unlimited",
  已达上限: "Limit reached",
  管理员视图: "Admin view",
  成员视图: "Member view",
  上游池: "Upstream pool",
  请选择上游池: "Select an upstream pool",
  需检查路由: "Review route",
  "此 Key 升级后需要重新选择上游池，保存后才能启用。":
    "Choose this key's upstream pool again after the upgrade and save it before enabling the key.",
  "OpenAI 上游池": "OpenAI upstream pool",
  "Claude API 池": "Claude API pool",
  "Claude 订阅账号池": "Claude subscription account pool",
  "Claude 上游": "Claude upstream",
  "Claude 订阅账号池（Messages / Responses）": "Claude subscription pool (Messages / Responses)",
  "Claude API 聚合（依上游能力）": "Claude API aggregation (upstream-dependent)",
  "Claude 订阅池支持 POST /v1/messages、POST /v1/responses、本地估算的 POST /v1/messages/count_tokens 和 GET /v1/models；不支持 POST /v1/chat/completions。":
    "The Claude subscription pool supports POST /v1/messages, POST /v1/responses, locally estimated POST /v1/messages/count_tokens, and GET /v1/models; POST /v1/chat/completions is not supported.",
  "Claude API 聚合的可用接口取决于所选上游；请按实际模型路由和上游协议接入。":
    "Available endpoints for Claude API aggregation depend on the selected upstream. Configure clients for its model routes and protocol.",
  "Claude 上游仅支持账号轮转或聚合 API 轮转": "Claude supports account or API upstream rotation only",
  "Claude API 轮转使用已启用的 Claude API 上游；与订阅账号池和 OpenAI 池隔离。":
    "Claude API rotation uses enabled Claude API upstreams, separate from subscription and OpenAI account pools.",
  "Claude 订阅账号池仅支持账号轮转": "The Claude subscription pool supports account rotation only",
  "绑定模型没有可用的 Claude 账号路由": "The bound model has no Claude account route",
  "Claude 平台 Key 只在已启用的 Claude 订阅账号间轮转，与 OpenAI 账号池隔离。":
    "Claude platform keys rotate only among enabled Claude subscription accounts, separate from the OpenAI account pool.",
  "优先 Claude API 上游": "Preferred Claude API upstream",
  "请选择可用的 Claude API 上游": "Select an available Claude API upstream",
  "绑定模型没有可用的 Claude API 路由":
    "The bound model has no available Claude API route",
  "此 Claude Key 的绑定模型由管理员管理，当前只能查看。":
    "The bound model for this Claude key is managed by an administrator and is read-only here.",
  "尚无可用的 Claude API 上游，请先在聚合 API 页面添加并启用。":
    "No Claude API upstream is available. Add and enable one on the Aggregate API page first.",
  "仅可选择已启用的 Claude 上游；绑定模型只显示有可用 Claude 路由的模型。":
    "Only enabled Claude upstreams can be selected. Bound model options have an available Claude route.",
  "用于复用固定的客户端 Key；填写后将按该值创建平台密钥，留空则继续随机生成。":
    "Reuse a fixed client key or leave blank to generate one randomly.",
  "请选择平台 Key 归属成员": "Select the member owner for this platform key",
  账号组筛选: "Account group filter",
  账号计划筛选: "Account plan filter",
  账号分组筛选: "Custom account group filter",
  全部分组: "All groups",
  "仅在选中的自定义账号分组内轮转；与账号计划筛选同时设置时，账号必须同时满足两项条件。":
    "Rotate only within the selected custom account group. When a plan filter is also set, accounts must match both filters.",
  "尚未配置账号分组。请先在 OpenAI 账号池中编辑账号并填写分组。":
    "No account groups are configured. Edit an account in the OpenAI account pool and assign a group first.",
  "额度分发开启时，平台 Key 必须归属到一个成员钱包。":
    "When quota distribution is enabled, the platform key must belong to a member wallet.",
  "未开启额度分发时可先不分配，开启后再补齐归属。":
    "When quota distribution is not enabled, you may leave this unassigned and fill in ownership later.",
  "总额度限制 (Token，可选)": "Total quota limit (tokens, optional)",
  不填表示不限制: "Leave blank for no limit",
  K: "K",
  M: "M",
  "达到上限后，这把平台密钥的新请求会被拒绝；已在途请求会按完成后的真实用量继续统计。":
    "After the limit is reached, new requests using this platform key will be rejected. In-flight requests continue to be counted by their final actual usage.",
  按: "By",
  参考估算: "Reference estimate",
};
