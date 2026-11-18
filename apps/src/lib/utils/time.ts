"use client";

export interface BusinessDayRange {
  dayStartTs: number;
  dayEndTs: number;
  timeZone: string | null;
}

export const BUSINESS_TIME_ZONE = "Asia/Shanghai";
const BUSINESS_TIME_ZONE_OFFSET_MS = 8 * 60 * 60 * 1000;

function getPreferredLocale(): string | undefined {
  if (typeof document !== "undefined") {
    const lang = document.documentElement.lang.trim();
    if (lang) {
      return lang;
    }
  }
  if (typeof navigator !== "undefined") {
    const language = String(navigator.language || "").trim();
    if (language) {
      return language;
    }
  }
  return undefined;
}

export function getBrowserTimeZone(): string | null {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || null;
  } catch {
    return null;
  }
}

export function getBusinessDayRange(referenceDate = new Date()): BusinessDayRange {
  const shanghaiTime = new Date(referenceDate.getTime() + BUSINESS_TIME_ZONE_OFFSET_MS);
  const startMs =
    Date.UTC(
      shanghaiTime.getUTCFullYear(),
      shanghaiTime.getUTCMonth(),
      shanghaiTime.getUTCDate(),
    ) - BUSINESS_TIME_ZONE_OFFSET_MS;

  return {
    dayStartTs: Math.floor(startMs / 1000),
    dayEndTs: Math.floor((startMs + 24 * 60 * 60 * 1000) / 1000),
    timeZone: BUSINESS_TIME_ZONE,
  };
}

export function formatLocalDateTimeFromSeconds(
  timestamp: number | null | undefined,
  emptyLabel = "未知",
): string {
  if (!timestamp) return emptyLabel;
  const date = new Date(timestamp * 1000);
  if (Number.isNaN(date.getTime())) return emptyLabel;

  return new Intl.DateTimeFormat(getPreferredLocale(), {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hour12: false,
  }).format(date);
}

export function formatLocalMinuteFromSeconds(
  timestamp: number | null | undefined,
  emptyLabel = "未知",
): string {
  if (!timestamp) return emptyLabel;
  const date = new Date(timestamp * 1000);
  if (Number.isNaN(date.getTime())) return emptyLabel;

  return new Intl.DateTimeFormat(getPreferredLocale(), {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  }).format(date);
}
