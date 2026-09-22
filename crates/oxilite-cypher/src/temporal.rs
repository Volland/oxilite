//! Cypher temporal values: `date`, `localtime`, `time`, `localdatetime`, `datetime` and
//! `duration`, with openCypher's construction, projection, truncation, arithmetic, component
//! access and ISO 8601 rendering (Java's, as the TCK expects).
//!
//! Dates are day numbers from 1970-01-01 (proleptic Gregorian, any `i64` year), times are
//! nanoseconds of the day. Named time zones resolve through the IANA database bundled by
//! `jiff`.
//!
// @lat: [[architecture#Property graph frontend#Mapping]]

use crate::error::{CypherError, Result};
use crate::value::{TemporalKind, Value};
use std::cmp::Ordering;
use std::collections::BTreeMap;

const NS: i64 = 1_000_000_000;
const DAY_NS: i64 = 86_400 * NS;
/// Seconds in an average Gregorian month (Java `Duration` / Neo4j).
const MONTH_SECONDS: i128 = 2_629_746;

fn rt(msg: impl Into<String>) -> CypherError {
    CypherError::runtime(msg)
}

// ----- calendar -----

/// Days from 1970-01-01 (Howard Hinnant's algorithm).
pub(crate) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub(crate) fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: i64) -> i64 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
    }
}

/// ISO weekday: Monday = 1.
fn weekday(days: i64) -> i64 {
    (days + 3).rem_euclid(7) + 1
}

fn ordinal(days: i64) -> i64 {
    let (y, _, _) = civil_from_days(days);
    days - days_from_civil(y, 1, 1) + 1
}

/// ISO week-year and week number.
fn iso_week(days: i64) -> (i64, i64) {
    let thursday = days + (4 - weekday(days));
    let (wy, _, _) = civil_from_days(thursday);
    let week = (thursday - days_from_civil(wy, 1, 1)) / 7 + 1;
    (wy, week)
}

fn week1_monday(wy: i64) -> i64 {
    let jan4 = days_from_civil(wy, 1, 4);
    jan4 - (weekday(jan4) - 1)
}

fn weeks_in_year(wy: i64) -> i64 {
    (week1_monday(wy + 1) - week1_monday(wy)) / 7
}

fn quarter_start(y: i64, q: i64) -> i64 {
    days_from_civil(y, (q - 1) * 3 + 1, 1)
}

fn add_months(days: i64, months: i64) -> i64 {
    let (y, m, d) = civil_from_days(days);
    let total = y * 12 + (m - 1) + months;
    let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) + 1);
    days_from_civil(ny, nm, d.min(days_in_month(ny, nm)))
}

// ----- zones -----

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Zone {
    Offset(i32),
    Named(String),
}

/// With `tzdb-bundle`, zones come from the embedded database even where the host has
/// `/usr/share/zoneinfo`: distributions build it differently (Debian and Ubuntu keep
/// `backzone`, so pre-1970 `Europe/Stockholm` is its own LMT, not `Europe/Berlin`'s), and
/// query results must not depend on the machine.
#[cfg(feature = "tzdb-bundle")]
fn tz_db() -> jiff::tz::TimeZoneDatabase {
    jiff::tz::TimeZoneDatabase::bundled()
}

#[cfg(not(feature = "tzdb-bundle"))]
fn tz_db() -> jiff::tz::TimeZoneDatabase {
    jiff::tz::db().clone()
}

fn tz(name: &str) -> Result<jiff::tz::TimeZone> {
    tz_db()
        .get(name)
        .map_err(|_| rt(format!("unknown time zone '{name}'")))
}

/// The offset of a named zone at a local date-time (earlier offset in gaps/overlaps, like
/// Java's `ZonedDateTime.of`).
fn offset_at_local(name: &str, days: i64, nanos: i64) -> Result<i32> {
    let z = tz(name)?;
    let (y, m, d) = civil_from_days(days);
    let (h, mi, s, n) = split_time(nanos);
    let y16 = i16::try_from(y).map_err(|_| rt("date out of the time zone database range"))?;
    let dt =
        jiff::civil::DateTime::new(y16, m as i8, d as i8, h as i8, mi as i8, s as i8, n as i32)
            .map_err(|e| rt(e.to_string()))?;
    let amb = z.to_ambiguous_zoned(dt);
    let zoned = amb.compatible().map_err(|e| rt(e.to_string()))?;
    Ok(zoned.offset().seconds())
}

/// The offset of a named zone at an instant (epoch seconds).
fn offset_at_instant(name: &str, epoch_seconds: i64) -> Result<i32> {
    let z = tz(name)?;
    let ts = jiff::Timestamp::from_second(epoch_seconds).map_err(|e| rt(e.to_string()))?;
    Ok(z.to_offset(ts).seconds())
}

fn parse_zone(s: &str) -> Result<Zone> {
    if s == "Z" || s == "z" {
        return Ok(Zone::Offset(0));
    }
    if s.starts_with('+') || s.starts_with('-') {
        return parse_offset(s).map(Zone::Offset);
    }
    tz(s)?;
    Ok(Zone::Named(s.to_string()))
}

fn parse_offset(s: &str) -> Result<i32> {
    let bad = || rt(format!("invalid time zone offset '{s}'"));
    if s == "Z" {
        return Ok(0);
    }
    let sign = match s.as_bytes().first() {
        Some(b'+') => 1,
        Some(b'-') => -1,
        _ => return Err(bad()),
    };
    let digits: String = s[1..].chars().filter(|c| *c != ':').collect();
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return Err(bad());
    }
    let n = |r: std::ops::Range<usize>| digits.get(r).and_then(|x| x.parse::<i32>().ok());
    let (h, m, sec) = match digits.len() {
        2 => (n(0..2).ok_or_else(bad)?, 0, 0),
        4 => (n(0..2).ok_or_else(bad)?, n(2..4).ok_or_else(bad)?, 0),
        6 => (
            n(0..2).ok_or_else(bad)?,
            n(2..4).ok_or_else(bad)?,
            n(4..6).ok_or_else(bad)?,
        ),
        _ => return Err(bad()),
    };
    if h > 18 || m > 59 || sec > 59 {
        return Err(bad());
    }
    Ok(sign * (h * 3600 + m * 60 + sec))
}

fn fmt_offset(o: i32) -> String {
    if o == 0 {
        return "Z".into();
    }
    let sign = if o < 0 { '-' } else { '+' };
    let a = o.abs();
    let (h, m, s) = (a / 3600, (a / 60) % 60, a % 60);
    if s != 0 {
        format!("{sign}{h:02}:{m:02}:{s:02}")
    } else {
        format!("{sign}{h:02}:{m:02}")
    }
}

// ----- durations -----

/// `months`, `days`, `seconds` and `nanos` (0 ≤ nanos < 10⁹), like Neo4j.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Dur {
    pub months: i64,
    pub days: i64,
    pub seconds: i64,
    pub nanos: i64,
}

impl Dur {
    fn new(months: i64, days: i64, total_nanos: i128) -> Self {
        let s = total_nanos.div_euclid(NS as i128);
        let n = total_nanos.rem_euclid(NS as i128);
        Self {
            months,
            days,
            seconds: s as i64,
            nanos: n as i64,
        }
    }

    fn total_nanos(&self) -> i128 {
        self.seconds as i128 * NS as i128 + self.nanos as i128
    }

    /// Builds a duration from possibly fractional components, cascading fractions into
    /// smaller units (a month is 2 629 746 s, a day 86 400 s).
    fn from_parts(months: f64, days: f64, nanos: f64, exact: Option<(i64, i64, i128)>) -> Self {
        if let Some((m, d, n)) = exact {
            return Self::new(m, d, n);
        }
        let mi = months.trunc();
        let mf = months - mi;
        // Fractional months become days and nanoseconds.
        let month_ns = (mf * MONTH_SECONDS as f64 * NS as f64).round() as i128;
        let extra_days = month_ns / DAY_NS as i128;
        let month_rest = month_ns % DAY_NS as i128;
        let di = days.trunc();
        let df = days - di;
        let day_ns = (df * DAY_NS as f64).round() as i128;
        let total = month_rest + day_ns + nanos.trunc() as i128;
        Self::new(mi as i64, di as i64 + extra_days as i64, total)
    }

