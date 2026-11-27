//! Le timecode maison, calculé à partir de l'heure du serveur.
//!
//! Quand l'heure du serveur est tenue par le PTP (SMPTE ST 2059), tous les serveurs calculent le
//! même timecode, à l'image près, sans câble LTC : c'est l'heure du jour (locale, UTC ou décalée),
//! comptée en images à la cadence maison.
//!
//! - **Cadences entières** (25, 30, 24) : les images tombent sur les secondes ; le timecode est
//!   l'heure du jour en images.
//! - **Cadences NTSC** (29,97 ; 23,98) : une image dure 1001/30000 s, le timecode prend donc du
//!   retard sur l'heure (86 ms par jour en drop frame, 86 s sans). Comme le veut ST 2059-1, il est
//!   **recalé chaque jour** (« daily jam ») à une heure choisie, puis compte les images ; leurs
//!   instants sont alignés sur l'époque PTP (1970, en TAI).
use serde::{Deserialize, Serialize};
use std::fmt;

/// La cadence maison (la famille de toutes les images du site).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HouseRate {
    /// 25 images/s (50 Hz : 625i50, 1080i50, 720p50, 1080p25…).
    #[default]
    #[serde(rename = "25")]
    R25,
    /// 30000/1001 (59,94 Hz : 525i59.94, 1080i59.94…).
    #[serde(rename = "29.97")]
    R2997,
    /// 30 images/s (60 Hz).
    #[serde(rename = "30")]
    R30,
    /// 24 images/s.
    #[serde(rename = "24")]
    R24,
    /// 24000/1001.
    #[serde(rename = "23.98")]
    R2398,
}

impl HouseRate {
    /// Images par seconde : (numérateur, dénominateur).
    pub fn ratio(self) -> (u64, u64) {
        match self {
            HouseRate::R25 => (25, 1),
            HouseRate::R2997 => (30000, 1001),
            HouseRate::R30 => (30, 1),
            HouseRate::R24 => (24, 1),
            HouseRate::R2398 => (24000, 1001),
        }
    }

    /// Les images comptées par seconde de timecode (25, 30, 24).
    pub fn nominal(self) -> u64 {
        match self {
            HouseRate::R25 => 25,
            HouseRate::R2997 | HouseRate::R30 => 30,
            HouseRate::R24 | HouseRate::R2398 => 24,
        }
    }

    pub fn fps(self) -> f64 {
        let (n, d) = self.ratio();
        n as f64 / d as f64
    }

    /// La cadence maison d'une cadence d'images ou de trames (50 → 25, 59.94 → 29.97).
    pub fn of_fps(fps: f64) -> Option<HouseRate> {
        let f = if fps > 31.0 { fps / 2.0 } else { fps };
        [HouseRate::R25, HouseRate::R2997, HouseRate::R30, HouseRate::R24, HouseRate::R2398].into_iter().find(|r| (r.fps() - f).abs() < 0.005)
    }
}

/// D'où vient l'heure du timecode.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TcOffset {
    /// L'heure locale du serveur (son fuseau, heure d'été comprise).
    #[default]
    Local,
    Utc,
    /// UTC plus un nombre de minutes fixe.
    Fixed,
}

/// Le timecode maison : réglé dans *Matériel › Horloge*, lu par les applis.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct HouseClock {
    pub rate: HouseRate,
    pub offset: TcOffset,
    /// Le décalage sur UTC en minutes (avec `Fixed`).
    pub fixed_minutes: i32,
    /// Drop frame (29,97 seulement).
    pub drop_frame: bool,
    /// L'heure du recalage quotidien (cadences NTSC), « HH:MM » dans l'heure du timecode.
    pub daily_jam: String,
}

impl Default for HouseClock {
    fn default() -> Self {
        HouseClock { rate: HouseRate::R25, offset: TcOffset::Local, fixed_minutes: 0, drop_frame: true, daily_jam: "00:00".into() }
    }
}

/// Un timecode.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub drop_frame: bool,
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02}:{:02}:{:02}{}{:02}", self.hours, self.minutes, self.seconds, if self.drop_frame { ';' } else { ':' }, self.frames)
    }
}

const NS: i128 = 1_000_000_000;
const DAY: i64 = 86_400;
/// Images d'un jour en drop frame 29,97.
const DF_DAY: i64 = 2_589_408;

