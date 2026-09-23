import type { ApiKeyUpstreamProvider } from "@/types/api-key";

export interface ApiKeyUpdatePayload {
  name?: string | null;
  modelSlug?: string | null;
  reasoningEffort?: string | null;
  serviceTier?: string | null;
  protocolType?: string | null;
  upstreamBaseUrl?: string | null;
  staticHeadersJson?: string | null;
  rotationStrategy?: string | null;
  upstreamProvider?: ApiKeyUpstreamProvider;
  confirmRouteReview?: boolean;
  aggregateApiId?: string | null;
  accountPlanFilter?: string | null;
  accountGroupFilter?: string | null;
  quotaLimitTokens?: number | null;
}

export function buildApiKeyUpdateInvokePayload(
  keyId: string,
  params: ApiKeyUpdatePayload,
): Record<string, unknown> {
  const payload: Record<string, unknown> = {
    keyId,
    protocolType: params.protocolType || null,
    upstreamBaseUrl: params.upstreamBaseUrl || null,
    staticHeadersJson: params.staticHeadersJson || null,
  };
  if ("name" in params) {
    payload.hasName = true;
    payload.name = params.name || null;
  }
  if (
    "modelSlug" in params ||
    "reasoningEffort" in params ||
    "serviceTier" in params
  ) {
    payload.hasModelConfig = true;
    payload.modelSlug = params.modelSlug || null;
    payload.reasoningEffort = params.reasoningEffort || null;
    payload.serviceTier = params.serviceTier || null;
  }
  if ("upstreamProvider" in params) {
    payload.upstreamProvider = params.upstreamProvider;
  }
  if (params.confirmRouteReview === true) {
    payload.confirmRouteReview = true;
  }
  if (
    "rotationStrategy" in params ||
    "aggregateApiId" in params ||
    "accountPlanFilter" in params
  ) {
    payload.hasRoutingConfig = true;
    payload.rotationStrategy = params.rotationStrategy || null;
    payload.aggregateApiId = params.aggregateApiId || null;
    payload.accountPlanFilter = params.accountPlanFilter || null;
  }
  if ("accountGroupFilter" in params) {
    payload.hasAccountGroupFilter = true;
    payload.accountGroupFilter = params.accountGroupFilter || null;
  }
  if ("quotaLimitTokens" in params) {
    payload.hasQuotaLimitTokens = true;
    payload.quotaLimitTokens = params.quotaLimitTokens ?? null;
  }
  return payload;
}