    fn render(self) -> String {
        if self == Self::default() {
            return "PT0S".into();
        }
        let mut s = String::from("P");
        let years = self.months / 12;
        let months = self.months % 12;
        if years != 0 {
            s.push_str(&format!("{years}Y"));
        }
        if months != 0 {
            s.push_str(&format!("{months}M"));
        }
        if self.days != 0 {
            s.push_str(&format!("{}D", self.days));
        }
        let total = self.total_nanos();
        if total != 0 {
            s.push('T');
            let neg = total < 0;
            let a = total.unsigned_abs();
            let secs = (a / NS as u128) as i128;
            let frac = (a % NS as u128) as i128;
            let (h, m, sec) = (secs / 3600, (secs / 60) % 60, secs % 60);
            let sign = if neg { "-" } else { "" };
            if h != 0 {
                s.push_str(&format!("{sign}{h}H"));
            }
            if m != 0 {
                s.push_str(&format!("{sign}{m}M"));
            }
            if sec != 0 || frac != 0 {
                let mut f = format!("{frac:09}");
                while f.ends_with('0') {
                    f.pop();
                }
                if f.is_empty() {
                    s.push_str(&format!("{sign}{sec}S"));
                } else {
                    s.push_str(&format!("{sign}{sec}.{f}S"));
                }
            }
        }
        s
    }

    fn component(&self, key: &str) -> Option<Value> {
        let n = self.total_nanos();
        let s = n.div_euclid(NS as i128) as i64;
        let ns = n.rem_euclid(NS as i128) as i64;
        Some(Value::Int(match key {
            "years" => self.months / 12,
            "quarters" => self.months / 3,
            "months" => self.months,
            "monthsofyear" => self.months % 12,
            "quartersofyear" => (self.months / 3) % 4,
            "monthsofquarter" => self.months % 3,
            "weeks" => self.days / 7,
            "days" => self.days,
            "daysofweek" => self.days % 7,
            "hours" => s / 3600,
            "minutes" => s / 60,
            "seconds" => s,
            "minutesofhour" => (s / 60) % 60,
            "secondsofminute" => s % 60,
            "milliseconds" => s * 1000 + ns / 1_000_000,
            "microseconds" => s * 1_000_000 + ns / 1000,
            "nanoseconds" => (n) as i64,
            "millisecondsofsecond" => ns / 1_000_000,
            "microsecondsofsecond" => ns / 1000,
            "nanosecondsofsecond" => ns,
            _ => return None,
        }))
    }

    fn neg(self) -> Self {
        Self::new(-self.months, -self.days, -self.total_nanos())
    }

    fn add(self, o: Self) -> Result<Self> {
        Ok(Self::new(
            self.months
                .checked_add(o.months)
                .ok_or_else(|| rt("duration overflow"))?,
            self.days
                .checked_add(o.days)
                .ok_or_else(|| rt("duration overflow"))?,
            self.total_nanos() + o.total_nanos(),
        ))
    }

    fn scale(self, f: f64) -> Self {
        let months = self.months as f64 * f;
        let days = self.days as f64 * f;
        let nanos = self.total_nanos() as f64 * f;
        Self::from_parts(months, days, nanos, None)
    }
}

// ----- temporal values -----

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum T {
    Date(i64),
    LocalTime(i64),
    Time(i64, i32),
    LocalDateTime(i64, i64),
    /// Local date and time, zone, and the zone's offset at that time.
    DateTime(i64, i64, Zone, i32),
    Duration(Dur),
}

fn split_time(nanos: i64) -> (i64, i64, i64, i64) {
    let s = nanos / NS;
    (s / 3600, (s / 60) % 60, s % 60, nanos % NS)
}

fn fmt_year(y: i64) -> String {
    if y > 9999 {
        format!("+{y}")
    } else if y < 0 {
        format!("-{:04}", -y)
    } else {
        format!("{y:04}")
    }
}

fn fmt_date(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{}-{m:02}-{d:02}", fmt_year(y))
}

fn fmt_time(nanos: i64) -> String {
    let (h, m, s, n) = split_time(nanos);
    let mut out = format!("{h:02}:{m:02}");
    if s != 0 || n != 0 {
        out.push_str(&format!(":{s:02}"));
    }
    if n != 0 {
        if n % 1_000_000 == 0 {
            out.push_str(&format!(".{:03}", n / 1_000_000));
        } else if n % 1000 == 0 {
            out.push_str(&format!(".{:06}", n / 1000));
        } else {
            out.push_str(&format!(".{n:09}"));
        }
    }
    out
}

/// xsd lexical forms always have seconds.
fn xsd_time(nanos: i64) -> String {
    let (h, m, s, n) = split_time(nanos);
    let mut out = format!("{h:02}:{m:02}:{s:02}");
    if n != 0 {
        let mut f = format!("{n:09}");
        while f.ends_with('0') {
            f.pop();
        }
        out.push('.');
        out.push_str(&f);
    }
    out
}

/// Datatype of datetimes in named zones (not valid `xsd:dateTime` lexical forms).
pub const ZONED_DATETIME: &str = "urn:oxilite:cypher:zonedDateTime";

impl T {
    pub fn kind(&self) -> TemporalKind {
        match self {
            Self::Date(_) => TemporalKind::Date,
            Self::LocalTime(_) => TemporalKind::LocalTime,
            Self::Time(..) => TemporalKind::Time,
            Self::LocalDateTime(..) => TemporalKind::LocalDateTime,
            Self::DateTime(..) => TemporalKind::DateTime,
            Self::Duration(_) => TemporalKind::Duration,
        }
    }

    pub fn render(&self) -> String {
        match self {
            Self::Date(d) => fmt_date(*d),
            Self::LocalTime(n) => fmt_time(*n),
            Self::Time(n, o) => format!("{}{}", fmt_time(*n), fmt_offset(*o)),
            Self::LocalDateTime(d, n) => format!("{}T{}", fmt_date(*d), fmt_time(*n)),
            Self::DateTime(d, n, z, o) => {
                let mut s = format!("{}T{}{}", fmt_date(*d), fmt_time(*n), fmt_offset(*o));
                if let Zone::Named(name) = z {
                    s.push_str(&format!("[{name}]"));
                }
                s
            }
            Self::Duration(d) => d.render(),
        }
    }

    pub fn value(self) -> Value {
        Value::Temporal(self.kind(), self.render())
    }

    /// The RDF literal of the value (`xsd:date`, `xsd:time`, `xsd:dateTime`,
    /// `xsd:duration`; datetimes in named zones get their own datatype).
    pub fn lexical(&self) -> (String, &'static str) {
        match self {
            Self::Date(d) => (fmt_date(*d), "http://www.w3.org/2001/XMLSchema#date"),
            Self::LocalTime(n) => (xsd_time(*n), "http://www.w3.org/2001/XMLSchema#time"),
            Self::Time(n, o) => (
                format!("{}{}", xsd_time(*n), fmt_offset(*o)),
                "http://www.w3.org/2001/XMLSchema#time",
            ),
            Self::LocalDateTime(d, n) => (
                format!("{}T{}", fmt_date(*d), xsd_time(*n)),
                "http://www.w3.org/2001/XMLSchema#dateTime",
            ),
            Self::DateTime(d, n, Zone::Offset(_), o) => (
                format!("{}T{}{}", fmt_date(*d), xsd_time(*n), fmt_offset(*o)),
                "http://www.w3.org/2001/XMLSchema#dateTime",
            ),
            Self::DateTime(..) => (self.render(), ZONED_DATETIME),
            Self::Duration(d) => (d.render(), "http://www.w3.org/2001/XMLSchema#duration"),
        }
    }

    fn date_days(&self) -> Option<i64> {
        match self {
            Self::Date(d) | Self::LocalDateTime(d, _) | Self::DateTime(d, ..) => Some(*d),
            _ => None,
        }
    }

    fn time_nanos(&self) -> Option<i64> {
        match self {
            Self::LocalTime(n)
            | Self::Time(n, _)
            | Self::LocalDateTime(_, n)
            | Self::DateTime(_, n, ..) => Some(*n),
            _ => None,
        }
    }