impl Timecode {
    /// Le numéro de l'image dans la journée, à `nominal` images par seconde de timecode.
    pub fn frame_of_day(&self, nominal: u64) -> i64 {
        let n = nominal as i64;
        let secs = self.hours as i64 * 3600 + self.minutes as i64 * 60 + self.seconds as i64;
        if self.drop_frame && n == 30 {
            let mins = self.hours as i64 * 60 + self.minutes as i64;
            secs * 30 + self.frames as i64 - 2 * (mins - mins / 10)
        } else {
            secs * n + self.frames as i64
        }
    }

    /// L'image `frame` de la journée.
    pub fn from_frame(frame: i64, nominal: u64, drop_frame: bool) -> Timecode {
        let n = nominal as i64;
        let df = drop_frame && n == 30;
        let mut f = frame.rem_euclid(if df { DF_DAY } else { DAY * n });
        if df {
            let (d, m) = (f / 17_982, f % 17_982);
            f += 18 * d + if m > 2 { 2 * ((m - 2) / 1798) } else { 0 };
        }
        Timecode { hours: (f / (3600 * n)) as u8, minutes: (f / (60 * n) % 60) as u8, seconds: (f / n % 60) as u8, frames: (f % n) as u8, drop_frame: df }
    }

    /// L'écart en images (`self − other`), ramené à moins d'une demi-journée.
    pub fn frames_from(&self, other: &Timecode, nominal: u64) -> i64 {
        let day = if self.drop_frame && nominal == 30 { DF_DAY } else { DAY * nominal as i64 };
        let d = (self.frame_of_day(nominal) - other.frame_of_day(nominal)).rem_euclid(day);
        if d > day / 2 {
            d - day
        } else {
            d
        }
    }
}

/// « HH:MM » → secondes du jour.
fn jam_seconds(s: &str) -> i64 {
    let (h, m) = s.trim().split_once(':').unwrap_or(("0", "0"));
    let (h, m): (i64, i64) = (h.parse().unwrap_or(0), m.parse().unwrap_or(0));
    (h.clamp(0, 23) * 60 + m.clamp(0, 59)) * 60
}

/// Le décalage sur UTC (s) de l'heure du timecode à l'instant `unix_s`. `tz` : le fuseau du serveur
/// (« Europe/Paris ») ; inconnu, celui du système.
pub fn utc_offset(clock: &HouseClock, tz: &str, unix_s: i64) -> i32 {
    match clock.offset {
        TcOffset::Utc => 0,
        TcOffset::Fixed => clock.fixed_minutes.clamp(-24 * 60, 24 * 60) * 60,
        TcOffset::Local => {
            let zone = jiff::tz::TimeZone::get(tz).unwrap_or_else(|_| jiff::tz::TimeZone::system());
            jiff::Timestamp::from_second(unix_s).map_or(0, |t| zone.to_offset(t).seconds())
        }
    }
}

