"use client";

import { AlertTriangle, Check, ArrowRight, PieChart } from "lucide-react";
import { buttonVariants } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { Progress } from "@/components/ui/progress";
import { Skeleton } from "@/components/ui/skeleton";
import {
  formatCompactTokenAmount,
  formatPercent,
} from "@/lib/dashboard/format";
import { useI18n } from "@/lib/i18n/provider";
import { cn } from "@/lib/utils";
import { buildStaticRouteUrl } from "@/lib/utils/static-routes";

interface DashboardGatewayStatusProps {
  connected: boolean;
  directMode: boolean;
  stats: {
    total: number;
    available: number;
    unavailable: number;
    todayTokens: number;
    cachedTokens: number;
    reasoningTokens: number;
    todayCost: number;
    todayRequests?: number;
    successRate?: number | null;
  };
  isLoading: boolean;
}

interface DashboardPoolRemainingProps {
  primary: number | null;
  secondary: number | null;
  primaryKnownCount: number;
  primaryBucketCount: number;
  secondaryKnownCount: number;
  secondaryBucketCount: number;
  isLoading: boolean;
}

function formatUsd(value: number): string {
  return `$${Math.max(0, value || 0).toFixed(2)}`;
}

export function DashboardGatewayStatus({
  connected,
  directMode,
  stats,
  isLoading,
}: DashboardGatewayStatusProps) {
  const { t } = useI18n();

  if (isLoading) {
    return <Skeleton className="h-[148px] rounded-xl xl:h-[176px] xl:rounded-2xl" />;
  }

  const title = directMode
    ? t("当前为账号直连模式")
    : connected
      ? t("网关运行正常")
      : t("正在等待网关连接");
  const description = directMode
    ? t("CodexManager 无法统计 CLI 请求日志和用量。")
    : connected
      ? t("近期请求路由稳定，账号池可正常参与调度。")
      : t("正在等待服务连接。");
  const actionHref = directMode ? "/platform-mode" : "/logs";
  const actionLabel = directMode ? t("去切换为本地网关") : t("查看异常请求");
  const successRate =
    stats.successRate == null ? "--" : `${Math.round(stats.successRate * 100)}%`;

  return (
    <Card className="dashboard-primary-panel dashboard-hero routing-command-card glass-card relative overflow-hidden rounded-[20px] border-border/70 py-0">
      <CardContent className="relative z-10 p-0">
        <div className="grid gap-6 px-5 py-5 lg:grid-cols-[minmax(0,1.08fr)_minmax(460px,0.92fr)] lg:px-7 lg:py-7">
          <div className="flex min-w-0 flex-col justify-between gap-7">
            <div>
              <span className="dashboard-hero-status" data-warning={!connected || directMode}>
                {connected ? t("实时运行") : t("等待连接")}
              </span>
              <div className="mt-5 flex items-start gap-4">
                <div
                  className={cn(
                    "mt-1 flex h-10 w-10 shrink-0 items-center justify-center rounded-full border",
                    connected && !directMode
                      ? "border-[#7f9a82]/35 bg-[#7f9a82]/10 text-[#607b64]"
                      : "border-amber-600/25 bg-amber-500/8 text-amber-700",
                  )}
                >
                  {connected && !directMode ? (
                    <Check className="h-[18px] w-[18px] stroke-[2]" />
                  ) : (
                    <AlertTriangle className="h-[18px] w-[18px]" />
                  )}
                </div>
                <div className="min-w-0">
                  <h2 className="font-display text-[2.15rem] font-medium leading-[1.05] tracking-[-0.045em] text-foreground xl:text-[2.65rem]">
                    {title}
                  </h2>
                  <p className="mt-3 max-w-xl text-sm leading-6 text-muted-foreground">
                    {description}
                  </p>
                </div>
              </div>
            </div>
            <div className="flex flex-wrap items-center gap-x-5 gap-y-3">
              <a
                href={buildStaticRouteUrl(actionHref)}
                className={cn(
                  buttonVariants({ size: "sm" }),
                  "command-center-primary-action h-9 rounded-full px-4 text-xs",
                )}
              >
                {actionLabel}
                <ArrowRight className="ml-0.5 h-3.5 w-3.5" />
              </a>
              <span className="text-xs text-muted-foreground">
                {t("服务连接")} · {connected ? t("正常") : t("离线")}
              </span>
              <span className="text-xs text-muted-foreground">
                {t("账号")} {stats.total} · {t("可用")} {stats.available} · {t("异常")} {stats.unavailable}
              </span>
            </div>
          </div>

          <div className="grid gap-2.5 sm:grid-cols-2">
            <div className="dashboard-stat-tile p-4 sm:col-span-2">
              <div className="flex items-center justify-between gap-3 text-xs text-muted-foreground">
                <span>{t("今日 Token")}</span>
                <span>{t("成功")} {successRate}</span>
              </div>
              <div className="mt-2 flex items-end justify-between gap-4">
                <strong className="font-display text-[2.1rem] font-medium leading-none tracking-[-0.04em] text-foreground">
                  {formatCompactTokenAmount(stats.todayTokens)}
                </strong>
                {stats.todayRequests != null ? (
                  <span className="pb-1 text-xs text-muted-foreground">
                    {stats.todayRequests.toLocaleString()} {t("请求数")}
                  </span>
                ) : null}
              </div>
            </div>
            <div className="dashboard-stat-tile p-4">
              <span className="text-[11px] font-semibold uppercase tracking-[0.1em] text-muted-foreground">
                {t("预计费用")}
              </span>
              <strong className="mt-2 block font-mono text-xl font-semibold text-foreground">
                {formatUsd(stats.todayCost)}
              </strong>
            </div>
            <a
              href={buildStaticRouteUrl("/logs")}
              className="dashboard-stat-tile group p-4 transition-colors hover:bg-foreground/[0.035]"
            >
              <span className="text-[11px] font-semibold uppercase tracking-[0.1em] text-muted-foreground">
                {t("今日/缓存/推理 用量")}
              </span>
              <strong className="mt-2 block font-mono text-[13px] font-semibold text-foreground">
                {formatCompactTokenAmount(stats.todayTokens)} ·{" "}
                {formatCompactTokenAmount(stats.cachedTokens)} ·{" "}
                {formatCompactTokenAmount(stats.reasoningTokens)}
              </strong>
            </a>
          </div>
        </div>
      </CardContent>
    </Card>
  );
}