    fn offset(&self) -> Option<i32> {
        match self {
            Self::Time(_, o) | Self::DateTime(_, _, _, o) => Some(*o),
            _ => None,
        }
    }

    fn zone(&self) -> Option<Zone> {
        match self {
            Self::Time(_, o) => Some(Zone::Offset(*o)),
            Self::DateTime(_, _, z, _) => Some(z.clone()),
            _ => None,
        }
    }

    /// Epoch nanoseconds of an instant (datetime), or of the UTC time of day (time).
    fn instant(&self) -> Option<i128> {
        match self {
            Self::DateTime(d, n, _, o) => {
                Some(*d as i128 * DAY_NS as i128 + *n as i128 - *o as i128 * NS as i128)
            }
            _ => None,
        }
    }

    // ----- parsing -----

    pub fn parse(kind: TemporalKind, s: &str) -> Result<Self> {
        let s = s.trim();
        match kind {
            TemporalKind::Date => Ok(Self::Date(parse_date(s)?)),
            TemporalKind::LocalTime => {
                let (n, z) = parse_time_zone(s)?;
                if z.is_some() {
                    // A time with an offset projected to a local time drops the offset.
                }
                Ok(Self::LocalTime(n))
            }
            TemporalKind::Time => {
                let (n, z) = parse_time_zone(s)?;
                let o = match z {
                    Some(Zone::Offset(o)) => o,
                    Some(Zone::Named(name)) => offset_at_local(&name, 0, n)?,
                    None => 0,
                };
                Ok(Self::Time(n, o))
            }
            TemporalKind::LocalDateTime => {
                let (d, n, _) = parse_datetime(s)?;
                Ok(Self::LocalDateTime(d, n))
            }
            TemporalKind::DateTime => {
                let (d, n, z) = parse_datetime(s)?;
                Self::zoned(d, n, z.unwrap_or(Zone::Offset(0)), None)
            }
            TemporalKind::Duration => Ok(Self::Duration(parse_duration(s)?)),
        }
    }

    /// A datetime at a local date-time in a zone (resolving the zone's offset).
    fn zoned(days: i64, nanos: i64, zone: Zone, given_offset: Option<i32>) -> Result<Self> {
        let o = match (&zone, given_offset) {
            (Zone::Offset(o), _) => *o,
            (Zone::Named(name), Some(given)) => {
                let o = offset_at_local(name, days, nanos)?;
                // An explicit offset that disagrees with the zone keeps the instant.
                if o != given {
                    let inst =
                        days as i128 * DAY_NS as i128 + nanos as i128 - given as i128 * NS as i128;
                    return Self::at_instant(inst, zone);
                }
                o
            }
            (Zone::Named(name), None) => offset_at_local(name, days, nanos)?,
        };
        Ok(Self::DateTime(days, nanos, zone, o))
    }

    /// A datetime at an instant (epoch nanoseconds) in a zone.
    fn at_instant(inst: i128, zone: Zone) -> Result<Self> {
        let o = match &zone {
            Zone::Offset(o) => *o,
            Zone::Named(name) => offset_at_instant(name, inst.div_euclid(NS as i128) as i64)?,
        };
        let local = inst + o as i128 * NS as i128;
        let d = local.div_euclid(DAY_NS as i128) as i64;
        let n = local.rem_euclid(DAY_NS as i128) as i64;
        Ok(Self::DateTime(d, n, zone, o))
    }

    pub fn from_value(v: &Value) -> Result<Option<Self>> {
        match v {
            Value::Temporal(k, s) => Ok(Some(Self::parse(*k, s)?)),
            _ => Ok(None),
        }
    }
}

fn digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn num(s: &str) -> Result<i64> {
    s.parse::<i64>()
        .map_err(|_| rt(format!("invalid number '{s}' in a temporal string")))
}

fn check_date(y: i64, m: i64, d: i64) -> Result<i64> {
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return Err(rt(format!("invalid date {y}-{m}-{d}")));
    }
    Ok(days_from_civil(y, m, d))
}

fn from_week(wy: i64, w: i64, dow: i64) -> Result<i64> {
    if w < 1 || w > weeks_in_year(wy) || !(1..=7).contains(&dow) {
        return Err(rt(format!("invalid week date {wy}-W{w}-{dow}")));
    }
    Ok(week1_monday(wy) + (w - 1) * 7 + (dow - 1))
}

fn from_ordinal(y: i64, od: i64) -> Result<i64> {
    let len = if is_leap(y) { 366 } else { 365 };
    if od < 1 || od > len {
        return Err(rt(format!("invalid ordinal day {y}-{od}")));
    }
    Ok(days_from_civil(y, 1, 1) + od - 1)
}

fn from_quarter(y: i64, q: i64, dq: i64) -> Result<i64> {
    if !(1..=4).contains(&q) {
        return Err(rt(format!("invalid quarter {q}")));
    }
    let start = quarter_start(y, q);
    let len = quarter_start(y + (q / 4), (q % 4) + 1) - start;
    if dq < 1 || dq > len {
        return Err(rt(format!("invalid day of quarter {dq}")));
    }
    Ok(start + dq - 1)
}

/// ISO 8601 dates: `2015-07-21`, `20150721`, `2015-07`, `201507`, `2015-W30-2`,
/// `2015W302`, `2015-W30`, `2015-202`, `2015202`, `2015`, with an optional sign and more
/// year digits (`+999999999-12-31`).
fn parse_date(s: &str) -> Result<i64> {
    let bad = || rt(format!("invalid date '{s}'"));
    let (sign, body) = match s.as_bytes().first() {
        Some(b'+') => (1, &s[1..]),
        Some(b'-') => (-1, &s[1..]),
        _ => (1, s),
    };
    // Year: 4 digits, or more when signed.
    let year_len = if sign == -1 || s.starts_with('+') {
        body.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(body.len())
    } else {
        4.min(body.len())
    };
    if year_len < 4 && !(year_len == body.len() && year_len > 0) {
        return Err(bad());
    }
    let y = sign * num(&body[..year_len])?;
    let rest = &body[year_len..];
    if rest.is_empty() {
        return check_date(y, 1, 1);
    }
    let dashed = rest.starts_with('-');
    let r = if dashed { &rest[1..] } else { rest };
    if let Some(w) = r.strip_prefix('W') {
        let (wk, dow) = if dashed {
            match w.split_once('-') {
                Some((a, b)) => (a, Some(b)),
                None => (w, None),
            }
        } else if w.len() == 3 {
            (&w[..2], Some(&w[2..]))
        } else {
            (w, None)
        };
        if !digits(wk) || wk.len() != 2 || dow.is_some_and(|d| !digits(d) || d.len() != 1) {
            return Err(bad());
        }
        return from_week(y, num(wk)?, dow.map_or(Ok(1), num)?);
    }
    if dashed {
        match r.split_once('-') {
            Some((m, d)) if m.len() == 2 && d.len() == 2 && digits(m) && digits(d) => {
                check_date(y, num(m)?, num(d)?)
            }
            None if r.len() == 2 && digits(r) => check_date(y, num(r)?, 1),
            None if r.len() == 3 && digits(r) => from_ordinal(y, num(r)?),
            _ => Err(bad()),
        }
    } else {
        match r.len() {
            4 if digits(r) => check_date(y, num(&r[..2])?, num(&r[2..])?),
            2 if digits(r) => check_date(y, num(r)?, 1),
            3 if digits(r) => from_ordinal(y, num(r)?),
            _ => Err(bad()),
        }
    }
}

/// `21:40:32.142`, `214032.142`, `21:40`, `2140`, `21`, with an optional zone
/// (`Z`, `+01:00`, `+0100`, `+01`, `[Europe/Stockholm]`).
fn parse_time_zone(s: &str) -> Result<(i64, Option<Zone>)> {
    let (s, named) = match s.find('[') {
        Some(i) if s.ends_with(']') => (&s[..i], Some(s[i + 1..s.len() - 1].to_string())),
        _ => (s, None),
    };
    let zpos = s.find(['Z', 'z', '+', '-']);
    let (t, off) = match zpos {
        Some(i) => (&s[..i], Some(&s[i..])),
        None => (s, None),
    };
    let n = parse_time(t)?;
    let zone = match (off, named) {
        (_, Some(name)) => {
            tz(&name)?;
            Some(Zone::Named(name))
        }
        (Some(o), None) => Some(Zone::Offset(parse_offset(if o == "z" { "Z" } else { o })?)),
        (None, None) => None,
    };
    Ok((n, zone))
}

