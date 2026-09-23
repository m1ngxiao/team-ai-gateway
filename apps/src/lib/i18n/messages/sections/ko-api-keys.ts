import type { MessageCatalog } from "../types";

export const KO_API_KEYS_MESSAGES: MessageCatalog = {
  "Gateway access": "게이트웨이 접근",
  项目: "프로젝트",
  "Token / 金额": "Token / 금액",
  已花费: "사용됨",
  不限额: "무제한",
  已达上限: "상한 도달",
  管理员视图: "관리자 보기",
  成员视图: "멤버 보기",
  上游池: "업스트림 풀",
  请选择上游池: "업스트림 풀을 선택하세요",
  需检查路由: "경로 확인 필요",
  "此 Key 升级后需要重新选择上游池，保存后才能启用。":
    "업그레이드 후 이 키의 업스트림 풀을 다시 선택하고 저장해야 활성화할 수 있습니다.",
  "OpenAI 上游池": "OpenAI 업스트림 풀",
  "Claude API 池": "Claude API 풀",
  "Claude 订阅账号池": "Claude 구독 계정 풀",
  "Claude 上游": "Claude 업스트림",
  "Claude 订阅账号池（Messages / Responses）": "Claude 구독 풀 (Messages / Responses)",
  "Claude API 聚合（依上游能力）": "Claude API 집계 (업스트림 기능에 따름)",
  "Claude 订阅池支持 POST /v1/messages、POST /v1/responses、本地估算的 POST /v1/messages/count_tokens 和 GET /v1/models；不支持 POST /v1/chat/completions。":
    "Claude 구독 풀은 POST /v1/messages, POST /v1/responses, 로컬 추정 방식의 POST /v1/messages/count_tokens 및 GET /v1/models를 지원합니다. POST /v1/chat/completions는 지원하지 않습니다.",
  "Claude API 聚合的可用接口取决于所选上游；请按实际模型路由和上游协议接入。":
    "Claude API 집계에서 사용할 수 있는 엔드포인트는 선택한 업스트림에 따라 다릅니다. 실제 모델 경로와 업스트림 프로토콜에 맞게 클라이언트를 설정하세요.",
  "Claude 上游仅支持账号轮转或聚合 API 轮转": "Claude는 계정 순환 또는 API 업스트림 순환만 지원합니다",
  "Claude API 轮转使用已启用的 Claude API 上游；与订阅账号池和 OpenAI 池隔离。":
    "Claude API 순환은 활성화된 Claude API 업스트림을 사용하며 구독 및 OpenAI 계정 풀과 분리됩니다.",
  "Claude 订阅账号池仅支持账号轮转": "Claude 구독 풀은 계정 순환만 지원합니다",
  "绑定模型没有可用的 Claude 账号路由": "연결된 모델에 사용 가능한 Claude 계정 경로가 없습니다",
  "Claude 平台 Key 只在已启用的 Claude 订阅账号间轮转，与 OpenAI 账号池隔离。":
    "Claude 플랫폼 키는 활성화된 Claude 구독 계정 사이에서만 순환하며 OpenAI 계정 풀과 분리됩니다.",
  "优先 Claude API 上游": "우선 Claude API 업스트림",
  "请选择可用的 Claude API 上游": "사용 가능한 Claude API 업스트림을 선택하세요",
  "绑定模型没有可用的 Claude API 路由":
    "바인딩한 모델에 사용 가능한 Claude API 경로가 없습니다",
  "此 Claude Key 的绑定模型由管理员管理，当前只能查看。":
    "이 Claude 키의 바인딩 모델은 관리자가 설정하며 여기서는 확인만 할 수 있습니다.",
  "尚无可用的 Claude API 上游，请先在聚合 API 页面添加并启用。":
    "사용 가능한 Claude API 업스트림이 없습니다. 먼저 Aggregate API 페이지에서 추가하고 활성화하세요.",
  "仅可选择已启用的 Claude 上游；绑定模型只显示有可用 Claude 路由的模型。":
    "활성화된 Claude 업스트림만 선택할 수 있습니다. 바인딩할 모델은 사용 가능한 Claude 경로가 있는 모델만 표시됩니다.",
  "用于复用固定的客户端 Key；填写后将按该值创建平台密钥，留空则继续随机生成。":
    "고정된 클라이언트 키를 재사용하거나 비워 두어 무작위로 생성합니다.",
  "请选择平台 Key 归属成员": "platform key의 소속 구성원을 선택하세요",
  账号组筛选: "계정 그룹 필터",
  账号计划筛选: "계정 플랜 필터",
  账号分组筛选: "사용자 지정 계정 그룹 필터",
  全部分组: "모든 그룹",
  "仅在选中的自定义账号分组内轮转；与账号计划筛选同时设置时，账号必须同时满足两项条件。":
    "선택한 사용자 지정 계정 그룹 안에서만 순환합니다. 플랜 필터도 설정하면 두 조건을 모두 충족해야 합니다.",
  "尚未配置账号分组。请先在 OpenAI 账号池中编辑账号并填写分组。":
    "아직 계정 그룹이 없습니다. 먼저 OpenAI 계정 풀에서 계정을 편집해 그룹을 지정하세요.",
  "额度分发开启时，平台 Key 必须归属到一个成员钱包。":
    "한도 분배가 켜져 있으면 platform key는 반드시 구성원 지갑에 귀속되어야 합니다.",
  "未开启额度分发时可先不分配，开启后再补齐归属。":
    "한도 분배가 꺼져 있으면 우선 미할당으로 둘 수 있고, 켠 뒤 소속을 보완할 수 있습니다.",
  "总额度限制 (Token，可选)": "총 한도 제한(Token, 선택)",
  不填表示不限制: "비워 두면 제한 없음",
  K: "K",
  M: "M",
  "达到上限后，这把平台密钥的新请求会被拒绝；已在途请求会按完成后的真实用量继续统计。":
    "상한에 도달하면 이 platform key의 새 요청은 거부됩니다. 이미 진행 중인 요청은 완료 후 실제 사용량으로 계속 집계됩니다.",
  按: "기준",
  参考估算: "참고 추정",
};
