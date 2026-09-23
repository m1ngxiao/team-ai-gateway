"use client";

import { ClaudeSubscriptionAccountsPanel } from "@/components/accounts/claude-subscription-accounts-panel";
import { Card, CardContent } from "@/components/ui/card";
import { resolveSessionRole, useAppSession } from "@/hooks/useAppSession";
import { useDesktopPageActive } from "@/hooks/useDesktopPageActive";
import { usePageTransitionReady } from "@/hooks/usePageTransitionReady";
import { useRuntimeCapabilities } from "@/hooks/useRuntimeCapabilities";
import { useI18n } from "@/lib/i18n/provider";
import { canManageClaudeAccounts } from "@/lib/app-shell/claude-account-access";
import { useAppStore } from "@/lib/store/useAppStore";

export default function ClaudeAccountsPage() {
  const { t } = useI18n();
  const serviceStatus = useAppStore((state) => state.serviceStatus);
  const { canAccessManagementRpc, isDesktopRuntime } = useRuntimeCapabilities();
  const { data: session, isLoading: isSessionLoading } = useAppSession();
  const role = resolveSessionRole(session, isSessionLoading, isDesktopRuntime);
  const isPageActive = useDesktopPageActive("/claude-accounts/");
  const isServiceReady = canAccessManagementRpc && serviceStatus.connected;
  const canManageAccounts = canManageClaudeAccounts(isDesktopRuntime, isSessionLoading, role);

  usePageTransitionReady("/claude-accounts/", true);

  return (
    <div className="space-y-6">
      {!isServiceReady ? (
        <Card className="glass-card mission-panel shadow-sm">
          <CardContent className="pt-6 text-sm text-muted-foreground">
            {t("服务未连接，账号列表与相关操作暂不可用；连接恢复后会自动继续加载。")}
          </CardContent>
        </Card>
      ) : null}

      <ClaudeSubscriptionAccountsPanel
        serviceAddr={serviceStatus.addr}
        enabled={isServiceReady && canManageAccounts && isPageActive}
      />
    </div>
  );
}