fn parse_time(t: &str) -> Result<i64> {
    let bad = || rt(format!("invalid time '{t}'"));
    let (main, frac) = match t.find(['.', ',']) {
        Some(i) => (&t[..i], Some(&t[i + 1..])),
        None => (t, None),
    };
    let parts: Vec<&str> = if main.contains(':') {
        main.split(':').collect()
    } else {
        let mut v = Vec::new();
        let mut i = 0;
        while i < main.len() {
            v.push(&main[i..(i + 2).min(main.len())]);
            i += 2;
        }
        v
    };
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|p| p.len() != 2 || !digits(p)) {
        return Err(bad());
    }
    let h = num(parts[0])?;
    let m = parts.get(1).map_or(Ok(0), |x| num(x))?;
    let sec = parts.get(2).map_or(Ok(0), |x| num(x))?;
    if h > 23 || m > 59 || sec > 59 {
        return Err(bad());
    }
    let mut nanos = 0;
    if let Some(f) = frac {
        if parts.len() != 3 || !digits(f) || f.len() > 9 {
            return Err(bad());
        }
        nanos = num(&format!("{f:0<9}"))?;
    }
    Ok(((h * 60 + m) * 60 + sec) * NS + nanos)
}

fn parse_datetime(s: &str) -> Result<(i64, i64, Option<Zone>)> {
    match s.find(['T', 't']) {
        Some(i) => {
            let d = parse_date(&s[..i])?;
            let (n, z) = parse_time_zone(&s[i + 1..])?;
            Ok((d, n, z))
        }
        None => Ok((parse_date(s)?, 0, None)),
    }
}

/// `P1Y2M3W4DT5H6M7.8S` (fractions allowed on any unit) or `P2012-02-02T14:37:21.545`.
fn parse_duration(s: &str) -> Result<Dur> {
    let bad = || rt(format!("invalid duration '{s}'"));
    let (neg, body) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s),
    };
    let body = body.strip_prefix(['P', 'p']).ok_or_else(bad)?;
    // Alternative form: a date-time.
    if body.contains('-') && !body.contains(['Y', 'W', 'D', 'H', 'S'])
        || body.starts_with(|c: char| c.is_ascii_digit())
            && body.len() >= 4
            && body[..4].bytes().all(|b| b.is_ascii_digit())
            && body.get(4..5) == Some("-")
    {
        let (date, time) = match body.split_once('T') {
            Some((d, t)) => (d, Some(t)),
            None => (body, None),
        };
        let p: Vec<&str> = date.split('-').collect();
        if p.len() != 3 {
            return Err(bad());
        }
        let (y, m, d) = (num(p[0])?, num(p[1])?, num(p[2])?);
        let n = time.map_or(Ok(0), parse_time)?;
        let dur = Dur::new(y * 12 + m, d, n as i128);
        return Ok(if neg { dur.neg() } else { dur });
    }
    let (date_part, time_part) = match body.split_once(['T', 't']) {
        Some((d, t)) => (d, t),
        None => (body, ""),
    };
    let mut months = 0f64;
    let mut days = 0f64;
    let mut nanos = 0f64;
    let mut exact = Some((0i64, 0i64, 0i128));
    let mut take = |part: &str, date: bool| -> Result<()> {
        let mut cur = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() || c == '.' || c == ',' || c == '-' || c == '+' {
                cur.push(if c == ',' { '.' } else { c });
                continue;
            }
            if cur.is_empty() {
                return Err(bad());
            }
            let v: f64 = cur.parse().map_err(|_| bad())?;
            let int = if cur.contains('.') {
                None
            } else {
                cur.parse::<i64>().ok()
            };
            let unit = c.to_ascii_uppercase();
            match (date, unit) {
                (true, 'Y') => {
                    months += v * 12.0;
                    exact = exact.and_then(|(m, d, n)| int.map(|i| (m + i * 12, d, n)));
                }
                (true, 'M') => {
                    months += v;
                    exact = exact.and_then(|(m, d, n)| int.map(|i| (m + i, d, n)));
                }
                (true, 'W') => {
                    days += v * 7.0;
                    exact = exact.and_then(|(m, d, n)| int.map(|i| (m, d + i * 7, n)));
                }
                (true, 'D') => {
                    days += v;
                    exact = exact.and_then(|(m, d, n)| int.map(|i| (m, d + i, n)));
                }
                (false, 'H') => {
                    nanos += v * 3600.0 * NS as f64;
                    exact = exact.and_then(|(m, d, n)| {
                        int.map(|i| (m, d, n + i as i128 * 3600 * NS as i128))
                    });
                }
                (false, 'M') => {
                    nanos += v * 60.0 * NS as f64;
                    exact = exact
                        .and_then(|(m, d, n)| int.map(|i| (m, d, n + i as i128 * 60 * NS as i128)));
                }
                (false, 'S') => {
                    nanos += v * NS as f64;
                    // Seconds with a fraction stay exact to the nanosecond.
                    let secs = decimal_nanos(&cur);
                    exact = exact.and_then(|(m, d, n)| secs.map(|x| (m, d, n + x)));
                }
                _ => return Err(bad()),
            }
            cur.clear();
        }
        if !cur.is_empty() {
            return Err(bad());
        }
        Ok(())
    };
    take(date_part, true)?;
    take(time_part, false)?;
    // Fractions below seconds only in seconds; other fractions cascade.
    let exact = exact.filter(|_| months.fract() == 0.0 && days.fract() == 0.0);
    let dur = Dur::from_parts(months, days, nanos, exact);
    Ok(if neg { dur.neg() } else { dur })
}

fn decimal_nanos(s: &str) -> Option<i128> {
    let (neg, s) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (i, f) = s.split_once('.').unwrap_or((s, ""));
    if f.len() > 9 {
        return None;
    }
    let v = i.parse::<i128>().ok()? * NS as i128 + format!("{f:0<9}").parse::<i128>().ok()?;
    Some(if neg { -v } else { v })
}

// ----- construction from maps -----

fn map_int(m: &BTreeMap<String, Value>, k: &str) -> Result<Option<i64>> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Int(i)) => Ok(Some(*i)),
        Some(Value::Float(f)) if f.fract() == 0.0 => Ok(Some(*f as i64)),
        Some(o) => Err(rt(format!(
            "`{k}` must be an integer, got {}",
            o.type_name()
        ))),
    }
}

fn map_temporal(m: &BTreeMap<String, Value>, k: &str) -> Result<Option<T>> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => T::from_value(v)?
            .map(Some)
            .ok_or_else(|| rt(format!("`{k}` must be a temporal value"))),
    }
}

const DATE_KEYS: &[&str] = &[
    "year",
    "month",
    "day",
    "week",
    "dayofweek",
    "ordinalday",
    "quarter",
    "dayofquarter",
];
const TIME_KEYS: &[&str] = &[
    "hour",
    "minute",
    "second",
    "millisecond",
    "microsecond",
    "nanosecond",
];

fn lower_keys(m: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    m.iter()
        .map(|(k, v)| (k.to_lowercase(), v.clone()))
        .collect()
}