/// Le timecode maison à l'instant `unix_ns` (ns depuis 1970, UTC). `tai_offset` : TAI − UTC (37 s).
pub fn house_timecode(unix_ns: i128, clock: &HouseClock, tz: &str, tai_offset: u32) -> Timecode {
    let unix_s = unix_ns.div_euclid(NS) as i64;
    let off = utc_offset(clock, tz, unix_s) as i128;
    let nominal = clock.rate.nominal();
    let (num, den) = clock.rate.ratio();
    if den == 1 {
        // Les images tombent sur les secondes : l'heure du jour en images.
        let day_ns = (unix_ns + off * NS).rem_euclid(DAY as i128 * NS);
        let frame = (day_ns * num as i128 / NS) as i64;
        return Timecode::from_frame(frame, nominal, false);
    }
    // NTSC : les images depuis le dernier recalage, alignées sur l'époque PTP (TAI).
    let drop_frame = clock.drop_frame && nominal == 30;
    let local_s = unix_s as i128 + off;
    let mut jam_local = local_s - local_s.rem_euclid(DAY as i128) + jam_seconds(&clock.daily_jam) as i128;
    if jam_local > local_s {
        jam_local -= DAY as i128;
    }
    let tai = tai_offset as i128 * NS;
    let per = den as i128 * NS;
    let frame_now = (unix_ns + tai) * num as i128 / per;
    let jam_ns = (jam_local - off) * NS + tai;
    let frame_jam = (jam_ns * num as i128 + per - 1) / per;
    let n = (frame_now - frame_jam).max(0) as i64;
    let at_jam = Timecode {
        hours: (jam_local.rem_euclid(DAY as i128) / 3600) as u8,
        minutes: (jam_local.rem_euclid(3600) / 60) as u8,
        seconds: 0,
        frames: 0,
        drop_frame,
    };
    Timecode::from_frame(at_jam.frame_of_day(nominal) + n, nominal, drop_frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(s: i64) -> i128 {
        s as i128 * NS
    }

    #[test]
    fn drop_frame_counts() {
        for f in [0i64, 1, 1799, 1800, 1801, 17_981, 17_982, 107_892, DF_DAY - 1] {
            let tc = Timecode::from_frame(f, 30, true);
            assert_eq!(tc.frame_of_day(30), f, "{tc}");
        }
        assert_eq!(Timecode::from_frame(1800, 30, true).to_string(), "00:01:00;02");
        assert_eq!(Timecode::from_frame(17_982, 30, true).to_string(), "00:10:00;00");
        assert_eq!(Timecode::from_frame(DF_DAY, 30, true).to_string(), "00:00:00;00");
    }

    #[test]
    fn integer_rates_follow_the_time_of_day() {
        // 2026-11-27 12:34:56.5 UTC.
        let t = 1_795_782_896;
        let utc = HouseClock { offset: TcOffset::Utc, ..HouseClock::default() };
        assert_eq!(house_timecode(ns(t) + NS / 2, &utc, "UTC", 37).to_string(), "12:34:56:12");
        let paris = HouseClock::default();
        assert_eq!(house_timecode(ns(t), &paris, "Europe/Paris", 37).to_string(), "13:34:56:00");
        // En été, Paris est à UTC+2.
        assert_eq!(utc_offset(&paris, "Europe/Paris", 1_782_000_000), 7200);
        let fixed = HouseClock { offset: TcOffset::Fixed, fixed_minutes: -90, ..HouseClock::default() };
        assert_eq!(house_timecode(ns(t), &fixed, "Europe/Paris", 37).to_string(), "11:04:56:00");
    }

    #[test]
    fn ntsc_rates_count_from_the_daily_jam() {
        let midnight = 1_795_737_600; // 2026-11-27 00:00:00 UTC
        let c = HouseClock { rate: HouseRate::R2997, offset: TcOffset::Utc, ..HouseClock::default() };
        // Juste après le recalage : 00:00:00;00.
        assert_eq!(house_timecode(ns(midnight) + NS / 100, &c, "UTC", 37).to_string(), "00:00:00;00");
        // Une heure plus tard, en drop frame, le timecode suit l'heure à quelques ms près.
        let tc = house_timecode(ns(midnight + 3600), &c, "UTC", 37);
        assert_eq!((tc.hours, tc.minutes), (0, 59));
        assert_eq!(tc.seconds, 59);
        let ndf = HouseClock { drop_frame: false, ..c.clone() };
        // Sans drop frame, il prend 3,6 s de retard par heure.
        let tc = house_timecode(ns(midnight + 3600), &ndf, "UTC", 37);
        assert_eq!((tc.minutes, tc.seconds), (59, 56));
        // Recalé à 06:00 : à 05:59, on compte encore depuis la veille.
        let six = HouseClock { daily_jam: "06:00".into(), ..c };
        assert_eq!(house_timecode(ns(midnight + 6 * 3600) + NS / 100, &six, "UTC", 37).to_string(), "06:00:00;00");
        let before = house_timecode(ns(midnight + 6 * 3600 - 1), &six, "UTC", 37);
        assert_eq!(before.hours, 5);
    }

    #[test]
    fn rates_and_gaps() {
        assert_eq!(HouseRate::of_fps(50.0), Some(HouseRate::R25));
        assert_eq!(HouseRate::of_fps(59.94), Some(HouseRate::R2997));
        assert_eq!(HouseRate::of_fps(23.976), Some(HouseRate::R2398));
        assert_eq!(HouseRate::of_fps(12.0), None);
        let a = Timecode { hours: 0, minutes: 0, seconds: 0, frames: 1, drop_frame: false };
        let b = Timecode { hours: 23, minutes: 59, seconds: 59, frames: 24, drop_frame: false };
        assert_eq!(a.frames_from(&b, 25), 2);
        assert_eq!(b.frames_from(&a, 25), -2);
    }
}