function PoolBucket({
  label,
  value,
  knownCount,
  bucketCount,
  tone,
}: {
  label: string;
  value: number | null;
  knownCount: number;
  bucketCount: number;
  tone: "emerald" | "blue";
}) {
  const { t } = useI18n();
  const normalizedValue = value == null ? 0 : Math.max(0, Math.min(100, value));
  const isEmerald = tone === "emerald";

  return (
    <div className="min-w-0">
      <div className="mb-2 flex items-end justify-between gap-3">
        <span className="text-xs font-medium text-muted-foreground">{label}</span>
        <span
          className={cn(
            "font-display text-[1.8rem] font-medium leading-none tracking-[-0.04em]",
            isEmerald ? "text-emerald-600" : "text-blue-600",
          )}
        >
          {formatPercent(value)}
        </span>
      </div>
      <Progress
        value={normalizedValue}
        className="gap-0"
        trackClassName={cn(
          "h-1.5",
          isEmerald ? "bg-[#6f8b72]/15" : "bg-[#6d8290]/15",
        )}
        indicatorClassName={isEmerald ? "bg-[#6f8b72]" : "bg-[#6d8290]"}
      />
      <div className="mt-2 truncate text-[10px] text-muted-foreground">
        {knownCount} / {bucketCount} {t("个账号有可用数据")}
      </div>
    </div>
  );
}

export function DashboardPoolRemaining({
  primary,
  secondary,
  primaryKnownCount,
  primaryBucketCount,
  secondaryKnownCount,
  secondaryBucketCount,
  isLoading,
}: DashboardPoolRemainingProps) {
  const { t } = useI18n();

  if (isLoading) {
    return <Skeleton className="h-[84px] rounded-xl xl:rounded-2xl" />;
  }

  return (
    <Card className="dashboard-pool-remaining dashboard-primary-panel dashboard-rail-card glass-card overflow-hidden border-border/70 py-0">
      <CardContent className="grid gap-6">
        <div className="flex min-w-0 items-start justify-between gap-4">
          <div>
            <div className="flex items-center gap-2">
              <PieChart className="h-4 w-4 shrink-0 text-[#6f8b72]" />
              <span className="text-sm font-semibold text-foreground">
                {t("账号池剩余")}
              </span>
            </div>
            <p className="mt-2 text-xs leading-5 text-muted-foreground">
              {t("按账号最近一次成功同步的额度窗口汇总")}
            </p>
          </div>
          <Badge
            variant="outline"
            className="h-6 shrink-0 rounded-full border-[#6f8b72]/25 bg-[#6f8b72]/8 px-2.5 text-[9px] font-semibold tracking-[0.08em] text-[#5e7b63]"
          >
            LIVE
          </Badge>
        </div>
        <div className="grid gap-5">
          <PoolBucket
            label={t("5小时内")}
            value={primary}
            knownCount={primaryKnownCount}
            bucketCount={primaryBucketCount}
            tone="emerald"
          />
          <div className="h-px bg-border/60" />
          <PoolBucket
            label={t("7天内")}
            value={secondary}
            knownCount={secondaryKnownCount}
            bucketCount={secondaryBucketCount}
            tone="blue"
          />
        </div>
        <div className="rounded-xl border border-border/60 bg-background/35 px-3 py-2.5 text-[11px] leading-5 text-muted-foreground">
          {t("剩余比例按各账号窗口平均值估算，不代表单次请求限制。")}
        </div>
      </CardContent>
    </Card>
  );
}