/// The date of a map, over an optional base date.
fn date_from_map(m: &BTreeMap<String, Value>, base: Option<i64>) -> Result<Option<i64>> {
    let g = |k| map_int(m, k);
    let (year, month, day) = (g("year")?, g("month")?, g("day")?);
    let (week, dow) = (g("week")?, g("dayofweek")?);
    let (od, q, dq) = (g("ordinalday")?, g("quarter")?, g("dayofquarter")?);
    let any = DATE_KEYS.iter().any(|k| m.contains_key(*k));
    let Some(base) = base else {
        if !any {
            return Ok(None);
        }
        let y = year.ok_or_else(|| rt("a date needs a year"))?;
        if let Some(w) = week {
            return from_week(y, w, dow.unwrap_or(1)).map(Some);
        }
        if let Some(o) = od {
            return from_ordinal(y, o).map(Some);
        }
        if let Some(q) = q {
            return from_quarter(y, q, dq.unwrap_or(1)).map(Some);
        }
        if day.is_some() && month.is_none() {
            return Err(rt("a date with a day needs a month"));
        }
        return check_date(y, month.unwrap_or(1), day.unwrap_or(1)).map(Some);
    };
    let (by, bm, bd) = civil_from_days(base);
    if let Some(w) = week {
        let (bwy, _) = iso_week(base);
        return from_week(year.unwrap_or(bwy), w, dow.unwrap_or(weekday(base))).map(Some);
    }
    if let Some(d) = dow {
        let (bwy, bw) = iso_week(base);
        return from_week(year.unwrap_or(bwy), bw, d).map(Some);
    }
    if let Some(o) = od {
        return from_ordinal(year.unwrap_or(by), o).map(Some);
    }
    if q.is_some() || dq.is_some() {
        let bq = (bm - 1) / 3 + 1;
        let y = year.unwrap_or(by);
        let q = q.unwrap_or(bq);
        let dq = dq.unwrap_or(base - quarter_start(by, bq) + 1);
        return from_quarter(y, q, dq).map(Some);
    }
    check_date(year.unwrap_or(by), month.unwrap_or(bm), day.unwrap_or(bd)).map(Some)
}

/// The time of day of a map, over an optional base time.
fn time_from_map(m: &BTreeMap<String, Value>, base: Option<i64>) -> Result<Option<i64>> {
    let g = |k| map_int(m, k);
    let (h, mi, s) = (g("hour")?, g("minute")?, g("second")?);
    let (ms, us, ns) = (g("millisecond")?, g("microsecond")?, g("nanosecond")?);
    let any = TIME_KEYS.iter().any(|k| m.contains_key(*k));
    if base.is_none() && !any {
        return Ok(None);
    }
    let (bh, bmi, bs, bn) = split_time(base.unwrap_or(0));
    let sub = if ms.is_some() || us.is_some() || ns.is_some() {
        // Each sub-second field is bounded by the next larger one given.
        let us_limit = if ms.is_some() { 1000 } else { 1_000_000 };
        let ns_limit = if us.is_some() {
            1000
        } else if ms.is_some() {
            1_000_000
        } else {
            NS
        };
        let given = (ms, us, ns);
        // Over a base value, the sub-second units larger than the ones given are kept.
        let (ms, us, ns, us_limit, ns_limit) = match (base, given) {
            (Some(_), (None, None, Some(n))) if n < 1000 => {
                (bn / 1_000_000, (bn / 1000) % 1000, n, 1000, 1000)
            }
            (Some(_), (None, Some(u), n)) if u < 1000 => {
                (bn / 1_000_000, u, n.unwrap_or(0), 1000, 1000)
            }
            _ => (
                ms.unwrap_or(0),
                us.unwrap_or(0),
                ns.unwrap_or(0),
                us_limit,
                ns_limit,
            ),
        };
        if !(0..1000).contains(&ms) || !(0..us_limit).contains(&us) || !(0..ns_limit).contains(&ns)
        {
            return Err(rt("sub-second fields out of range"));
        }
        ms * 1_000_000 + us * 1000 + ns
    } else if base.is_some() {
        bn
    } else {
        0
    };
    let hh = h.unwrap_or(if base.is_some() { bh } else { 0 });
    let mm = mi.unwrap_or(if base.is_some() { bmi } else { 0 });
    let ss = s.unwrap_or(if base.is_some() { bs } else { 0 });
    if base.is_none() && ((mi.is_some() && h.is_none()) || (s.is_some() && mi.is_none())) {
        return Err(rt("time fields need their larger units"));
    }
    if !(0..24).contains(&hh) || !(0..60).contains(&mm) || !(0..60).contains(&ss) {
        return Err(rt("time fields out of range"));
    }
    Ok(Some(((hh * 60 + mm) * 60 + ss) * NS + sub))
}

fn map_zone(m: &BTreeMap<String, Value>) -> Result<Option<Zone>> {
    match m.get("timezone") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => parse_zone(s).map(Some),
        Some(o) => Err(rt(format!(
            "timezone must be a string, got {}",
            o.type_name()
        ))),
    }
}

/// Builds a temporal value of `kind` from a map (`date({year: 1984, week: 10})`,
/// `datetime({date: d, time: t, timezone: 'Europe/Stockholm'})`…).
pub(crate) fn from_map(kind: TemporalKind, m: &BTreeMap<String, Value>) -> Result<Value> {
    let m = lower_keys(m);
    if kind == TemporalKind::Duration {
        return duration_from_map(&m).map(|d| T::Duration(d).value());
    }
    let base_dt = map_temporal(&m, "datetime")?;
    let base_date = map_temporal(&m, "date")?.or_else(|| base_dt.clone());
    let base_time = map_temporal(&m, "time")?.or_else(|| base_dt.clone());
    let zone = map_zone(&m)?;
    let bd = base_date.as_ref().and_then(T::date_days);
    let bt = base_time.as_ref().and_then(T::time_nanos);
    // The zone of the result comes from the time component (or the whole datetime).
    let zone_src = if m.contains_key("time") {
        base_time.as_ref()
    } else {
        base_dt.as_ref()
    };
    let src_zone = zone_src.and_then(T::zone);
    Ok(match kind {
        TemporalKind::Date => {
            let d = date_from_map(&m, bd)?.ok_or_else(|| rt("date() needs date fields"))?;
            T::Date(d).value()
        }
        TemporalKind::LocalTime => {
            let n = time_from_map(&m, bt)?.ok_or_else(|| rt("localtime() needs time fields"))?;
            T::LocalTime(n).value()
        }
        TemporalKind::Time => {
            let n = time_from_map(&m, bt)?.ok_or_else(|| rt("time() needs time fields"))?;
            let src = base_time.as_ref().and_then(T::zone);
            let (n, o) = match (zone, src) {
                // A new zone on a zoned time keeps the instant.
                (Some(z), Some(sz)) => {
                    let so = zone_offset_now(&sz, bd.unwrap_or(0), n)?;
                    let no = zone_offset_now(&z, bd.unwrap_or(0), n)?;
                    let shifted = (n as i128 - so as i128 * NS as i128 + no as i128 * NS as i128)
                        .rem_euclid(DAY_NS as i128) as i64;
                    (shifted, no)
                }
                (Some(z), None) => (n, zone_offset_now(&z, bd.unwrap_or(0), n)?),
                (None, Some(sz)) => (n, zone_offset_now(&sz, bd.unwrap_or(0), n)?),
                (None, None) => (n, 0),
            };
            T::Time(n, o).value()
        }
        TemporalKind::LocalDateTime => {
            let d = date_from_map(&m, bd)?.ok_or_else(|| rt("localdatetime() needs a date"))?;
            let n = time_from_map(&m, bt)?.unwrap_or(0);
            T::LocalDateTime(d, n).value()
        }
        TemporalKind::DateTime => {
            let d = date_from_map(&m, bd)?.ok_or_else(|| rt("datetime() needs a date"))?;
            let n = time_from_map(&m, bt)?.unwrap_or(0);
            match (zone, src_zone) {
                (Some(z), Some(sz)) => {
                    // Converting a zoned value to another zone keeps the instant (of the local
                    // date-time after the overrides, in the source zone).
                    let so = zone_offset_now(&sz, d, n)?;
                    let inst = d as i128 * DAY_NS as i128 + n as i128 - so as i128 * NS as i128;
                    T::at_instant(inst, z)?.value()
                }
                (Some(z), _) => T::zoned(d, n, z, None)?.value(),
                (None, Some(sz)) => match sz {
                    Zone::Named(_) => T::zoned(d, n, sz, None)?.value(),
                    Zone::Offset(o) => T::DateTime(d, n, Zone::Offset(o), o).value(),
                },
                (None, None) => T::DateTime(d, n, Zone::Offset(0), 0).value(),
            }
        }
        TemporalKind::Duration => unreachable!("handled above"),
    })
}

