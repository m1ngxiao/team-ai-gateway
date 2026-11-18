"use client";

import { useEffect, useState } from "react";
import { getBusinessDayRange, type BusinessDayRange } from "@/lib/utils/time";

/**
 * 函数 `useBusinessDayRange`
 *
 * 作者: gaohongshun
 *
 * 时间: 2026-04-13
 *
 * # 参数
 * 无
 *
 * # 返回
 * 返回团队统一业务时区 Asia/Shanghai 对应的当天时间范围
 */
export function useBusinessDayRange(): BusinessDayRange {
  const [dayRange, setDayRange] = useState<BusinessDayRange>(() => getBusinessDayRange());

  useEffect(() => {
    const refresh = () => {
      setDayRange((current) => {
        const next = getBusinessDayRange();
        if (
          current.dayStartTs === next.dayStartTs &&
          current.dayEndTs === next.dayEndTs &&
          current.timeZone === next.timeZone
        ) {
          return current;
        }
        return next;
      });
    };

    refresh();
    const intervalId = window.setInterval(refresh, 60_000);
    return () => window.clearInterval(intervalId);
  }, []);

  return dayRange;
}
