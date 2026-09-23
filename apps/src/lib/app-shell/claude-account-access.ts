import type { AppRole } from "@/types";

export function canManageClaudeAccounts(
  isDesktopRuntime: boolean,
  isSessionLoading: boolean,
  role: AppRole | string | null | undefined,
): boolean {
  return isDesktopRuntime || (
    !isSessionLoading && (role === "system_admin" || role === "admin")
  );
}