fn zone_offset_now(z: &Zone, days: i64, nanos: i64) -> Result<i32> {
    match z {
        Zone::Offset(o) => Ok(*o),
        Zone::Named(n) => offset_at_local(n, days, nanos),
    }
}

fn duration_from_map(m: &BTreeMap<String, Value>) -> Result<Dur> {
    let mut months = 0f64;
    let mut days = 0f64;
    let mut nanos = 0f64;
    let mut exact = Some((0i64, 0i64, 0i128));
    for (k, v) in m {
        let (f, int) = match v {
            Value::Int(i) => (*i as f64, Some(*i)),
            Value::Float(x) => (*x, None),
            Value::Null => continue,
            o => {
                return Err(rt(format!(
                    "duration field `{k}` must be a number, got {}",
                    o.type_name()
                )))
            }
        };
        let add = |e: Option<(i64, i64, i128)>, dm: i64, dd: i64, dn: i128| {
            e.and_then(|(m, d, n)| int.map(|i| (m + i * dm, d + i * dd, n + i as i128 * dn)))
        };
        match k.as_str() {
            "years" => {
                months += f * 12.0;
                exact = add(exact, 12, 0, 0);
            }
            "quarters" => {
                months += f * 3.0;
                exact = add(exact, 3, 0, 0);
            }
            "months" => {
                months += f;
                exact = add(exact, 1, 0, 0);
            }
            "weeks" => {
                days += f * 7.0;
                exact = add(exact, 0, 7, 0);
            }
            "days" => {
                days += f;
                exact = add(exact, 0, 1, 0);
            }
            "hours" => {
                nanos += f * 3600.0 * NS as f64;
                exact = add(exact, 0, 0, 3600 * NS as i128);
            }
            "minutes" => {
                nanos += f * 60.0 * NS as f64;
                exact = add(exact, 0, 0, 60 * NS as i128);
            }
            "seconds" => {
                nanos += f * NS as f64;
                exact = add(exact, 0, 0, NS as i128);
            }
            "milliseconds" => {
                nanos += f * 1e6;
                exact = add(exact, 0, 0, 1_000_000);
            }
            "microseconds" => {
                nanos += f * 1e3;
                exact = add(exact, 0, 0, 1000);
            }
            "nanoseconds" => {
                nanos += f;
                exact = add(exact, 0, 0, 1);
            }
            other => return Err(rt(format!("unknown duration field `{other}`"))),
        }
    }
    Ok(Dur::from_parts(months, days, nanos, exact))
}

// ----- functions -----

/// `date(x)`, `datetime(x)`… from a string, a map or another temporal value.
pub(crate) fn construct(kind: TemporalKind, arg: Option<&Value>) -> Result<Value> {
    match arg {
        None => now(kind),
        Some(Value::Null) => Ok(Value::Null),
        Some(Value::String(s)) => Ok(T::parse(kind, s)?.value()),
        Some(Value::Map(m)) => from_map(kind, m),
        Some(v @ Value::Temporal(..)) => {
            if kind == TemporalKind::Duration {
                return Err(rt("duration() of a temporal value"));
            }
            // Projection: keep the components the target type has.
            let key = match v {
                Value::Temporal(TemporalKind::Date, _) => "date",
                Value::Temporal(TemporalKind::LocalTime | TemporalKind::Time, _) => "time",
                _ => "datetime",
            };
            from_map(kind, &BTreeMap::from([(key.to_string(), v.clone())]))
        }
        Some(o) => Err(rt(format!(
            "cannot build a temporal value from a {}",
            o.type_name()
        ))),
    }
}

thread_local! {
    /// The statement clock: every `date()`, `time()`… of one statement sees the same instant.
    static CLOCK: std::cell::Cell<Option<i128>> = const { std::cell::Cell::new(None) };
}

/// Starts a statement: the next clock read fixes its instant.
pub(crate) fn reset_clock() {
    CLOCK.with(|c| c.set(None));
}

fn now(kind: TemporalKind) -> Result<Value> {
    #[cfg(not(all(target_family = "wasm", target_os = "unknown")))]
    {
        let inst = match CLOCK.with(std::cell::Cell::get) {
            Some(i) => i,
            None => {
                let d = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| rt(e.to_string()))?;
                let i = d.as_nanos() as i128;
                CLOCK.with(|c| c.set(Some(i)));
                i
            }
        };
        let t = T::at_instant(inst, Zone::Offset(0))?;
        let T::DateTime(days, nanos, _, _) = t.clone() else {
            unreachable!()
        };
        return Ok(match kind {
            TemporalKind::Date => T::Date(days),
            TemporalKind::LocalTime => T::LocalTime(nanos),
            TemporalKind::Time => T::Time(nanos, 0),
            TemporalKind::LocalDateTime => T::LocalDateTime(days, nanos),
            TemporalKind::DateTime => t,
            TemporalKind::Duration => return Err(rt("duration() needs an argument")),
        }
        .value());
    }
    #[allow(unreachable_code)]
    Err(CypherError::unsupported(format!(
        "{kind:?}() without arguments (no clock on this platform)"
    )))
}

/// Component access (`d.year`, `t.timezone`, `dur.days`…).
pub(crate) fn field(v: &T, key: &str) -> Result<Value> {
    let key = key.to_lowercase();
    if let T::Duration(d) = v {
        return Ok(d.component(&key).unwrap_or(Value::Null));
    }
    if let Some(days) = v.date_days() {
        let (y, m, d) = civil_from_days(days);
        let (wy, w) = iso_week(days);
        let q = (m - 1) / 3 + 1;
        let r = match key.as_str() {
            "year" => Some(y),
            "quarter" => Some(q),
            "month" => Some(m),
            "week" => Some(w),
            "weekyear" => Some(wy),
            "day" => Some(d),
            "ordinalday" => Some(ordinal(days)),
            "weekday" | "dayofweek" => Some(weekday(days)),
            "dayofquarter" => Some(days - quarter_start(y, q) + 1),
            _ => None,
        };
        if let Some(r) = r {
            return Ok(Value::Int(r));
        }
    }
    if let Some(n) = v.time_nanos() {
        let (h, mi, s, ns) = split_time(n);
        let r = match key.as_str() {
            "hour" => Some(h),
            "minute" => Some(mi),
            "second" => Some(s),
            "millisecond" => Some(ns / 1_000_000),
            "microsecond" => Some(ns / 1000),
            "nanosecond" => Some(ns),
            _ => None,
        };
        if let Some(r) = r {
            return Ok(Value::Int(r));
        }
    }
    if let Some(o) = v.offset() {
        match key.as_str() {
            "timezone" => {
                return Ok(Value::String(match v.zone() {
                    Some(Zone::Named(n)) => n,
                    _ => fmt_offset(o),
                }))
            }
            "offset" => return Ok(Value::String(fmt_offset(o))),
            "offsetminutes" => return Ok(Value::Int(o as i64 / 60)),
            "offsetseconds" => return Ok(Value::Int(o as i64)),
            _ => {}
        }
    }
    if let Some(inst) = v.instant() {
        match key.as_str() {
            "epochseconds" => return Ok(Value::Int(inst.div_euclid(NS as i128) as i64)),
            "epochmillis" => return Ok(Value::Int(inst.div_euclid(1_000_000) as i64)),
            _ => {}
        }
    }
    Ok(Value::Null)
}

