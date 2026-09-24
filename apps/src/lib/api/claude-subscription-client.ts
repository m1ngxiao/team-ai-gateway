import { invoke, withAddr } from "./transport";

export interface ClaudeSubscriptionUsageWindow {
  usedPercent: number | null;
  resetsAt: number | null;
}

export interface ClaudeSubscriptionUsage {
  fiveHour: ClaudeSubscriptionUsageWindow | null;
  sevenDay: ClaudeSubscriptionUsageWindow | null;
  capturedAt: number | null;
  lastAttemptAt: number | null;
  nextAttemptAt: number | null;
  lastError: string | null;
}

export interface ClaudeSubscriptionAccount {
  id: string;
  label: string;
  email: string | null;
  organizationUuid: string | null;
  subscriptionType: string | null;
  status: "active" | "disabled" | "needs_login";
  sort: number;
  expiresAt: number;
  lastError: string | null;
  usage?: ClaudeSubscriptionUsage | null;
}

export interface ClaudeSubscriptionLoginStart {
  loginId: string;
  authUrl: string;
}

export interface ClaudeSubscriptionLoginComplete {
  accountId: string;
  status: string;
}

function paramsWithAddr(addr?: string): Record<string, unknown> {
  return withAddr(addr ? { addr } : {});
}

export const claudeSubscriptionClient = {
  list: (addr?: string) =>
    invoke<ClaudeSubscriptionAccount[]>("service_claude_account_list", paramsWithAddr(addr)),
  loginStart: (addr?: string) =>
    invoke<ClaudeSubscriptionLoginStart>("service_claude_account_login_start", paramsWithAddr(addr)),
  loginComplete: (loginId: string, code: string, addr?: string) =>
    invoke<ClaudeSubscriptionLoginComplete>("service_claude_account_login_complete", {
      ...paramsWithAddr(addr), loginId, code,
    }),
  updateStatus: (accountId: string, status: "active" | "disabled", addr?: string) =>
    invoke<unknown>("service_claude_account_update_status", {
      ...paramsWithAddr(addr), accountId, status,
    }),
  usageRefresh: (accountId: string, addr?: string) =>
    invoke<ClaudeSubscriptionUsage>("service_claude_account_usage_refresh", {
      ...paramsWithAddr(addr), accountId,
    }),
  delete: (accountId: string, addr?: string) =>
    invoke<unknown>("service_claude_account_delete", {
      ...paramsWithAddr(addr), accountId,
    }),
};
