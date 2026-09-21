import { APP_DISPLAY_NAME } from "@/lib/branding";
import type { Metadata } from "next";
import "./globals.css";
import { AppFrame } from "@/components/layout/app-frame";
import { Providers } from "@/components/providers";
import { AppBootstrap } from "@/components/layout/app-bootstrap";
import {
  appearanceInitScript,
  DEFAULT_APPEARANCE_PRESET,
} from "@/lib/appearance";

export const metadata: Metadata = {
  title: APP_DISPLAY_NAME,
  description: "团队共享的模型接入、账号池与用量管理服务",
};

const trayPreviewModeInitScript = `
(() => {
  try {
    if (window.location.pathname.replace(/\\/$/, "") === "/tray-preview") {
      document.documentElement.classList.add("tray-preview-mode");
    }
  } catch (_error) {}
})();
`;

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode;
}>) {
  return (
    <html
      lang="zh-CN"
      suppressHydrationWarning
      data-appearance={DEFAULT_APPEARANCE_PRESET}
    >
      <body className="antialiased">
        <script dangerouslySetInnerHTML={{ __html: trayPreviewModeInitScript }} />
        <script dangerouslySetInnerHTML={{ __html: appearanceInitScript }} />
        <Providers>
          <AppBootstrap>
            <AppFrame>{children}</AppFrame>
          </AppBootstrap>
        </Providers>
      </body>
    </html>
  );
}