/// `x.truncate(unit, value, map)`.
pub(crate) fn truncate(
    kind: TemporalKind,
    unit: &str,
    v: &T,
    overrides: Option<&BTreeMap<String, Value>>,
) -> Result<Value> {
    let unit = unit.to_lowercase();
    let days = v.date_days();
    let nanos = v.time_nanos().unwrap_or(0);
    let tdays = |days: i64| -> Result<i64> {
        let (y, m, _) = civil_from_days(days);
        Ok(match unit.as_str() {
            "millennium" => days_from_civil(y.div_euclid(1000) * 1000, 1, 1),
            "century" => days_from_civil(y.div_euclid(100) * 100, 1, 1),
            "decade" => days_from_civil(y.div_euclid(10) * 10, 1, 1),
            "year" => days_from_civil(y, 1, 1),
            "weekyear" => week1_monday(iso_week(days).0),
            "quarter" => quarter_start(y, (m - 1) / 3 + 1),
            "month" => days_from_civil(y, m, 1),
            "week" => days - (weekday(days) - 1),
            _ => days,
        })
    };
    let date_unit = matches!(
        unit.as_str(),
        "millennium"
            | "century"
            | "decade"
            | "year"
            | "weekyear"
            | "quarter"
            | "month"
            | "week"
            | "day"
    );
    let tn = match unit.as_str() {
        "hour" => nanos / (3600 * NS) * 3600 * NS,
        "minute" => nanos / (60 * NS) * 60 * NS,
        "second" => nanos / NS * NS,
        "millisecond" => nanos / 1_000_000 * 1_000_000,
        "microsecond" => nanos / 1000 * 1000,
        _ if date_unit => 0,
        other => return Err(rt(format!("unknown truncation unit '{other}'"))),
    };
    let td = match days {
        Some(d) => Some(tdays(d)?),
        None if date_unit && unit != "day" => {
            return Err(rt("cannot truncate a time to a date unit"))
        }
        None => None,
    };
    let zone = v.zone();
    let mut base = BTreeMap::new();
    let truncated = match (kind, td) {
        (TemporalKind::Date, Some(d)) => T::Date(d),
        (TemporalKind::LocalDateTime, Some(d)) => T::LocalDateTime(d, tn),
        (TemporalKind::DateTime, Some(d)) => match &zone {
            Some(z) => T::zoned(d, tn, z.clone(), None)?,
            None => T::DateTime(d, tn, Zone::Offset(0), 0),
        },
        (TemporalKind::LocalTime, _) => T::LocalTime(tn),
        (TemporalKind::Time, _) => T::Time(tn, v.offset().unwrap_or(0)),
        _ => return Err(rt("invalid truncation")),
    };
    let Some(o) = overrides.filter(|m| !m.is_empty()) else {
        return Ok(truncated.value());
    };
    let key = match kind {
        TemporalKind::Date => "date",
        TemporalKind::LocalTime | TemporalKind::Time => "time",
        _ => "datetime",
    };
    base.insert(key.to_string(), truncated.value());
    let o = lower_keys(o);
    for (k, v) in o {
        base.insert(k, v);
    }
    // A timezone in the overrides applies to the truncated local value (no conversion).
    if kind == TemporalKind::DateTime && base.contains_key("timezone") {
        let dt = base.remove("datetime").expect("inserted");
        base.insert("date".into(), dt.clone());
        let T::DateTime(_, n, _, _) = T::from_value(&dt)?.expect("temporal") else {
            unreachable!()
        };
        base.insert("time".into(), T::LocalTime(n).value());
    }
    if kind == TemporalKind::Time && base.contains_key("timezone") {
        let t = base.remove("time").expect("inserted");
        let T::Time(n, _) = T::from_value(&t)?.expect("temporal") else {
            unreachable!()
        };
        base.insert("time".into(), T::LocalTime(n).value());
    }
    from_map(kind, &base)
}

// ----- arithmetic, comparison -----

pub(crate) fn add_duration(v: &T, d: &Dur, sign: i64) -> Result<T> {
    let d = if sign < 0 { d.neg() } else { *d };
    let n = d.total_nanos();
    Ok(match v {
        T::Date(days) => {
            let x = add_months(*days, d.months) + d.days;
            // Whole days of the time part apply to dates.
            T::Date(x + (n / DAY_NS as i128) as i64)
        }
        T::LocalTime(t) => T::LocalTime((*t as i128 + n).rem_euclid(DAY_NS as i128) as i64),
        T::Time(t, o) => T::Time((*t as i128 + n).rem_euclid(DAY_NS as i128) as i64, *o),
        T::LocalDateTime(days, t) => {
            let base = add_months(*days, d.months) + d.days;
            let total = base as i128 * DAY_NS as i128 + *t as i128 + n;
            T::LocalDateTime(
                total.div_euclid(DAY_NS as i128) as i64,
                total.rem_euclid(DAY_NS as i128) as i64,
            )
        }
        T::DateTime(days, t, z, _) => {
            let base = add_months(*days, d.months) + d.days;
            let local = T::zoned(base, *t, z.clone(), None)?;
            let inst = local.instant().expect("datetime") + n;
            T::at_instant(inst, z.clone())?
        }
        T::Duration(x) => T::Duration(x.add(d)?),
    })
}

/// `a + b`, `a - b`, `a * n`, `a / n` when a temporal value is involved (`None` when the
/// operation does not apply).
pub(crate) fn arith(op: char, a: &Value, b: &Value) -> Result<Option<Value>> {
    let (ta, tb) = (T::from_value(a)?, T::from_value(b)?);
    Ok(match (op, ta, tb) {
        ('+', Some(x), Some(T::Duration(d))) => Some(add_duration(&x, &d, 1)?.value()),
        ('+', Some(T::Duration(d)), Some(x)) if !matches!(x, T::Duration(_)) => {
            Some(add_duration(&x, &d, 1)?.value())
        }
        ('-', Some(x), Some(T::Duration(d))) => Some(add_duration(&x, &d, -1)?.value()),
        ('*', Some(T::Duration(d)), None) => b.as_f64().map(|f| T::Duration(d.scale(f)).value()),
        ('*', None, Some(T::Duration(d))) => a.as_f64().map(|f| T::Duration(d.scale(f)).value()),
        ('/', Some(T::Duration(d)), None) => match b.as_f64() {
            Some(0.0) => return Err(rt("division of a duration by zero")),
            Some(f) => Some(T::Duration(d.scale(1.0 / f)).value()),
            None => None,
        },
        _ => None,
    })
}

pub(crate) fn compare(a: &T, b: &T) -> Option<Ordering> {
    match (a, b) {
        (T::Date(x), T::Date(y)) => Some(x.cmp(y)),
        (T::LocalTime(x), T::LocalTime(y)) => Some(x.cmp(y)),
        (T::Time(x, ox), T::Time(y, oy)) => {
            let ux = *x as i128 - *ox as i128 * NS as i128;
            let uy = *y as i128 - *oy as i128 * NS as i128;
            Some(ux.cmp(&uy).then(ox.cmp(oy).reverse()))
        }
        (T::LocalDateTime(dx, x), T::LocalDateTime(dy, y)) => Some((dx, x).cmp(&(dy, y))),
        (T::DateTime(..), T::DateTime(..)) => Some(a.instant()?.cmp(&b.instant()?)),
        _ => None,
    }
}

pub(crate) fn equal(a: &T, b: &T) -> Option<bool> {
    match (a, b) {
        (T::Duration(x), T::Duration(y)) => Some(x == y),
        (T::Time(x, ox), T::Time(y, oy)) => Some(x == y && ox == oy),
        (T::DateTime(dx, x, zx, ox), T::DateTime(dy, y, zy, oy)) => {
            Some(a.instant() == b.instant() && (zx == zy || (dx == dy && x == y && ox == oy)))
        }
        _ if a.kind() != b.kind() => Some(false),
        _ => compare(a, b).map(|o| o == Ordering::Equal),
    }
}

// ----- durations between -----

