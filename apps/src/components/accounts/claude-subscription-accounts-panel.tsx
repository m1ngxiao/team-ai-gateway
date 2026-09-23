"use client";

import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ExternalLink, Loader2, Plus, RefreshCw, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { ConfirmDialog } from "@/components/modals/confirm-dialog";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Switch } from "@/components/ui/switch";
import {
  claudeSubscriptionClient,
  type ClaudeSubscriptionAccount,
  type ClaudeSubscriptionLoginStart,
} from "@/lib/api/claude-subscription-client";
import { getAppErrorMessage } from "@/lib/api/transport";
import { useI18n } from "@/lib/i18n/provider";

interface ClaudeSubscriptionAccountsPanelProps {
  serviceAddr: string;
  enabled: boolean;
}

function statusLabel(status: string): string {
  switch (status) {
    case "active": return "轮转中";
    case "needs_login": return "需要重新登录";
    default: return "已停用";
  }
}

export function ClaudeSubscriptionAccountsPanel({ serviceAddr, enabled }: ClaudeSubscriptionAccountsPanelProps) {
  const { t } = useI18n();
  const queryClient = useQueryClient();
  const queryKey = ["claude-subscription-accounts", serviceAddr] as const;
  const { data: accounts = [], isLoading, isError, refetch } = useQuery({
    queryKey,
    queryFn: () => claudeSubscriptionClient.list(serviceAddr),
    enabled,
    retry: 1,
  });
  const [login, setLogin] = useState<ClaudeSubscriptionLoginStart | null>(null);
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [busyAccountId, setBusyAccountId] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<ClaudeSubscriptionAccount | null>(null);

  if (!enabled) return null;

  const refresh = async () => {
    await queryClient.invalidateQueries({ queryKey });
  };

  const beginLogin = async () => {
    if (busy) return;
    setBusy(true);
    try {
      const result = await claudeSubscriptionClient.loginStart(serviceAddr);
      setCode("");
      setLogin(result);
    } catch (error) {
      toast.error(getAppErrorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  const finishLogin = async () => {
    if (!login || busy) return;
    if (!code.trim().includes("#")) {
      toast.error(t("请粘贴网页显示的完整授权码（包含 # 后的校验码）"));
      return;
    }
    setBusy(true);
    try {
      await claudeSubscriptionClient.loginComplete(login.loginId, code.trim(), serviceAddr);
      setLogin(null);
      setCode("");
      await refresh();
      toast.success(t("Claude 账号已添加，启用后进入 Claude 独立账号池"));
    } catch (error) {
      toast.error(getAppErrorMessage(error));
    } finally {
      setBusy(false);
    }
  };

  const updateStatus = async (account: ClaudeSubscriptionAccount, active: boolean) => {
    if (busyAccountId) return;
    setBusyAccountId(account.id);
    try {
      await claudeSubscriptionClient.updateStatus(account.id, active ? "active" : "disabled", serviceAddr);
      await refresh();
      toast.success(active ? t("Claude 账号已加入轮转") : t("Claude 账号已停用"));
    } catch (error) {
      toast.error(getAppErrorMessage(error));
    } finally {
      setBusyAccountId(null);
    }
  };

  const deleteAccount = async () => {
    if (!deleting) return false;
    try {
      await claudeSubscriptionClient.delete(deleting.id, serviceAddr);
      await refresh();
      toast.success(t("Claude 账号已删除"));
      setDeleting(null);
    } catch (error) {
      toast.error(getAppErrorMessage(error));
      return false;
    }
  };

  return (
    <>
      <Card className="glass-card mission-panel shadow-sm">
        <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-3">
          <div>
            <CardTitle>{t("Claude 订阅账号池")}</CardTitle>
            <p className="mt-1 text-xs text-muted-foreground">
              {t("Claude.ai Pro、Max、Team 账号独立轮转。平台 Key 选择 Claude 池后只使用这里启用的账号。")}
            </p>
          </div>
          <div className="flex gap-2">
            <Button size="sm" variant="outline" aria-label={t("刷新 Claude 账号")} onClick={() => void refetch()}>
              <RefreshCw className="size-4" />
            </Button>
            <Button size="sm" onClick={() => void beginLogin()} disabled={busy}>
              {busy ? <Loader2 className="size-4 animate-spin" /> : <Plus className="size-4" />}
              {t("添加 Claude 账号")}
            </Button>
          </div>
        </CardHeader>
        <CardContent className="space-y-3">
          {isLoading ? <p className="text-sm text-muted-foreground">{t("正在加载账号...")}</p> : null}
          {isError ? <p className="text-sm text-destructive">{t("Claude 账号加载失败，请刷新重试")}</p> : null}
          {!isLoading && !isError && accounts.length === 0 ? (
            <p className="text-sm text-muted-foreground">{t("尚无 Claude 订阅账号，点击“添加 Claude 账号”开始登录。")}</p>
          ) : null}
          {accounts.map((account) => (
            <div key={account.id} className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-border/70 bg-card/40 px-3 py-2">
              <div className="min-w-0 space-y-1">
                <div className="flex flex-wrap items-center gap-2">
                  <span className="max-w-[280px] truncate font-medium" title={account.email || account.label}>{account.email || account.label}</span>
                  <Badge variant={account.status === "needs_login" ? "destructive" : account.status === "active" ? "default" : "secondary"}>
                    {t(statusLabel(account.status))}
                  </Badge>
                </div>
                {account.lastError ? <p className="text-xs text-destructive">{account.lastError}</p> : null}
              </div>
              <div className="flex items-center gap-3">
                {account.status === "needs_login" ? (
                  <Button size="sm" variant="outline" onClick={() => void beginLogin()} disabled={busy}>
                    {t("重新登录")}
                  </Button>
                ) : null}
                <Switch
                  aria-label={`${t("启用 Claude 账号")} ${account.email || account.label}`}
                  checked={account.status === "active"}
                  disabled={busyAccountId !== null || account.status === "needs_login"}
                  onCheckedChange={(active) => void updateStatus(account, active)}
                />
                <Button size="icon" variant="ghost" aria-label={`${t("删除 Claude 账号")} ${account.email || account.label}`} onClick={() => setDeleting(account)}>
                  <Trash2 className="size-4" />
                </Button>
              </div>
            </div>
          ))}
        </CardContent>
      </Card>

      <Dialog open={login !== null} onOpenChange={(open) => { if (!open && !busy) { setLogin(null); setCode(""); } }}>
        <DialogContent className="glass-card sm:max-w-[540px]">
          <DialogHeader>
            <DialogTitle>{t("登录 Claude 订阅账号")}</DialogTitle>
            <DialogDescription>
              {t("使用要加入账号池的 Claude.ai 账号完成网页登录，然后复制网页显示的完整一次性授权码。")}
            </DialogDescription>
          </DialogHeader>
          {login ? (
            <div className="space-y-4">
              <Button variant="outline" render={<a href={login.authUrl} target="_blank" rel="noopener noreferrer" />}>
                <ExternalLink className="size-4" />{t("打开 Claude 授权页面")}
              </Button>
              <div className="space-y-2">
                <Label htmlFor="claude-subscription-code">{t("一次性授权码")}</Label>
                <Input id="claude-subscription-code" type="password" autoComplete="off" spellCheck={false} value={code} onChange={(event) => setCode(event.target.value)} placeholder="CODE#STATE" />
              </div>
            </div>
          ) : null}
          <DialogFooter>
            <Button variant="outline" onClick={() => { setLogin(null); setCode(""); }} disabled={busy}>{t("取消")}</Button>
            <Button onClick={() => void finishLogin()} disabled={busy || !code.trim()}>
              {busy ? <Loader2 className="size-4 animate-spin" /> : null}{t("完成登录")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(open) => { if (!open) setDeleting(null); }}
        title={t("删除 Claude 账号")}
        description={`${t("确定删除账号")} ${deleting?.email || deleting?.label || ""}？`}
        confirmText={t("删除")}
        confirmVariant="destructive"
        onConfirm={deleteAccount}
      />
    </>
  );
}
