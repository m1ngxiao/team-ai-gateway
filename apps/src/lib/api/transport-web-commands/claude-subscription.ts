import type { WebCommandDescriptor } from "./shared";

export function createClaudeSubscriptionWebCommands(): Record<string, WebCommandDescriptor> {
  return {
    service_claude_account_list: { rpcMethod: "claudeAccount/list" },
    service_claude_account_login_start: { rpcMethod: "claudeAccount/loginStart" },
    service_claude_account_login_complete: { rpcMethod: "claudeAccount/loginComplete" },
    service_claude_account_update_status: { rpcMethod: "claudeAccount/updateStatus" },
    service_claude_account_usage_refresh: { rpcMethod: "claudeAccount/usageRefresh" },
    service_claude_account_delete: { rpcMethod: "claudeAccount/delete" },
  };
}
