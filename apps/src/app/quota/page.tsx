"use client";

import { useCallback, useEffect, useState } from "react";
import { postJsonRpc } from "@/lib/api/rpc-http";
import { Button } from "@/components/ui/button";

type QuotaKey = {
  keyId: string;
  name: string | null;
  status: string;
  usedCents: number;
  limitCents: number;
  remainingCents: number;
};

type QuotaAccount = {
  accountId: string;
  label: string | null;
  status: string;
  usedCents: number;
  limitCents: number;
  remainingCents: number;
  resetAt: number | null;
};

type QuotaOverview = {
  generatedAt: number;
  keyLimitCents: number;
  accountLimitCents: number;
  keys: QuotaKey[];
  accounts: QuotaAccount[];
};

function usd(cents: number): string {
  return `$${(cents / 100).toFixed(2)}`;
}

function usagePercent(used: number, limit: number): number {
  if (limit <= 0) return 0;
  return Math.min(100, Math.max(0, (used / limit) * 100));
}

function formatTime(value: number | null): string {
  return value ? new Date(value * 1000).toLocaleString() : "-";
}

function meterClass(percent: number): string {
  if (percent >= 100) return "bg-red-500";
  if (percent >= 80) return "bg-amber-500";
  return "bg-emerald-500";
}

export default function QuotaPage() {
  const [data, setData] = useState<QuotaOverview | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const load = useCallback(async () => {
    setLoading(true);
    setError("");
    try {
      const overview = await postJsonRpc<QuotaOverview>(
        fetch,
        "/api/rpc",
        "quota/weeklyOverview"
      );
      setData(overview);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const resetKey = async (key: QuotaKey) => {
    const label = key.name?.trim() || key.keyId;
    if (!window.confirm(`确认将「${label}」的滚动 7 天额度清零？`)) return;
    setBusy(true);
    setError("");
    try {
      await postJsonRpc(fetch, "/api/rpc", "quota/resetKeyWeekly", {
        keyId: key.keyId,
      });
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  const officialReset = async () => {
    if (
      !window.confirm(
        "确认这是 OpenAI 官方全局重置？这会把所有 Key 的滚动 7 天额度清零（包括已停用的 Key）。"
      )
    ) {
      return;
    }
    setBusy(true);
    setError("");
    try {
      await postJsonRpc(fetch, "/api/rpc", "quota/officialGlobalReset", {
        confirmedBy: "admin",
      });
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="mx-auto flex w-full max-w-6xl flex-col gap-5 p-4 md:p-6">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-2xl font-semibold tracking-tight">额度</h1>
          <p className="mt-1 text-sm text-muted-foreground">
            每 Key 滚动 7 天 $240；每账号自然周 $1,200。这里只做运营查看与重置。
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline" onClick={() => void load()} disabled={loading || busy}>
            刷新
          </Button>
          <Button variant="destructive" onClick={() => void officialReset()} disabled={busy}>
            官方全局重置
          </Button>
        </div>
      </div>

      {error ? (
        <div className="rounded-lg border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
          {error}
        </div>
      ) : null}

      <section className="rounded-xl border bg-card p-4">
        <h2 className="text-sm font-semibold">账号</h2>
        <div className="mt-3 overflow-x-auto">
          <table className="w-full min-w-[720px] text-left text-sm">
            <thead className="text-xs uppercase text-muted-foreground">
              <tr>
                <th className="pb-2 pr-4 font-medium">账号</th>
                <th className="pb-2 pr-4 font-medium">状态</th>
                <th className="pb-2 pr-4 font-medium">已用</th>
                <th className="pb-2 pr-4 font-medium">剩余</th>
                <th className="pb-2 pr-4 font-medium">重置时间</th>
              </tr>
            </thead>
            <tbody>
              {(data?.accounts ?? []).map((account) => {
                const percent = usagePercent(account.usedCents, account.limitCents);
                return (
                  <tr key={account.accountId} className="border-t">
                    <td className="py-3 pr-4 font-medium">
                      {account.label?.trim() || account.accountId}
                    </td>
                    <td className="py-3 pr-4">{account.status}</td>
                    <td className="py-3 pr-4">
                      <div className="flex min-w-[180px] items-center gap-2">
                        <div className="h-2 flex-1 overflow-hidden rounded bg-muted">
                          <div className={`h-full ${meterClass(percent)}`} style={{ width: `${percent}%` }} />
                        </div>
                        <span className="w-20 text-right tabular-nums">
                          {usd(account.usedCents)}
                        </span>
                      </div>
                    </td>
                    <td className="py-3 pr-4 tabular-nums">{usd(account.remainingCents)}</td>
                    <td className="py-3 pr-4 text-muted-foreground">{formatTime(account.resetAt)}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </section>

      <section className="rounded-xl border bg-card p-4">
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-semibold">Key</h2>
          <span className="text-xs text-muted-foreground">
            {data ? `生成于 ${formatTime(data.generatedAt)}` : ""}
          </span>
        </div>
        {loading ? (
          <p className="mt-3 text-sm text-muted-foreground">加载中…</p>
        ) : (
          <div className="mt-3 overflow-x-auto">
            <table className="w-full min-w-[820px] text-left text-sm">
              <thead className="text-xs uppercase text-muted-foreground">
                <tr>
                  <th className="pb-2 pr-4 font-medium">名称</th>
                  <th className="pb-2 pr-4 font-medium">状态</th>
                  <th className="pb-2 pr-4 font-medium">已用</th>
                  <th className="pb-2 pr-4 font-medium">剩余</th>
                  <th className="pb-2 pr-4 font-medium">操作</th>
                </tr>
              </thead>
              <tbody>
                {(data?.keys ?? []).map((key) => {
                  const percent = usagePercent(key.usedCents, key.limitCents);
                  return (
                    <tr key={key.keyId} className="border-t">
                      <td className="py-3 pr-4 font-medium">{key.name?.trim() || key.keyId}</td>
                      <td className="py-3 pr-4">{key.status}</td>
                      <td className="py-3 pr-4">
                        <div className="flex min-w-[180px] items-center gap-2">
                          <div className="h-2 flex-1 overflow-hidden rounded bg-muted">
                            <div className={`h-full ${meterClass(percent)}`} style={{ width: `${percent}%` }} />
                          </div>
                          <span className="w-20 text-right tabular-nums">{usd(key.usedCents)}</span>
                        </div>
                      </td>
                      <td className="py-3 pr-4 tabular-nums">{usd(key.remainingCents)}</td>
                      <td className="py-3 pr-4">
                        <Button
                          size="sm"
                          variant="outline"
                          disabled={busy}
                          onClick={() => void resetKey(key)}
                        >
                          清零
                        </Button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}
