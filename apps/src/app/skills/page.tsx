"use client";

import { BookOpen } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle, CardDescription } from "@/components/ui/card";
import { useI18n } from "@/lib/i18n/provider";

export default function SkillsPage() {
  const { t } = useI18n();
  return (
    <main className="flex w-full flex-col gap-5">
      <Card className="glass-card">
        <CardHeader>
          <CardTitle className="flex items-center gap-3 text-xl">
            <BookOpen className="size-6 text-primary" aria-hidden="true" />
            {t("客户端 Skills 与插件")}
          </CardTitle>
          <CardDescription className="text-sm leading-6">
            {t("Docker 网关不管理客户端 Skills，请在自己的 Codex 客户端安装。")}
          </CardDescription>
        </CardHeader>
        <CardContent className="space-y-2 text-sm leading-6 text-muted-foreground">
          <p>{t("Skills 与 Codex 插件在运行 Codex 的客户端生效，安装、更新和移除也在该客户端完成。")}</p>
          <p>{t("使用平台 API Key 连接此网关，不会同步客户端 Skills。网关继续管理账号池、平台密钥、请求路由与用量记录。")}</p>
        </CardContent>
      </Card>
    </main>
  );
}
