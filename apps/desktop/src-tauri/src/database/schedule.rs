use chrono::{DateTime, Datelike, Days, LocalResult, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use super::DigitalColleagueScheduleRecord;

pub const DEFAULT_SCHEDULE_CATCHUP_SECONDS: i64 = 3_600;
pub const MIN_SCHEDULE_INTERVAL_SECONDS: i64 = 60;
pub const MAX_SCHEDULE_INTERVAL_SECONDS: i64 = 2_592_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DigitalColleagueScheduleSpec {
    #[serde(rename = "once")]
    Once { at_ms: i64, timezone: String },
    #[serde(rename = "interval")]
    Interval {
        interval_seconds: i64,
        timezone: String,
    },
    #[serde(rename = "daily")]
    Daily {
        weekdays: Vec<u8>,
        local_time: String,
        timezone: String,
    },
    #[serde(rename = "weekly")]
    Weekly {
        weekdays: Vec<u8>,
        local_time: String,
        timezone: String,
    },
}

impl DigitalColleagueScheduleSpec {
    pub fn canonicalized(mut self) -> Result<Self, String> {
        let timezone = canonical_timezone(self.timezone())?;
        match &mut self {
            Self::Once {
                timezone: current, ..
            }
            | Self::Interval {
                timezone: current, ..
            }
            | Self::Daily {
                timezone: current, ..
            }
            | Self::Weekly {
                timezone: current, ..
            } => *current = timezone,
        }
        Ok(self)
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Once { .. } => "once",
            Self::Interval { .. } => "interval",
            Self::Daily { .. } => "daily",
            Self::Weekly { .. } => "weekly",
        }
    }

    pub fn timezone(&self) -> &str {
        match self {
            Self::Once { timezone, .. }
            | Self::Interval { timezone, .. }
            | Self::Daily { timezone, .. }
            | Self::Weekly { timezone, .. } => timezone,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        normalize_timezone(self.timezone())?;
        match self {
            Self::Once { at_ms, .. } if *at_ms < 0 => {
                Err("digital_colleague.schedule_once_invalid".to_owned())
            }
            Self::Once { .. } => Ok(()),
            Self::Interval {
                interval_seconds, ..
            } if !(MIN_SCHEDULE_INTERVAL_SECONDS..=MAX_SCHEDULE_INTERVAL_SECONDS)
                .contains(interval_seconds) =>
            {
                Err("digital_colleague.schedule_interval_invalid".to_owned())
            }
            Self::Interval { .. } => Ok(()),
            Self::Daily {
                weekdays,
                local_time,
                ..
            } => validate_calendar(weekdays, local_time, true),
            Self::Weekly {
                weekdays,
                local_time,
                ..
            } => validate_calendar(weekdays, local_time, false),
        }
    }

    pub fn from_record(record: &DigitalColleagueScheduleRecord) -> Result<Self, String> {
        let timezone = record.timezone.clone();
        match record.schedule_kind.as_str() {
            "once" => Ok(Self::Once {
                at_ms: record
                    .once_at
                    .ok_or_else(|| "digital_colleague.schedule_once_missing".to_owned())?,
                timezone,
            }),
            "interval" => Ok(Self::Interval {
                interval_seconds: record
                    .interval_seconds
                    .ok_or_else(|| "digital_colleague.schedule_interval_missing".to_owned())?,
                timezone,
            }),
            "daily" | "weekly" => {
                let weekdays = record
                    .weekdays
                    .clone()
                    .ok_or_else(|| "digital_colleague.schedule_weekdays_missing".to_owned())?;
                let local_time = record
                    .local_time
                    .clone()
                    .ok_or_else(|| "digital_colleague.schedule_time_missing".to_owned())?;
                if record.schedule_kind == "daily" {
                    Ok(Self::Daily {
                        weekdays,
                        local_time,
                        timezone,
                    })
                } else {
                    Ok(Self::Weekly {
                        weekdays,
                        local_time,
                        timezone,
                    })
                }
            }
            _ => Err("digital_colleague.schedule_kind_unsupported".to_owned()),
        }
    }

    /// OpenMausBot server/routine-schedules.ts nextOccurrence supplies the
    /// strictly-after control flow; routine-intervals.ts nextAlignedInterval
    /// supplies the anchored interval. Hermes cron/jobs.py compute_next_run
    /// supplies UTC duration arithmetic and pinned-zone wall-clock handling.
    pub fn next_after(
        &self,
        anchor_at_ms: Option<i64>,
        after_ms: i64,
    ) -> Result<Option<i64>, String> {
        self.validate()?;
        match self {
            Self::Once { at_ms, .. } => Ok((*at_ms > after_ms).then_some(*at_ms)),
            Self::Interval {
                interval_seconds, ..
            } => {
                let anchor = anchor_at_ms
                    .ok_or_else(|| "digital_colleague.schedule_anchor_missing".to_owned())?;
                if anchor > after_ms {
                    return Ok(Some(anchor));
                }
                let interval_ms = i128::from(*interval_seconds) * 1_000;
                let elapsed = i128::from(after_ms) - i128::from(anchor);
                let candidate = i128::from(anchor) + (elapsed / interval_ms + 1) * interval_ms;
                i64::try_from(candidate)
                    .map(Some)
                    .map_err(|_| "digital_colleague.schedule_overflow".to_owned())
            }
            Self::Daily {
                weekdays,
                local_time,
                timezone,
            }
            | Self::Weekly {
                weekdays,
                local_time,
                timezone,
            } => {
                let zone = normalize_timezone(timezone)?;
                let local_time = NaiveTime::parse_from_str(local_time, "%H:%M")
                    .map_err(|_| "digital_colleague.schedule_time_invalid".to_owned())?;
                let after = utc_datetime(after_ms)?;
                let after_local = after.with_timezone(&zone);
                // At least two weeks: a weekly occurrence that lands in a DST
                // gap must skip that week and still find the next valid one,
                // instead of returning None and disabling the plan.
                const CALENDAR_SEARCH_DAYS: u64 = 16;
                let mut candidates = Vec::new();
                for day_offset in 0..=CALENDAR_SEARCH_DAYS {
                    let date = after_local
                        .date_naive()
                        .checked_add_days(Days::new(day_offset))
                        .ok_or_else(|| "digital_colleague.schedule_overflow".to_owned())?;
                    let weekday = date.weekday().num_days_from_sunday() as u8;
                    if !weekdays.contains(&weekday) {
                        continue;
                    }
                    // Conservative DST policy: a nonexistent wall time is skipped,
                    // never approximated. Both repeated folds are considered and
                    // the earliest instant strictly after the base wins.
                    candidates.extend(local_instants(date.and_time(local_time), zone));
                }
                candidates.sort_unstable();
                candidates.dedup();
                Ok(candidates
                    .into_iter()
                    .find(|candidate| *candidate > after_ms))
            }
        }
    }
}

pub fn normalize_timezone(value: &str) -> Result<Tz, String> {
    // Adapted from Hermes cron/jobs_time.py normalize_timezone/zone_for_job:
    // validate the IANA key before persistence, never use the host zone.
    value
        .trim()
        .parse::<Tz>()
        .map_err(|_| "digital_colleague.schedule_timezone_invalid".to_owned())
}

pub fn canonical_timezone(value: &str) -> Result<String, String> {
    Ok(normalize_timezone(value)?.name().to_owned())
}

pub fn resolved_device_timezone() -> Result<String, String> {
    canonical_timezone(&iana_time_zone::get_timezone().map_err(|error| error.to_string())?)
}

pub fn validate_catchup_window(seconds: i64) -> Result<(), String> {
    if (60..=86_400).contains(&seconds) {
        Ok(())
    } else {
        Err("digital_colleague.schedule_catchup_invalid".to_owned())
    }
}

/// Adapted from Hermes cron/jobs_time.py oneshot_instant. Naive input is a
/// wall clock in the plan's pinned IANA zone. Gaps are rejected; folds select
/// the earliest instant strictly after the supplied base.
pub fn oneshot_instant(wall: NaiveDateTime, timezone: &str, after_ms: i64) -> Result<i64, String> {
    let zone = normalize_timezone(timezone)?;
    local_instants(wall, zone)
        .into_iter()
        .filter(|candidate| *candidate > after_ms)
        .min()
        .ok_or_else(|| "digital_colleague.schedule_once_invalid".to_owned())
}

pub fn local_day_start_ms(now_ms: i64, timezone: &str) -> Result<i64, String> {
    let zone = normalize_timezone(timezone)?;
    let local_date = utc_datetime(now_ms)?.with_timezone(&zone).date_naive();
    local_instants(
        local_date
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| "digital_colleague.schedule_time_invalid".to_owned())?,
        zone,
    )
    .into_iter()
    .min()
    .ok_or_else(|| "digital_colleague.schedule_local_midnight_invalid".to_owned())
}

