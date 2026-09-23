export function formatClaudeSubscriptionPlan(value: string | null | undefined): string {
  const normalized = String(value || "").trim().toLowerCase().replaceAll("-", "_");
  const plan = normalized.startsWith("claude_") ? normalized.slice(7) : normalized;
  for (const [prefix, label] of [
    ["pro", "Pro"],
    ["max", "Max"],
    ["team", "Team"],
    ["enterprise", "Enterprise"],
    ["free", "Free"],
  ]) {
    if (plan === prefix || plan.startsWith(`${prefix}_`)) return label;
  }
  return "未知方案";
}
