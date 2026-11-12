"use client";

import { APP_DISPLAY_NAME } from "@/lib/branding";
import Image from "next/image";
import {
  Cable,
  House,
  Users,
  Bot,
  UserCog,
  Key,
  Boxes,
  Database,
  Puzzle,
  WandSparkles,
  FileText,
  FolderKanban,
  Route,
  Settings,
  Globe,
  Gauge,
  ChevronLeft,
  ChevronRight,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { buildStaticRouteUrl } from "@/lib/utils/static-routes";
import { Button } from "@/components/ui/button";
import { useAppStore } from "@/lib/store/useAppStore";
import { useI18n } from "@/lib/i18n/provider";
import { useRuntimeCapabilities } from "@/hooks/useRuntimeCapabilities";
import {
  getAllowedTopLevelRoutes,
  getTopLevelRouteLabel,
  type TopLevelRoutePath,
} from "@/lib/app-shell/top-level-routes";
import { resolveSessionRole, useAppSession } from "@/hooks/useAppSession";
import {
  memo,
  useCallback,
  useMemo,
  useState,
  type MouseEvent,
} from "react";

const NAV_ITEM_BY_PATH = new Map<TopLevelRoutePath, { icon: LucideIcon }>([
  ["/", { icon: House }],
  ["/accounts", { icon: Users }],
  ["/claude-accounts", { icon: Bot }],
  ["/account-manager", { icon: UserCog }],
  ["/aggregate-api", { icon: Database }],
  ["/apikeys", { icon: Key }],
  ["/quota", { icon: Gauge }],
  ["/platform-mode", { icon: Cable }],
  ["/projects", { icon: FolderKanban }],
  ["/models", { icon: Boxes }],
  ["/model-groups", { icon: Route }],
  ["/plugins", { icon: Puzzle }],
  ["/skills", { icon: WandSparkles }],
  ["/logs", { icon: FileText }],
  ["/settings", { icon: Settings }],
  ["/proxy-settings", { icon: Globe }],
]);

type SidebarNavItem = {
  href: TopLevelRoutePath;
  icon: LucideIcon;
};

const NavItem = memo(({
  item,
  isActive,
  isSidebarOpen,
  onNavigate,
  itemName,
}: {
  item: SidebarNavItem,
  isActive: boolean,
  isSidebarOpen: boolean,
  onNavigate: (href: string, event: MouseEvent<HTMLAnchorElement>) => void,
  itemName: string,
}) => (
  <a
    href={buildStaticRouteUrl(item.href)}
    onClick={(event) => onNavigate(item.href, event)}
    aria-current={isActive ? "page" : undefined}
    aria-label={itemName}
    title={itemName}
    className={cn(
      "group/nav relative flex min-h-10 items-center gap-3 overflow-hidden rounded-[12px] px-3.5 py-1.5 text-dense font-medium transition-[background-color,color] duration-200 ease-out hover:bg-foreground/[0.045] hover:text-foreground xl:min-h-11 xl:gap-3.5 xl:px-3.5 xl:py-2 [@media(max-height:800px)]:min-h-9 [@media(max-height:800px)]:gap-2.5 [@media(max-height:800px)]:px-3 [@media(max-height:800px)]:py-1",
      !isSidebarOpen && "justify-center px-0",
      isActive
        ? "bg-foreground/[0.06] text-foreground shadow-[inset_0_0_0_1px_rgb(var(--foreground-rgb)/0.035)] [@media(max-height:800px)]:min-h-11"
        : "text-muted-foreground",
    )}
  >
    <item.icon className={cn("h-[17px] w-[17px] shrink-0 xl:h-[18px] xl:w-[18px]", isActive && "text-primary")} />
    {isSidebarOpen && (
      <>
        <span className="min-w-0 truncate">{itemName}</span>
        {isActive ? <span className="ml-auto h-1.5 w-1.5 rounded-full bg-primary" /> : null}
      </>
    )}
  </a>
));

NavItem.displayName = "NavItem";

/**
 * 函数 `Sidebar`
 *
 * 作者: gaohongshun
 *
 * 时间: 2026-04-02
 *
 * # 参数
 * 无
 *
 * # 返回
 * 返回函数执行结果
 */
export function Sidebar() {
  const { t } = useI18n();
  const [logoFailed, setLogoFailed] = useState(false);
  const isSidebarOpen = useAppStore((state) => state.isSidebarOpen);
  const currentShellPath = useAppStore((state) => state.currentShellPath);
  const toggleSidebar = useAppStore((state) => state.toggleSidebar);
  const openCodexCliGuide = useAppStore((state) => state.openCodexCliGuide);
  const navigateShellPath = useAppStore((state) => state.navigateShellPath);
  const { isDesktopRuntime } = useRuntimeCapabilities();
  const { data: session, isLoading: isSessionLoading } = useAppSession();
  const role = resolveSessionRole(session, isSessionLoading, isDesktopRuntime);
  const brandTitle = isSidebarOpen ? t("重新打开 Codex 引导") : APP_DISPLAY_NAME;
  const toggleTitle = isSidebarOpen ? t("收起侧边栏") : t("展开侧边栏");
  const routeAccess = useMemo(
    () => ({ role, mode: session?.mode ?? null, isDesktopRuntime }),
    [isDesktopRuntime, role, session?.mode],
  );

  const handleNavigate = useCallback(
    (href: string, event: MouseEvent<HTMLAnchorElement>) => {
      if (
        event.defaultPrevented ||
        event.button !== 0 ||
        event.metaKey ||
        event.ctrlKey ||
        event.shiftKey ||
        event.altKey
      ) {
        return;
      }

      if (href === currentShellPath) {
        event.preventDefault();
        return;
      }

      event.preventDefault();
      navigateShellPath(href);
    },
    [currentShellPath, navigateShellPath],
  );

  const renderedItems = useMemo(() => {
    const items: SidebarNavItem[] = getAllowedTopLevelRoutes(routeAccess).flatMap(
      (route) => {
        const item = NAV_ITEM_BY_PATH.get(route.path);
        if (!item) return [];
        return [{ href: route.path, icon: item.icon }];
      },
    );

    return (
      <div
        className={cn(
          "grid",
          isDesktopRuntime
            ? "gap-2 xl:gap-2.5 [@media(max-height:800px)]:gap-2"
            : "gap-1.5 xl:gap-2 [@media(max-height:800px)]:gap-1",
        )}
      >
        {items.map((item) => {
          const itemName = t(getTopLevelRouteLabel(item.href, routeAccess));
          return (
            <NavItem
              key={item.href}
              item={item}
              itemName={itemName}
              isActive={item.href === currentShellPath}
              isSidebarOpen={isSidebarOpen}
              onNavigate={handleNavigate}
            />
          );
        })}
      </div>
    );
  }, [currentShellPath, handleNavigate, isDesktopRuntime, isSidebarOpen, routeAccess, t]);

  return (
    <div
      data-slot="app-sidebar"
      className={cn(
        "relative z-20 flex shrink-0 flex-col glass-sidebar",
        isSidebarOpen ? "w-[224px] xl:w-[252px]" : "w-[64px] xl:w-[68px]"
      )}
    >
      <div
        aria-hidden="true"
        data-slot="app-sidebar-motion-edge"
        className={cn(
          "pointer-events-none absolute inset-y-0 left-0 z-20 w-px bg-border/70 transition-transform duration-300 ease-out will-change-transform motion-reduce:transition-none",
          isSidebarOpen
            ? "translate-x-[calc(224px-1px)] xl:translate-x-[calc(252px-1px)]"
            : "translate-x-[calc(64px-1px)] xl:translate-x-[calc(68px-1px)]",
        )}
      />
      <div
        className={cn(
          "flex h-[72px] shrink-0 items-center border-b border-border/45 xl:h-[84px] [@media(max-height:800px)]:h-[68px]",
          isSidebarOpen ? "px-4 xl:px-5" : "px-2 xl:px-2.5"
        )}
      >
        <Button
          type="button"
          variant="ghost"
          onClick={openCodexCliGuide}
          title={brandTitle}
          aria-label={brandTitle}
          className={cn(
            "flex h-auto w-full items-center gap-2.5 overflow-hidden rounded-xl px-0 py-1.5 transition-colors duration-200 hover:bg-foreground/[0.035] focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/30 xl:gap-3 xl:py-2",
            isSidebarOpen ? "justify-start text-left" : "justify-center"
          )}
        >
          <div className="flex h-9 w-9 shrink-0 items-center justify-center overflow-hidden rounded-[11px] border border-border/70 bg-card text-primary shadow-[0_10px_24px_-20px_rgb(40_30_20/0.7)] xl:h-10 xl:w-10 [@media(max-height:800px)]:h-9 [@media(max-height:800px)]:w-9">
            {logoFailed ? (
              <span className="text-sm font-bold">T</span>
            ) : (
              <Image
                src="/team-relay.svg"
                alt={APP_DISPLAY_NAME}
                width={48}
                height={48}
                className="h-full w-full object-cover"
                onError={() => setLogoFailed(true)}
              />
            )}
          </div>
          {isSidebarOpen && (
            <div className="flex flex-col overflow-hidden animate-in fade-in slide-in-from-left-1 duration-200 motion-reduce:animate-none">
              <span className="truncate text-[17px] font-semibold tracking-[-0.025em] text-foreground">{APP_DISPLAY_NAME}</span>
              <span className="truncate text-[12px] text-muted-foreground xl:mt-0.5">
                {t("账号池 · 路由管理")}
              </span>
            </div>
          )}
        </Button>
      </div>

      <div className="flex-1 overflow-y-auto py-4 no-scrollbar xl:py-5 [@media(max-height:800px)]:py-2.5">
        <nav className="px-2.5 xl:px-3">
          {renderedItems}
        </nav>
      </div>

      <div
        className={cn(
          "shrink-0 border-t border-border/45 p-2.5",
          !isSidebarOpen && "flex justify-center",
        )}
      >
        <Button
          variant="ghost"
          size="icon"
          className={cn(
            "h-9 rounded-lg border border-transparent text-muted-foreground hover:border-border/70 hover:bg-foreground/[0.035] hover:text-foreground",
            isSidebarOpen
              ? "w-full justify-start gap-3 px-3"
              : "w-9 justify-center px-0",
          )}
          title={toggleTitle}
          aria-label={toggleTitle}
          onClick={toggleSidebar}
        >
          {isSidebarOpen ? (
            <>
              <ChevronLeft className="h-4 w-4 shrink-0" />
              <span className="text-sm">{t("收起侧边栏")}</span>
            </>
          ) : (
            <ChevronRight className="h-4 w-4 shrink-0" />
          )}
        </Button>
      </div>
    </div>
  );
}