fn validate_calendar(weekdays: &[u8], local_time: &str, daily: bool) -> Result<(), String> {
    NaiveTime::parse_from_str(local_time, "%H:%M")
        .map_err(|_| "digital_colleague.schedule_time_invalid".to_owned())?;
    let mut cleaned = weekdays.to_vec();
    cleaned.sort_unstable();
    cleaned.dedup();
    if cleaned.len() != weekdays.len()
        || cleaned.is_empty()
        || cleaned.iter().any(|weekday| *weekday > 6)
        || (daily && cleaned != vec![0, 1, 2, 3, 4, 5, 6])
    {
        return Err("digital_colleague.schedule_weekdays_invalid".to_owned());
    }
    Ok(())
}

fn local_instants(wall: NaiveDateTime, zone: Tz) -> Vec<i64> {
    match zone.from_local_datetime(&wall) {
        LocalResult::Single(value) => vec![value.timestamp_millis()],
        LocalResult::Ambiguous(first, second) => {
            vec![first.timestamp_millis(), second.timestamp_millis()]
        }
        LocalResult::None => Vec::new(),
    }
}

fn utc_datetime(timestamp_ms: i64) -> Result<DateTime<Utc>, String> {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .ok_or_else(|| "digital_colleague.schedule_timestamp_invalid".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instant(value: &str) -> i64 {
        DateTime::parse_from_rfc3339(value)
            .unwrap()
            .timestamp_millis()
    }

    fn daily(timezone: &str, local_time: &str) -> DigitalColleagueScheduleSpec {
        DigitalColleagueScheduleSpec::Daily {
            weekdays: vec![0, 1, 2, 3, 4, 5, 6],
            local_time: local_time.to_owned(),
            timezone: timezone.to_owned(),
        }
    }

    #[test]
    fn once_is_strictly_after_the_base() {
        let schedule = DigitalColleagueScheduleSpec::Once {
            at_ms: instant("2026-11-12T09:00:00+08:00"),
            timezone: "Asia/Shanghai".to_owned(),
        };
        assert_eq!(
            schedule
                .next_after(None, instant("2026-11-12T00:59:59Z"))
                .unwrap(),
            Some(instant("2026-11-12T01:00:00Z"))
        );
        assert_eq!(
            schedule
                .next_after(None, instant("2026-11-12T01:00:00Z"))
                .unwrap(),
            None
        );
    }

    #[test]
    fn interval_preserves_utc_duration_across_dst() {
        let schedule = DigitalColleagueScheduleSpec::Interval {
            interval_seconds: 3_600,
            timezone: "America/New_York".to_owned(),
        };
        let anchor = instant("2026-11-01T04:30:00Z");
        assert_eq!(
            schedule
                .next_after(Some(anchor), instant("2026-11-01T05:15:00Z"))
                .unwrap(),
            Some(instant("2026-11-01T05:30:00Z"))
        );
        assert_eq!(
            schedule
                .next_after(Some(anchor), instant("2026-11-01T05:45:00Z"))
                .unwrap(),
            Some(instant("2026-11-01T06:30:00Z"))
        );
    }

    #[test]
    fn shanghai_daily_crosses_local_midnight_not_utc() {
        let schedule = daily("Asia/Shanghai", "00:00");
        assert_eq!(
            schedule
                .next_after(None, instant("2026-11-11T16:00:00Z"))
                .unwrap(),
            Some(instant("2026-11-12T16:00:00Z"))
        );
        assert_eq!(
            local_day_start_ms(instant("2026-11-12T00:30:00+08:00"), "Asia/Shanghai").unwrap(),
            instant("2026-11-11T16:00:00Z")
        );
    }

    #[test]
    fn weekly_uses_weekday_set_and_crosses_a_week() {
        let schedule = DigitalColleagueScheduleSpec::Weekly {
            weekdays: vec![5],
            local_time: "15:00".to_owned(),
            timezone: "Asia/Shanghai".to_owned(),
        };
        assert_eq!(
            schedule
                .next_after(None, instant("2026-11-13T07:00:00Z"))
                .unwrap(),
            Some(instant("2026-11-20T07:00:00Z"))
        );
    }

    #[test]
    fn weekly_gap_finds_the_next_valid_week_instead_of_retiring() {
        let schedule = DigitalColleagueScheduleSpec::Weekly {
            weekdays: vec![0],
            local_time: "02:30".to_owned(),
            timezone: "America/New_York".to_owned(),
        };
        // 2026-03-01 02:30 EST is the base. 2026-03-08 02:30 falls in the
        // spring-forward gap; the next valid Sunday is 2026-03-15 02:30 EDT.
        assert_eq!(
            schedule
                .next_after(None, instant("2026-03-01T07:30:00Z"))
                .unwrap(),
            Some(instant("2026-03-15T06:30:00Z"))
        );
    }

    #[test]
    fn new_york_gap_is_skipped_without_approximation() {
        let schedule = daily("America/New_York", "02:30");
        assert_eq!(
            schedule
                .next_after(None, instant("2026-03-07T12:00:00Z"))
                .unwrap(),
            Some(instant("2026-03-09T06:30:00Z"))
        );
    }

    #[test]
    fn new_york_fold_selects_earliest_instant_strictly_after_base() {
        let schedule = daily("America/New_York", "01:30");
        assert_eq!(
            schedule
                .next_after(None, instant("2026-11-01T05:30:00Z"))
                .unwrap(),
            Some(instant("2026-11-01T06:30:00Z"))
        );
        assert_eq!(
            schedule
                .next_after(None, instant("2026-11-01T06:30:00Z"))
                .unwrap(),
            Some(instant("2026-11-02T06:30:00Z"))
        );
        let wall =
            NaiveDateTime::parse_from_str("2026-11-01T01:30:00", "%Y-%m-%dT%H:%M:%S").unwrap();
        assert_eq!(
            oneshot_instant(wall, "America/New_York", instant("2026-11-01T05:30:00Z")).unwrap(),
            instant("2026-11-01T06:30:00Z")
        );
    }

    #[test]
    fn invalid_inputs_are_rejected_before_persistence() {
        assert!(normalize_timezone("Mars/Olympus").is_err());
        assert!(daily("Asia/Shanghai", "24:00").validate().is_err());
        assert!(DigitalColleagueScheduleSpec::Weekly {
            weekdays: vec![],
            local_time: "09:00".to_owned(),
            timezone: "UTC".to_owned()
        }
        .validate()
        .is_err());
        assert!(DigitalColleagueScheduleSpec::Interval {
            interval_seconds: 59,
            timezone: "UTC".to_owned()
        }
        .validate()
        .is_err());
        assert!(DigitalColleagueScheduleSpec::Interval {
            interval_seconds: 2_592_001,
            timezone: "UTC".to_owned()
        }
        .validate()
        .is_err());
        assert_eq!(DEFAULT_SCHEDULE_CATCHUP_SECONDS, 3_600);
    }
}