/// `duration.between`, `inMonths`, `inDays`, `inSeconds` (Java `until` semantics: months
/// and days on local date-times, the rest on instants when a zone is involved).
pub(crate) fn between(unit: &str, a: &T, b: &T) -> Result<Value> {
    let zone = match (a, b) {
        (T::DateTime(_, _, z, _), _) | (_, T::DateTime(_, _, z, _)) => Some(z.clone()),
        _ => None,
    };
    let has_date = |v: &T| v.date_days().is_some();
    let dur = if has_date(a) && has_date(b) {
        match &zone {
            Some(z) => {
                // Both in the first zone found: zoned values by instant, local ones as local.
                let local = |v: &T| -> Result<(i64, i64)> {
                    Ok(match v {
                        T::DateTime(..) => {
                            match T::at_instant(v.instant().expect("datetime"), z.clone())? {
                                T::DateTime(d, n, _, _) => (d, n),
                                _ => unreachable!(),
                            }
                        }
                        _ => (v.date_days().expect("date"), v.time_nanos().unwrap_or(0)),
                    })
                };
                let (da, na) = local(a)?;
                let (db, nb) = local(b)?;
                let inst = |d: i64, n: i64| -> Result<i128> {
                    Ok(T::zoned(d, n, z.clone(), None)?
                        .instant()
                        .expect("datetime"))
                };
                let (ia, ib) = (inst(da, na)?, inst(db, nb)?);
                match unit {
                    "inseconds" => Dur::new(0, 0, ib - ia),
                    _ => {
                        let (months, days) = months_days(da, na as i128, db, nb as i128);
                        match unit {
                            "inmonths" => Dur::new(months, 0, 0),
                            "indays" => Dur::new(0, add_months(da, months) - da + days, 0),
                            _ => {
                                let mid = inst(add_months(da, months) + days, na)?;
                                Dur::new(months, days, ib - mid)
                            }
                        }
                    }
                }
            }
            None => {
                let (da, na) = (
                    a.date_days().expect("date"),
                    a.time_nanos().unwrap_or(0) as i128,
                );
                let (db, nb) = (
                    b.date_days().expect("date"),
                    b.time_nanos().unwrap_or(0) as i128,
                );
                let start = da as i128 * DAY_NS as i128 + na;
                let end = db as i128 * DAY_NS as i128 + nb;
                match unit {
                    "inseconds" => Dur::new(0, 0, end - start),
                    "indays" => Dur::new(0, ((end - start) / DAY_NS as i128) as i64, 0),
                    _ => {
                        let (months, days) = months_days(da, na, db, nb);
                        if unit == "inmonths" {
                            Dur::new(months, 0, 0)
                        } else {
                            let mid = (add_months(da, months) + days) as i128 * DAY_NS as i128 + na;
                            Dur::new(months, days, end - mid)
                        }
                    }
                }
            }
        }
    } else if matches!(unit, "inmonths" | "indays") {
        Dur::default()
    } else {
        // At least one side has no date: compare times of day. A zoned datetime and a local
        // time are placed on the same day in the zone; offset times compare in UTC.
        let utc = |v: &T| -> Option<i128> {
            match v {
                T::Time(n, o) => Some(*n as i128 - *o as i128 * NS as i128),
                _ => None,
            }
        };
        match (a, b, &zone) {
            (T::DateTime(d, n, _, o), other, Some(z))
            | (other, T::DateTime(d, n, _, o), Some(z))
                if other.time_nanos().is_some() || other.date_days().is_some() =>
            {
                let inst_dt = *d as i128 * DAY_NS as i128 + *n as i128 - *o as i128 * NS as i128;
                let inst_other = match other {
                    T::Time(tn, to) => {
                        *d as i128 * DAY_NS as i128 + *tn as i128 - *to as i128 * NS as i128
                    }
                    _ => T::zoned(
                        other.date_days().unwrap_or(*d),
                        other.time_nanos().unwrap_or(0),
                        z.clone(),
                        None,
                    )?
                    .instant()
                    .expect("datetime"),
                };
                let (x, y) = if matches!(a, T::DateTime(..)) {
                    (inst_dt, inst_other)
                } else {
                    (inst_other, inst_dt)
                };
                if matches!(other, T::Time(..)) {
                    // Offset times against a datetime: times of day in UTC.
                    let tx = x.rem_euclid(DAY_NS as i128);
                    let ty = y.rem_euclid(DAY_NS as i128);
                    Dur::new(0, 0, ty - tx)
                } else {
                    Dur::new(0, 0, y - x)
                }
            }
            _ => {
                let (x, y) = match (utc(a), utc(b)) {
                    (Some(x), Some(y)) => (x, y),
                    _ => (
                        a.time_nanos().unwrap_or(0) as i128,
                        b.time_nanos().unwrap_or(0) as i128,
                    ),
                };
                Dur::new(0, 0, y - x)
            }
        }
    };
    Ok(T::Duration(dur).value())
}

/// Whole months, then whole days, from `(da, na)` to `(db, nb)` (Java's `until`).
fn months_days(da: i64, na: i128, db: i64, nb: i128) -> (i64, i64) {
    let (y1, m1, d1) = civil_from_days(da);
    let (y2, m2, d2) = civil_from_days(db);
    let mut months = (y2 * 12 + m2) - (y1 * 12 + m1);
    if months > 0 && (d2, nb) < (d1, na) {
        months -= 1;
    } else if months < 0 && (d2, nb) > (d1, na) {
        months += 1;
    }
    let mid = add_months(da, months) as i128 * DAY_NS as i128 + na;
    let end = db as i128 * DAY_NS as i128 + nb;
    let days = ((end - mid) / DAY_NS as i128) as i64;
    (months, days)
}

pub(crate) fn from_epoch(seconds: i64, nanos: i64) -> Result<Value> {
    Ok(T::at_instant(
        seconds as i128 * NS as i128 + nanos as i128,
        Zone::Offset(0),
    )?
    .value())
}

/// Decodes a stored literal of a temporal datatype.
pub(crate) fn from_literal(dt: &str, lex: &str) -> Option<Value> {
    let kind = match dt {
        "http://www.w3.org/2001/XMLSchema#date" => TemporalKind::Date,
        "http://www.w3.org/2001/XMLSchema#time" => {
            if parse_time_zone(lex).ok()?.1.is_some() {
                TemporalKind::Time
            } else {
                TemporalKind::LocalTime
            }
        }
        "http://www.w3.org/2001/XMLSchema#dateTime"
        | "http://www.w3.org/2001/XMLSchema#dateTimeStamp" => {
            if parse_datetime(lex).ok()?.2.is_some() {
                TemporalKind::DateTime
            } else {
                TemporalKind::LocalDateTime
            }
        }
        ZONED_DATETIME => TemporalKind::DateTime,
        "http://www.w3.org/2001/XMLSchema#duration"
        | "http://www.w3.org/2001/XMLSchema#dayTimeDuration"
        | "http://www.w3.org/2001/XMLSchema#yearMonthDuration" => TemporalKind::Duration,
        _ => return None,
    };
    T::parse(kind, lex).ok().map(T::value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(k: TemporalKind, s: &str) -> String {
        T::parse(k, s).unwrap().render()
    }

    #[test]
    fn calendar() {
        for d in [-1_000_000i64, -1, 0, 1, 18_000, 3_000_000] {
            let (y, m, dd) = civil_from_days(d);
            assert_eq!(days_from_civil(y, m, dd), d);
        }
        assert_eq!(fmt_date(days_from_civil(1816, 1, 1)), "1816-01-01");
        assert_eq!(fmt_date(from_week(1817, 1, 1).unwrap()), "1816-12-30");
        assert_eq!(fmt_date(from_week(1818, 53, 1).unwrap()), "1818-12-28");
    }

    #[test]
    fn parsing_and_rendering() {
        use TemporalKind::*;
        assert_eq!(p(Date, "20150721"), "2015-07-21");
        assert_eq!(p(Date, "2015-W30-2"), "2015-07-21");
        assert_eq!(p(Date, "2015-202"), "2015-07-21");
        assert_eq!(p(Date, "2015"), "2015-01-01");
        assert_eq!(p(Date, "+999999999-12-31"), "+999999999-12-31");
        assert_eq!(p(LocalTime, "214032.142"), "21:40:32.142");
        assert_eq!(p(Time, "21:40-01:30"), "21:40-01:30");
        assert_eq!(
            p(DateTime, "2015-07-21T21:40:32.142[Europe/London]"),
            "2015-07-21T21:40:32.142+01:00[Europe/London]"
        );
        assert_eq!(
            p(DateTime, "1818-07-21T21:40:32.142[Europe/Stockholm]"),
            "1818-07-21T21:40:32.142+00:53:28[Europe/Stockholm]"
        );
        assert_eq!(p(Duration, "P5M1.5D"), "P5M1DT12H");
        assert_eq!(p(Duration, "P0.75M"), "P22DT19H51M49.5S");
        assert_eq!(p(Duration, "PT0.75M"), "PT45S");
        assert_eq!(p(Duration, "P2.5W"), "P17DT12H");
    }
}
