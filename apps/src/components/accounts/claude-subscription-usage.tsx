import { Badge } from "@/components/ui/badge";
import type { ClaudeSubscriptionUsage, ClaudeSubscriptionUsageWindow } from "@/lib/api/claude-subscription-client";
import { useI18n } from "@/lib/i18n/provider";

const USAGE_STALE_AFTER_MS = 60 * 60 * 1000;

function asMilliseconds(value: number | null | undefined): number | null {
  if (value == null || !Number.isFinite(value) || value <= 0) return null;
  return value < 1_000_000_000_000 ? value * 1000 : value;
}

function formatTime(value: number | null | undefined, locale: string): string | null {
  const milliseconds = asMilliseconds(value);
  if (milliseconds == null) return null;
  const date = new Date(milliseconds);
  if (Number.isNaN(date.getTime())) return null;
  return new Intl.DateTimeFormat(locale, { dateStyle: "short", timeStyle: "short" }).format(date);
}

function isValidPercent(value: number | null | undefined): value is number {
  return value != null && Number.isFinite(value) && value >= 0 && value <= 100;
}

function windowIsCurrent(
  window: ClaudeSubscriptionUsageWindow | null | undefined,
  capturedMilliseconds: number | null,
  now: number,
): boolean {
  const resetMilliseconds = asMilliseconds(window?.resetsAt);
  return capturedMilliseconds != null && capturedMilliseconds <= now &&
    now - capturedMilliseconds <= USAGE_STALE_AFTER_MS &&
    (resetMilliseconds == null || resetMilliseconds > now);
}

function usageLabel(window: ClaudeSubscriptionUsageWindow | null | undefined, current: boolean): string {
  const percent = window?.usedPercent;
  if (!current || !isValidPercent(percent)) return "—";
  return `${Math.round(percent * 10) / 10}%`;
}

export function ClaudeSubscriptionUsageView({ usage, now }: { usage?: ClaudeSubscriptionUsage | null; now: number }) {
  const { locale, t } = useI18n();
  const capturedAt = formatTime(usage?.capturedAt, locale);
  const lastAttemptAt = formatTime(usage?.lastAttemptAt, locale);
  const nextAttemptAt = formatTime(usage?.nextAttemptAt, locale);
  const capturedMilliseconds = asMilliseconds(usage?.capturedAt);
  const fiveHourCurrent = windowIsCurrent(usage?.fiveHour, capturedMilliseconds, now);
  const sevenDayCurrent = windowIsCurrent(usage?.sevenDay, capturedMilliseconds, now);
  const fiveHourReset = fiveHourCurrent ? formatTime(usage?.fiveHour?.resetsAt, locale) : null;
  const sevenDayReset = sevenDayCurrent ? formatTime(usage?.sevenDay?.resetsAt, locale) : null;
  const hasFiveHourData = isValidPercent(usage?.fiveHour?.usedPercent);
  const hasSevenDayData = isValidPercent(usage?.sevenDay?.usedPercent);
  const hasCurrentData = (hasFiveHourData && fiveHourCurrent) || (hasSevenDayData && sevenDayCurrent);
  const isStale = (hasFiveHourData && !fiveHourCurrent) || (hasSevenDayData && !sevenDayCurrent);
  const queryState = usage?.lastError
    ? t("查询失败")
    : hasCurrentData
      ? t("已更新")
      : isStale
        ? t("数据过期")
      : t("暂无额度数据");

  return (
    <div className="space-y-1 text-xs text-muted-foreground">
      <div className="flex flex-wrap gap-x-4 gap-y-1">
        <span>{t("5 小时已用")}: <strong className="font-medium text-foreground">{usageLabel(usage?.fiveHour, fiveHourCurrent)}</strong>
          {fiveHourReset ? ` · ${t("重置于")} ${fiveHourReset}` : null}
        </span>
        <span>{t("7 天已用")}: <strong className="font-medium text-foreground">{usageLabel(usage?.sevenDay, sevenDayCurrent)}</strong>
          {sevenDayReset ? ` · ${t("重置于")} ${sevenDayReset}` : null}
        </span>
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Badge variant={usage?.lastError ? "destructive" : hasCurrentData ? "secondary" : "outline"}>{queryState}</Badge>
        {isStale && (usage?.lastError || hasCurrentData) ? <Badge variant="outline">{t("数据过期")}</Badge> : null}
        {capturedAt ? <span>{t("最近观测于")} {capturedAt}</span> : null}
        {lastAttemptAt ? <span>{t("上次查询")} {lastAttemptAt}</span> : null}
        {nextAttemptAt ? <span>{t("下次尝试")} {nextAttemptAt}</span> : null}
      </div>
      {usage?.lastError ? <p className="text-destructive">{usage.lastError}</p> : null}
    </div>
  );
}
