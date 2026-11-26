//! What the DAB receiver knows about the ensemble (port of the Android fork's DabModel/DabEvents).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DabChannel {
    pub name: &'static str,
    pub freq: u64,
}

pub const BAND_III: [DabChannel; 41] = {
    const fn c(name: &'static str, khz: u64) -> DabChannel {
        DabChannel { name, freq: khz * 1000 }
    }
    [
        c("5A", 174_928), c("5B", 176_640), c("5C", 178_352), c("5D", 180_064),
        c("6A", 181_936), c("6B", 183_648), c("6C", 185_360), c("6D", 187_072),
        c("7A", 188_928), c("7B", 190_640), c("7C", 192_352), c("7D", 194_064),
        c("8A", 195_936), c("8B", 197_648), c("8C", 199_360), c("8D", 201_072),
        c("9A", 202_928), c("9B", 204_640), c("9C", 206_352), c("9D", 208_064),
        c("10A", 209_936), c("10N", 210_096), c("10B", 211_648), c("10C", 213_360), c("10D", 215_072),
        c("11A", 216_928), c("11N", 217_088), c("11B", 218_640), c("11C", 220_352), c("11D", 222_064),
        c("12A", 223_936), c("12N", 224_096), c("12B", 225_648), c("12C", 227_360), c("12D", 229_072),
        c("13A", 230_784), c("13B", 232_496), c("13C", 234_208), c("13D", 235_776), c("13E", 237_488), c("13F", 239_200),
    ]
};

pub fn channel_by_name(name: &str) -> Option<DabChannel> {
    BAND_III.iter().copied().find(|c| c.name.eq_ignore_ascii_case(name))
}

pub fn channel_for_freq(f: u64) -> Option<DabChannel> {
    BAND_III.iter().copied().min_by_key(|c| c.freq.abs_diff(f)).filter(|c| c.freq.abs_diff(f) <= 20_000)
}

