//! L'historique d'une source analysée (une mesure par seconde, 10 minutes) et son journal
//! d'événements (2000 au plus) : ce qui s'est passé pendant qu'on regardait ailleurs.
//!
//! - Les séries : un nombre (ou rien) par seconde, toutes alignées sur la même horloge.
//! - Les états (noir, figée, silence, saturation, pas d'image, pertes) : un masque de bits par
//!   seconde, dessiné en bandes.
//! - Les événements : le début et la fin d'un défaut (avec sa durée), les pertes de paquets, les
//!   changements de format, l'horloge PTP.

use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, VecDeque};

/// Dix minutes, une mesure par seconde.
pub const SPAN: usize = 600;
/// Le journal garde les 2000 derniers événements.
pub const EVENTS: usize = 2000;

/// Les bits d'état d'une seconde.
pub const BLACK: u32 = 1;
pub const FROZEN: u32 = 2;
pub const SILENCE: u32 = 4;
pub const CLIP: u32 = 8;
pub const NO_PICTURE: u32 = 16;
pub const LOSS: u32 = 32;
pub const NO_SOUND: u32 = 64;
pub const LIP: u32 = 128;
pub const PTP: u32 = 256;

#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Serialize, Clone, Debug)]
pub struct Event {
    pub id: u64,
    /// L'heure (ms depuis 1970).
    pub t: i64,
    pub level: Level,
    /// Le code de l'événement (`black`, `blackEnd`, `loss`…), traduit par l'interface.
    pub code: &'static str,
    pub args: Value,
}

/// Un défaut qui commence et finit : un événement à chaque bout, la durée à la fin.
#[derive(Default)]
struct Watch {
    since: Option<i64>,
    /// Les détails du début (repris à la fin : « silence sur 7-8 » … « revenu sur 7-8 »).
    args: Value,
}

#[derive(Default)]
pub struct History {
    /// L'heure de la dernière mesure (s depuis 1970).
    pub last: Option<i64>,
    series: BTreeMap<&'static str, VecDeque<Option<f32>>>,
    states: VecDeque<u32>,
    events: VecDeque<Event>,
    next_id: u64,
    watches: BTreeMap<&'static str, Watch>,
}

impl History {
    /// Une seconde de plus : les valeurs de chaque série (absente : rien) et les états.
    pub fn sample(&mut self, t: i64, values: &[(&'static str, Option<f32>)], state: u32) {
        // Une seconde sautée (le fil a pris du retard) : des trous, l'axe reste juste.
        let gap = self.last.map_or(1, |l| (t - l).clamp(1, SPAN as i64)) as usize;
        if self.last.is_some_and(|l| t <= l) {
            return;
        }
        for _ in 1..gap {
            for s in self.series.values_mut() {
                s.push_back(None);
            }
            self.states.push_back(0);
        }
        let len = self.states.len();
        for (k, v) in values {
            let s = self.series.entry(k).or_insert_with(|| std::iter::repeat_n(None, len).collect());
            s.push_back(*v);
        }
        for (k, s) in self.series.iter_mut() {
            if !values.iter().any(|(n, _)| n == k) {
                s.push_back(None);
            }
            while s.len() > SPAN {
                s.pop_front();
            }
        }
        self.states.push_back(state);
        while self.states.len() > SPAN {
            self.states.pop_front();
        }
        self.last = Some(t);
    }

    pub fn event(&mut self, t: i64, level: Level, code: &'static str, args: Value) {
        self.next_id += 1;
        self.events.push_back(Event { id: self.next_id, t, level, code, args });
        while self.events.len() > EVENTS {
            self.events.pop_front();
        }
    }

    /// Un défaut suivi : `code` quand il commence, `end` (avec sa durée en s) quand il finit.
    pub fn watch(&mut self, t: i64, on: bool, level: Level, code: &'static str, end: &'static str, args: Value) {
        let w = self.watches.entry(code).or_default();
        match (on, w.since) {
            (true, None) => {
                w.since = Some(t);
                w.args = args.clone();
                self.event(t, level, code, args);
            }
            (false, Some(s)) => {
                w.since = None;
                let mut a = std::mem::take(&mut w.args);
                let secs = json!(((t - s) as f64 / 1000.0 * 10.0).round() / 10.0);
                match &mut a {
                    Value::Object(m) => {
                        m.insert("s".into(), secs);
                    }
                    _ => a = json!({ "s": secs }),
                }
                self.event(t, Level::Info, end, a);
            }
            _ => {}
        }
    }

    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    /// Pour l'interface : les séries, les états, et les derniers événements (`limit`).
    pub fn view(&self, limit: usize) -> Value {
        let n = self.states.len();
        let errors = self.events.iter().filter(|e| e.level == Level::Error).count();
        let warnings = self.events.iter().filter(|e| e.level == Level::Warn).count();
        json!({
            "end": self.last,
            "length": n,
            "series": self.series,
            "states": self.states,
            "events": self.events.iter().rev().take(limit).collect::<Vec<_>>(),
            "total": self.events.len(),
            "errors": errors,
            "warnings": warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_stay_aligned() {
        let mut h = History::default();
        h.sample(100, &[("a", Some(1.0))], 0);
        h.sample(101, &[("a", Some(2.0)), ("b", Some(5.0))], BLACK);
        // Une seconde sautée : un trou partout.
        h.sample(103, &[("b", Some(6.0))], 0);
        let v = h.view(10);
        assert_eq!(v["length"], 4);
        assert_eq!(v["series"]["a"], json!([1.0, 2.0, null, null]));
        assert_eq!(v["series"]["b"], json!([null, 5.0, null, 6.0]));
        assert_eq!(v["states"], json!([0, 1, 0, 0]));
        for t in 0..700 {
            h.sample(200 + t, &[("a", Some(0.0))], 0);
        }
        assert_eq!(h.view(0)["length"], SPAN);
        assert_eq!(h.view(0)["series"]["b"].as_array().unwrap().len(), SPAN);
    }

    #[test]
    fn a_fault_has_a_start_and_an_end() {
        let mut h = History::default();
        h.watch(1_000, true, Level::Warn, "black", "blackEnd", json!({}));
        h.watch(1_500, true, Level::Warn, "black", "blackEnd", json!({}));
        h.watch(3_500, false, Level::Warn, "black", "blackEnd", json!({}));
        h.watch(4_000, true, Level::Warn, "silence", "silenceEnd", json!({ "channels": [7, 8] }));
        h.watch(5_000, false, Level::Warn, "silence", "silenceEnd", json!({ "channels": [] }));
        assert_eq!(h.view(10)["events"][0]["args"], json!({ "channels": [7, 8], "s": 1.0 }));
        let v = h.view(10);
        assert_eq!(v["total"], 4);
        assert_eq!(v["events"][2]["code"], "blackEnd");
        assert_eq!(v["events"][2]["args"]["s"], 2.5);
        assert_eq!(v["warnings"], 2);
    }
}
