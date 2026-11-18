use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};

pub(crate) const DAY_SECONDS: i64 = 24 * 60 * 60;
pub(crate) const BUSINESS_TIME_ZONE: &str = "Asia/Shanghai";
const BUSINESS_UTC_OFFSET_SECONDS: i32 = 8 * 60 * 60;

fn business_offset() -> FixedOffset {
    FixedOffset::east_opt(BUSINESS_UTC_OFFSET_SECONDS)
        .unwrap_or_else(|| panic!("{BUSINESS_TIME_ZONE} has a fixed UTC+08:00 offset"))
}

pub(crate) fn business_day_bounds_for_ts(now_ts: i64) -> Result<(i64, i64), String> {
    let now = DateTime::<Utc>::from_timestamp(now_ts, 0)
        .ok_or_else(|| "invalid business-day timestamp".to_string())?
        .with_timezone(&business_offset());
    let today = now.date_naive();
    let start_naive = today
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| "build business-day start failed".to_string())?;
    let tomorrow_naive = (today + Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .ok_or_else(|| "build business-day end failed".to_string())?;
    let start = business_offset()
        .from_local_datetime(&start_naive)
        .single()
        .ok_or_else(|| "resolve business-day start failed".to_string())?
        .timestamp();
    let end = business_offset()
        .from_local_datetime(&tomorrow_naive)
        .single()
        .ok_or_else(|| "resolve business-day end failed".to_string())?
        .timestamp();
    Ok((start, end.max(start)))
}

pub(crate) fn business_day_bounds_ts() -> Result<(i64, i64), String> {
    business_day_bounds_for_ts(Utc::now().timestamp())
}

pub(crate) fn resolve_optional_business_day_bounds_ts(
    day_start_ts: Option<i64>,
    day_end_ts: Option<i64>,
    now_ts: i64,
) -> Result<(i64, i64), String> {
    match (
        day_start_ts.filter(|value| *value > 0),
        day_end_ts.filter(|value| *value > 0),
    ) {
        (Some(start), Some(end)) if end > start => Ok((start, end)),
        _ => business_day_bounds_for_ts(now_ts),
    }
}

pub(crate) fn resolve_bounded_business_day_bounds_ts(
    day_start_ts: Option<i64>,
    day_end_ts: Option<i64>,
    max_range_seconds: i64,
) -> Result<(i64, i64), String> {
    match (day_start_ts, day_end_ts) {
        (Some(start), Some(end)) => {
            if end <= start {
                return Err("dayEndTs must be greater than dayStartTs".to_string());
            }
            if end - start > max_range_seconds {
                return Err("requested day range is too large".to_string());
            }
            Ok((start, end))
        }
        (None, None) => business_day_bounds_ts(),
        _ => Err("dayStartTs and dayEndTs must be provided together".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn business_day_is_aligned_to_shanghai_midnight_in_utc() {
        let now_ts = 1_793_265_895; // 2026-10-29 09:24:55 UTC
        let (start, end) = business_day_bounds_for_ts(now_ts).expect("business day bounds");

        assert_eq!(BUSINESS_TIME_ZONE, "Asia/Shanghai");
        assert_eq!(start, 1_793_203_200); // 2026-10-28 16:00:00 UTC
        assert_eq!(end, 1_793_289_600); // 2026-10-29 16:00:00 UTC
    }

    #[test]
    fn omitted_bounds_use_shanghai_day_and_explicit_bounds_are_preserved() {
        let now_ts = 1_793_265_895;
        let fallback =
            resolve_optional_business_day_bounds_ts(None, None, now_ts).expect("fallback bounds");
        assert_eq!(fallback, (1_793_203_200, 1_793_289_600));
        assert_eq!(
            resolve_optional_business_day_bounds_ts(Some(11), Some(22), now_ts)
                .expect("explicit bounds"),
            (11, 22)
        );
        let bounded = resolve_bounded_business_day_bounds_ts(None, None, DAY_SECONDS)
            .expect("bounded fallback");
        assert_eq!(bounded.1 - bounded.0, DAY_SECONDS);
        assert!(resolve_bounded_business_day_bounds_ts(Some(1), None, DAY_SECONDS).is_err());
        assert!(resolve_bounded_business_day_bounds_ts(
            Some(1),
            Some(DAY_SECONDS + 2),
            DAY_SECONDS
        )
        .is_err());
    }
}
