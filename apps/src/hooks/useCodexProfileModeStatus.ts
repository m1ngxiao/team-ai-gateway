"use client";

import { skipToken, useQuery } from "@tanstack/react-query";
import { CODEX_PROFILE_STATUS_QUERY_KEY } from "@/lib/api/codex-profile-client";
import { useRuntimeCapabilities } from "@/hooks/useRuntimeCapabilities";
import { useAppStore } from "@/lib/store/useAppStore";
import type { CodexProfileMode, CodexProfileStatus } from "@/types";

export const CODEX_PROFILE_MODE_LABELS: Record<CodexProfileMode, string> = {
  missing: "未发现配置",
  unmanaged: "未托管",
  direct_account: "直接连接 OpenAI",
  gateway: "通过 CodexManager",
  managed_unknown: "托管状态未知",
};

interface UseCodexProfileModeStatusOptions {
  enabled?: boolean;
  refetchIntervalMs?: number | false;
}

// The Docker gateway cannot know the login mode of a remote Codex client.
// skipToken prevents both background requests and manual refetch from reading
// legacy profiles. Explicitly discard cached profile data from older pages.
export function useCodexProfileModeStatus(_options: UseCodexProfileModeStatusOptions = {}) {
  const connected = useAppStore((state) => state.serviceStatus.connected);
  const { canAccessManagementRpc } = useRuntimeCapabilities();
  const query = useQuery<CodexProfileStatus>({
    queryKey: CODEX_PROFILE_STATUS_QUERY_KEY,
    queryFn: skipToken,
    enabled: false,
  });
  return {
    ...query,
    data: undefined as CodexProfileStatus | undefined,
    status: undefined as CodexProfileStatus | undefined,
    mode: null as CodexProfileMode | null,
    modeLabel: "Docker 服务",
    isServiceReady: canAccessManagementRpc && connected,
    isDirectAccountMode: false,
    isGatewayMode: false,
  };
}
