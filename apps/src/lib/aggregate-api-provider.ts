function normalizeAggregateApiProvider(value: string): string {
  return String(value || "")
    .trim()
    .toLowerCase()
    .replaceAll("-", "_");
}

const CLAUDE_AGGREGATE_API_PROVIDERS = new Set([
  "claude",
  "anthropic",
  "anthropic_native",
  "claude_code",
]);
const GEMINI_AGGREGATE_API_PROVIDERS = new Set([
  "gemini",
  "gemini_native",
  "google",
  "google_ai",
  "google_gemini",
]);

function isActiveAggregateApi(status: string): boolean {
  return String(status || "").trim().toLowerCase() === "active";
}

export function isActiveClaudeAggregateApi(
  providerType: string,
  status: string,
): boolean {
  return (
    CLAUDE_AGGREGATE_API_PROVIDERS.has(
      normalizeAggregateApiProvider(providerType),
    ) && isActiveAggregateApi(status)
  );
}

export function isActiveOpenAiAggregateApi(
  providerType: string,
  status: string,
  protocolType: string,
): boolean {
  if (!isActiveAggregateApi(status)) return false;
  const provider = normalizeAggregateApiProvider(providerType);
  if (protocolType === "gemini_native") {
    return GEMINI_AGGREGATE_API_PROVIDERS.has(provider);
  }
  return (
    !CLAUDE_AGGREGATE_API_PROVIDERS.has(provider) &&
    !GEMINI_AGGREGATE_API_PROVIDERS.has(provider)
  );
}

export function aggregateApiUsesIncomingPath(providerType: string): boolean {
  return normalizeAggregateApiProvider(providerType) === "compatible";
}

export function aggregateApiProviderMatchesFilter(
  providerType: string,
  providerFilter: string,
): boolean {
  const provider = normalizeAggregateApiProvider(providerType);
  const filter = normalizeAggregateApiProvider(providerFilter);

  if (!filter || filter === "all" || provider === filter) return true;
  return provider === "compatible" && (filter === "codex" || filter === "claude");
}