/// The N channels overlap their neighbours and are rarely used: the scan skips them.
pub fn scan_list() -> Vec<DabChannel> {
    BAND_III.iter().copied().filter(|c| !c.name.ends_with('N')).collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DabService {
    pub sid: u32,
    pub label: String,
    pub short_label: String,
    /// None: not known yet
    pub is_audio: Option<bool>,
    pub subch: i32,
    pub start_cu: i32,
    pub length_cu: i32,
    pub bitrate: i32,
    pub short_form: bool,
    pub prot_level: i32,
    pub ascty: i32,
    pub pty: i32,
    pub language: i32,
}

impl DabService {
    pub fn new(sid: u32, label: String) -> Self {
        Self { sid, label, subch: -1, start_cu: -1, ascty: -1, ..Default::default() }
    }
    pub fn described(&self) -> bool {
        self.is_audio == Some(false) || self.subch >= 0
    }
    pub fn codec(&self) -> &'static str {
        match (self.is_audio, self.ascty) {
            (Some(false), _) => "Data",
            (_, 63) => "DAB+ (HE-AAC)",
            (_, 0) => "DAB (MP2)",
            _ => "-",
        }
    }
    pub fn codec_short(&self) -> &'static str {
        match (self.is_audio, self.ascty) {
            (Some(false), _) => "data",
            (_, 63) => "DAB+",
            (_, 0) => "DAB",
            _ => "",
        }
    }
    pub fn protection(&self) -> String {
        if self.subch < 0 {
            "-".into()
        } else if self.short_form {
            format!("UEP-{}", self.prot_level)
        } else if (0..=3).contains(&self.prot_level) {
            format!("EEP-{}A", self.prot_level + 1)
        } else if (4..=7).contains(&self.prot_level) {
            format!("EEP-{}B", self.prot_level - 3)
        } else {
            "?".into()
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DabTransmitter {
    pub main_id: i32,
    pub sub_id: i32,
    pub strength: f32,
    pub last_seen: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DabQuality {
    pub fic: i32,
    pub frames: i32,
    pub rs: i32,
    pub aac: i32,
}

#[derive(Clone, Debug, Default)]
pub struct DabState {
    pub active: bool,
    pub frequency: u64,
    pub sync: bool,
    pub sync_seen: bool,
    pub snr: i32,
    pub freq_offset: i32,
    pub quality: DabQuality,
    pub ensemble_label: String,
    pub ensemble_id: i64,
    pub time_utc: String,
    pub services: Vec<DabService>,
    pub selected_sid: Option<u32>,
    pub playing: Option<DabService>,
    pub dls: String,
    pub dls_history: Vec<String>,
    pub slide: Option<std::sync::Arc<Vec<u8>>>,
    pub slide_name: String,
    pub slide_type: i32,
    pub slide_serial: u32,
    pub audio_rate: i32,
    pub audio_buffered_ms: i32,
    pub dropped_samples: u64,
    pub input_rate: i32,
    pub input_level_db: f32,
    pub audio_in_per_sec: i32,
    pub audio_out_per_sec: i32,
    pub transmitters: Vec<DabTransmitter>,
    pub seconds_since_tune: f32,
    pub snr_history: Vec<f32>,
    pub constellation: Vec<[f32; 2]>,
}

impl DabState {
    pub fn channel(&self) -> Option<DabChannel> {
        channel_for_freq(self.frequency)
    }
    pub fn selected(&self) -> Option<&DabService> {
        self.selected_sid.and_then(|s| self.services.iter().find(|x| x.sid == s))
    }
    pub fn ensemble_id_hex(&self) -> String {
        if self.ensemble_id < 0 { "-".into() } else { format!("{:04X}", self.ensemble_id & 0xFFFF) }
    }
    pub fn used_cu(&self) -> i32 {
        let mut seen = std::collections::HashSet::new();
        self.services.iter().filter(|s| s.subch >= 0 && seen.insert(s.subch)).map(|s| s.length_cu).sum()
    }

    /// Applies one JSON event line from the native receiver.
    pub fn apply_event(&mut self, line: &str, now: f64) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { return };
        let int = |k: &str| v.get(k).and_then(|x| x.as_i64()).unwrap_or(0);
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
        let b = |k: &str| v.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
        match v.get("t").and_then(|x| x.as_str()).unwrap_or("") {
            "sync" => {
                self.sync = b("v");
                self.sync_seen |= self.sync;
            }
            "sys" => {
                self.sync = b("sync");
                self.sync_seen |= self.sync;
                self.snr = int("snr") as i32;
                self.freq_offset = int("off") as i32;
            }
            "fic" => self.quality.fic = (int("q") as i32).clamp(0, 100),
            "ens" => {
                self.ensemble_label = s("name");
                self.ensemble_id = int("id") & 0xFFFF_FFFF;
            }
            "svc" => {
                let sid = (int("sid") & 0xFFFF_FFFF) as u32;
                if !self.services.iter().any(|x| x.sid == sid) {
                    self.services.push(DabService::new(sid, s("name")));
                }
            }
            t @ ("info" | "prog") => {
                let sid = (int("sid") & 0xFFFF_FFFF) as u32;
                let d = DabService {
                    sid,
                    label: s("name"),
                    short_label: s("shortName"),
                    is_audio: Some(true),
                    subch: int("subch") as i32,
                    start_cu: int("start") as i32,
                    length_cu: int("length") as i32,
                    bitrate: int("bitrate") as i32,
                    short_form: b("short"),
                    prot_level: int("prot") as i32,
                    ascty: int("ascty") as i32,
                    pty: int("pty") as i32,
                    language: int("lang") as i32,
                };
                if let Some(old) = self.services.iter_mut().find(|x| x.sid == sid) {
                    let label = if d.label.is_empty() { old.label.clone() } else { d.label.clone() };
                    *old = DabService { label, ..d.clone() };
                } else {
                    self.services.push(d.clone());
                }
                if t == "prog" {
                    self.playing = Some(d);
                    self.selected_sid = Some(sid);
                    self.dls.clear();
                    self.quality.frames = 0;
                    self.quality.rs = 0;
                    self.quality.aac = 0;
                }
            }
            "data" => {
                let sid = (int("sid") & 0xFFFF_FFFF) as u32;
                if let Some(x) = self.services.iter_mut().find(|x| x.sid == sid) {
                    x.is_audio = Some(false);
                }
            }
            "time" => self.time_utc = format!("{:02}:{:02}", int("h"), int("m")),
            "dls" => {
                let t = s("text");
                if !t.is_empty() && t != self.dls {
                    self.dls_history.retain(|x| x != &t);
                    self.dls_history.insert(0, t.clone());
                    self.dls_history.truncate(30);
                }
                self.dls = t;
            }
            "pq" => {
                self.quality.frames = (int("fe") as i32).clamp(0, 100);
                self.quality.rs = (int("rs") as i32).clamp(0, 100);
                self.quality.aac = (int("aac") as i32).clamp(0, 100);
            }
            "tii" => {
                let tx = DabTransmitter {
                    main_id: int("main") as i32,
                    sub_id: int("sub") as i32,
                    strength: v.get("strength").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                    last_seen: now,
                };
                self.transmitters.retain(|t| !(t.main_id == tx.main_id && t.sub_id == tx.sub_id) && now - t.last_seen < 60.0);
                self.transmitters.push(tx);
                self.transmitters.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap_or(std::cmp::Ordering::Equal));
                self.transmitters.truncate(12);
            }
            _ => {}
        }
    }
}

/// A scan's finding, remembered between runs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedEnsemble {
    pub channel: String,
    pub frequency: u64,
    pub label: String,
    pub ensemble_id: i64,
    pub services: Vec<String>,
}

