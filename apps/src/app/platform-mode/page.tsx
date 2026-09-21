"use client";

import { Server, ShieldCheck, KeyRound } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { useAppStore } from "@/lib/store/useAppStore";
import { useRuntimeCapabilities } from "@/hooks/useRuntimeCapabilities";
import { buildStaticRouteUrl } from "@/lib/utils/static-routes";
import { PUBLIC_ORIGINS } from "@/lib/gateway/public-origins";
import { useI18n } from "@/lib/i18n/provider";

export default function PlatformModePage() {
  const { t } = useI18n();
  const endpoints = [
    [t("Codex / OpenAI 兼容 API"), `${PUBLIC_ORIGINS.api}/v1`],
    ["Claude Code", PUBLIC_ORIGINS.api],
    [t("管理员后台"), PUBLIC_ORIGINS.admin],
    [t("团队用量看板"), PUBLIC_ORIGINS.stats],
  ] as const;
  const connected = useAppStore((state) => state.serviceStatus.connected);
  const { canAccessManagementRpc } = useRuntimeCapabilities();
  const ready = canAccessManagementRpc && connected;

  return (
    <main className="flex w-full flex-col gap-5">
      <Card className="glass-card">
        <CardHeader>
          <div className="flex items-center gap-3">
            <Server className="size-6 text-primary" aria-hidden="true" />
            <CardTitle className="text-xl">{t("Docker 服务接入")}</CardTitle>
            <Badge variant={ready ? "default" : "secondary"}>
              {ready ? t("管理服务已连接") : t("等待管理服务连接")}
            </Badge>
          </div>
          <CardDescription className="text-sm leading-6">
            {t("这里管理共享网关，不管理任何一台电脑上的 Codex 登录或配置。")}
          </CardDescription>
        </CardHeader>
        <CardContent>
          <dl className="grid gap-5 sm:grid-cols-2">
            <div><dt className="text-sm text-muted-foreground">{t("部署方式")}</dt><dd className="mt-1 font-medium">{t("Docker · 独立网关服务")}</dd></div>
            <div><dt className="text-sm text-muted-foreground">{t("客户端鉴权")}</dt><dd className="mt-1 font-medium">{t("每人使用自己的平台 API Key")}</dd></div>
            <div><dt className="text-sm text-muted-foreground">{t("请求路由与模型")}</dt><dd className="mt-1 font-medium">{t("由请求携带的平台 Key 配置决定")}</dd></div>
            <div><dt className="text-sm text-muted-foreground">{t("日志与用量")}</dt><dd className="mt-1 font-medium">{t("记录经过此网关的请求")}</dd></div>
          </dl>
        </CardContent>
      </Card>

      <Card className="glass-card">
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-lg"><KeyRound className="size-5" aria-hidden="true" />{t("服务地址")}</CardTitle>
          <CardDescription className="text-sm">{t("同事配置客户端时使用 API 地址，不使用管理员域名。")}</CardDescription>
        </CardHeader>
        <CardContent>
          <dl className="grid gap-4">
            {endpoints.map(([label, endpoint]) => (
              <div key={label} className="grid gap-1 border-b border-border/50 pb-4 last:border-0 last:pb-0 sm:grid-cols-[220px_minmax(0,1fr)]">
                <dt className="text-sm text-muted-foreground">{label}</dt>
                <dd className="break-all font-mono text-sm select-text">{endpoint}</dd>
              </div>
            ))}
          </dl>
          <div className="mt-5 flex flex-wrap gap-5 text-sm">
            <a className="text-primary underline underline-offset-4" href={buildStaticRouteUrl("/apikeys")}>{t("管理平台密钥")}</a>
            <a className="text-primary underline underline-offset-4" href={buildStaticRouteUrl("/accounts")}>{t("管理账号池")}</a>
            <a className="text-primary underline underline-offset-4" href={buildStaticRouteUrl("/logs")}>{t("查看请求日志")}</a>
          </div>
        </CardContent>
      </Card>

      <Card className="glass-card">
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-lg"><ShieldCheck className="size-5" aria-hidden="true" />{t("客户端与服务端分开管理")}</CardTitle>
        </CardHeader>
        <CardContent className="space-y-2 text-sm leading-6 text-muted-foreground">
          <p>{t("本机 Codex 可以继续使用账号登录；同事通过 API Key 连接本服务。两种方式互不替换。")}</p>
          <p>{t("此 Docker 版本不提供客户端登录切换、配置目录修改、客户端重载或历史修复。")}</p>
          <p>{t("管理员后台使用后台账号登录；如部署了邮箱验证入口，还需完成验证。可选的团队看板使用独立账号。")}</p>
        </CardContent>
      </Card>
    </main>
  );
}
