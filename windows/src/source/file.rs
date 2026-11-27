//! IQ file playback in real time (cu8, cs8, cs16, cf32), optionally looping.

use super::{convert, IqBlock, Source};
use crate::settings::{FileFormat, SourceConfig};
use crossbeam_channel::Sender;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct FileSource {
    cfg: SourceConfig,
    /// Shared with the reader thread; set false to make it exit.
    running: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Reads frequency and sample rate from names like `RFA_20261126_1200_97000000Hz_2400000sps.cu8`
/// or SDR# / gqrx style names (`..._97000000Hz_...`, `..._2400000sps...` / `..._2.4MSPS...`).
pub fn parse_file_name(name: &str) -> (Option<u64>, Option<u32>, Option<FileFormat>) {
    let lower = name.to_ascii_lowercase();
    let fmt = std::path::Path::new(&lower).extension().and_then(|e| e.to_str()).and_then(FileFormat::from_ext);
    let mut freq = None;
    let mut rate = None;
    // split the name into tokens and recognise "<number>[k|m]hz" and "<number>[k|m]sps"
    for part in lower.split(|c: char| c == '_' || c == '-' || c == ' ' || c == '.') {
        if let Some(v) = part.strip_suffix("hz") {
            if let Some(k) = v.strip_suffix('k') {
                freq = k.parse::<f64>().ok().map(|f| (f * 1e3) as u64);
            } else if let Some(m) = v.strip_suffix('m') {
                freq = m.parse::<f64>().ok().map(|f| (f * 1e6) as u64);
            } else if let Ok(f) = v.parse::<u64>() {
                freq = Some(f);
            }
        } else if let Some(v) = part.strip_suffix("msps") {
            rate = v.parse::<f64>().ok().map(|r| (r * 1e6) as u32);
        } else if let Some(v) = part.strip_suffix("ksps") {
            rate = v.parse::<f64>().ok().map(|r| (r * 1e3) as u32);
        } else if let Some(v) = part.strip_suffix("sps") {
            rate = v.parse::<u32>().ok();
        }
    }
    (freq, rate, fmt)
}

impl FileSource {
    pub fn open(cfg: &SourceConfig) -> Result<Self, String> {
        if cfg.file_path.is_empty() {
            return Err("Choose an IQ file first (Source panel).".into());
        }
        File::open(&cfg.file_path).map_err(|e| format!("Could not open {}: {e}", cfg.file_path))?;
        Ok(Self { cfg: cfg.clone(), running: Arc::new(AtomicBool::new(false)), thread: None })
    }
}

impl Source for FileSource {
    fn start(&mut self, tx: Sender<IqBlock>) -> Result<(), String> {
        let f = File::open(&self.cfg.file_path).map_err(|e| e.to_string())?;
        let mut rd = BufReader::with_capacity(1 << 20, f);
        let fmt = self.cfg.file_format;
        let rate = self.cfg.file_rate.max(1000);
        let freq = self.cfg.file_frequency;
        let repeat = self.cfg.file_repeat;
        let running = self.running.clone();
        running.store(true, Ordering::SeqCst);
        self.thread = Some(
            std::thread::Builder::new()
                .name("file-rx".into())
                .spawn(move || {
                    // 50 ms blocks, like a real device would deliver
                    let block_samples = (rate as usize / 20).max(1024);
                    let block_bytes = block_samples * fmt.bytes_per_sample();
                    let start = Instant::now();
                    let mut sent: u64 = 0;
                    while running.load(Ordering::Relaxed) {
                        let mut raw = vec![0u8; block_bytes];
                        // read() may return less than asked; loop until the block is full,
                        // rewinding at end of file when looping
                        let mut got = 0;
                        while got < block_bytes {
                            match rd.read(&mut raw[got..]) {
                                Ok(0) => {
                                    if repeat && rd.seek(SeekFrom::Start(0)).is_ok() {
                                        continue;
                                    }
                                    break;
                                }
                                Ok(n) => got += n,
                                Err(_) => break,
                            }
                        }
                        if got == 0 {
                            break;
                        }
                        // drop a trailing partial sample
                        raw.truncate(got - got % fmt.bytes_per_sample());
                        let mut samples = Vec::new();
                        convert(fmt, &raw, &mut samples);
                        sent += samples.len() as u64;
                        if tx.send(IqBlock { samples, raw, frequency: freq, rate }).is_err() {
                            break;
                        }
                        // pace to real time: sleep until wall-clock time catches up with the
                        // amount of signal sent (no drift, since it's measured from the start)
                        let due = Duration::from_secs_f64(sent as f64 / rate as f64);
                        let el = start.elapsed();
                        if due > el {
                            std::thread::sleep(due - el);
                        }
                    }
                    running.store(false, Ordering::SeqCst);
                })
                .map_err(|e| e.to_string())?,
        );
        Ok(())
    }

    fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    fn apply(&mut self, _cfg: &SourceConfig) -> Result<(), String> {
        Ok(())
    }

    fn raw_format(&self) -> FileFormat {
        self.cfg.file_format
    }

    fn description(&self) -> String {
        let name = std::path::Path::new(&self.cfg.file_path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        format!("File {name}")
    }

    fn fixed_tuning(&self) -> Option<(u64, u32)> {
        Some((self.cfg.file_frequency, self.cfg.file_rate))
    }
}

impl Drop for FileSource {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_names() {
        let (f, r, fmt) = parse_file_name("RFA_20261126_2100_97000000Hz_2400000sps.cu8");
        assert_eq!(f, Some(97_000_000));
        assert_eq!(r, Some(2_400_000));
        assert_eq!(fmt, Some(FileFormat::Cu8));
        let (f, r, _) = parse_file_name("gqrx_20200101_225000_100000000_2000000_fc.raw");
        assert_eq!((f, r), (None, None));
        let (f, r, _) = parse_file_name("HDSDR_20200101_225000Z_7100kHz_RF.cs16");
        assert_eq!(f, Some(7_100_000));
        let _ = r;
    }
}