#[derive(Clone, Debug, Default)]
pub struct DabScanState {
    pub running: bool,
    pub index: usize,
    pub count: usize,
    pub current: String,
    pub found: Vec<SavedEnsemble>,
    pub log: Vec<String>,
    pub finished: bool,
}

pub const PTY: [&str; 32] = [
    "None", "News", "Current Affairs", "Information", "Sport", "Education", "Drama", "Culture", "Science", "Varied", "Pop Music",
    "Rock Music", "Easy Listening", "Light Classical", "Serious Classical", "Other Music", "Weather", "Finance", "Children's",
    "Social Affairs", "Religion", "Phone In", "Travel", "Leisure", "Jazz Music", "Country Music", "National Music", "Oldies Music",
    "Folk Music", "Documentary", "Alarm Test", "Alarm",
];

pub fn pty_name(code: i32) -> String {
    PTY.get(code as usize).map(|s| s.to_string()).unwrap_or_else(|| format!("PTY {code}"))
}

pub fn language_name(code: i32) -> String {
    match code {
        0 => "-".into(),
        0x01 => "Albanian".into(),
        0x05 => "Czech".into(),
        0x06 => "Danish".into(),
        0x07 => "German".into(),
        0x09 => "English".into(),
        0x0A => "Spanish".into(),
        0x0F => "French".into(),
        0x13 => "Croatian".into(),
        0x15 => "Italian".into(),
        0x1D => "Dutch".into(),
        0x1E => "Norwegian".into(),
        0x20 => "Polish".into(),
        0x21 => "Portuguese".into(),
        0x24 => "Slovak".into(),
        0x27 => "Finnish".into(),
        0x28 => "Swedish".into(),
        0x29 => "Turkish".into(),
        0x41 => "Frisian".into(),
        0x56 => "Russian".into(),
        _ => format!("0x{code:02X}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn events_build_state() {
        let mut s = DabState::default();
        s.apply_event(r#"{"t":"ens","name":"NPO       ","id":33282}"#, 0.0);
        s.apply_event(r#"{"t":"svc","name":"NPO Radio 1","sid":33281,"sub":0}"#, 0.0);
        s.apply_event(
            r#"{"t":"prog","name":"NPO Radio 1","shortName":"NPO 1","sid":33281,"pty":1,"subch":3,"start":0,"length":72,"short":false,"prot":2,"bitrate":96,"ascty":63,"lang":29}"#,
            0.0,
        );
        s.apply_event(r#"{"t":"sys","sync":true,"snr":14,"off":-120}"#, 0.0);
        assert_eq!(s.ensemble_label, "NPO");
        assert_eq!(s.services.len(), 1);
        assert_eq!(s.services[0].protection(), "EEP-3A");
        assert_eq!(s.services[0].codec(), "DAB+ (HE-AAC)");
        assert_eq!(s.selected_sid, Some(33281));
        assert!(s.sync_seen);
        assert_eq!(channel_by_name("12c").unwrap().freq, 227_360_000);
    }
}
