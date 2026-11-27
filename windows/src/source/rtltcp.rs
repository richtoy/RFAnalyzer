//! rtl_tcp network source (e.g. rtl_tcp on another PC, or an Android rtl_tcp driver).
//!
//! Protocol: on connect the server sends a 12-byte header ("RTL0", tuner type, gain count,
//! big-endian), then an endless stream of unsigned 8-bit IQ. Control goes the other way as
//! 5-byte commands: 1 byte command id + a big-endian u32 parameter (see `cmd` calls).

use super::{convert_u8, hw_freq, IqBlock, Source};
use crate::settings::{FileFormat, SourceConfig};
use crossbeam_channel::Sender;
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// R820T gain steps in tenths of dB (rtl_tcp doesn't report them, so use the common tuner's).
const R820T_GAINS: [i32; 29] = [0, 9, 14, 27, 37, 77, 87, 125, 144, 157, 166, 197, 207, 229, 254, 280, 297, 328, 338, 364, 372, 386, 402, 421, 434, 439, 445, 480, 496];

pub struct RtlTcpSource {
    /// Control side of the socket (the reader thread has its own clone for reading).
    stream: TcpStream,
    cfg: SourceConfig,
    /// Shared with the reader thread: run flag, and the frequency/rate to stamp on blocks,
    /// updated by `apply` so blocks after a retune carry the new values.
    running: Arc<AtomicBool>,
    freq: Arc<AtomicU64>,
    rate: Arc<AtomicU32>,
    thread: Option<JoinHandle<()>>,
    tuner: u32,
}

impl RtlTcpSource {
    pub fn open(cfg: &SourceConfig) -> Result<Self, String> {
        let addr = (cfg.tcp_host.as_str(), cfg.tcp_port)
            .to_socket_addrs()
            .map_err(|e| format!("rtl_tcp: bad address: {e}"))?
            .next()
            .ok_or("rtl_tcp: bad address")?;
        let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(4)).map_err(|e| format!("rtl_tcp: could not connect to {addr}: {e}"))?;
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut hdr = [0u8; 12];
        stream.read_exact(&mut hdr).map_err(|e| format!("rtl_tcp: no header: {e}"))?;
        if &hdr[0..4] != b"RTL0" {
            return Err("rtl_tcp: the server did not send an RTL0 header".into());
        }
        let tuner = u32::from_be_bytes([hdr[4], hdr[5], hdr[6], hdr[7]]);
        let mut s = Self {
            stream,
            cfg: cfg.clone(),
            running: Arc::new(AtomicBool::new(false)),
            freq: Arc::new(AtomicU64::new(hw_freq(cfg))),
            rate: Arc::new(AtomicU32::new(cfg.sample_rate)),
            thread: None,
            tuner,
        };
        // initial settings: 0x02 rate, 0x09 direct sampling, 0x01 frequency, 0x05 ppm,
        // 0x08 RTL AGC, 0x03 manual gain mode, 0x04 gain, 0x0e bias tee
        s.cmd(0x02, cfg.sample_rate)?;
        s.cmd(0x09, cfg.rtl_direct_sampling)?;
        s.cmd(0x01, hw_freq(cfg) as u32)?;
        s.cmd(0x05, cfg.ppm as u32)?;
        s.cmd(0x08, cfg.rtl_agc as u32)?;
        s.cmd(0x03, cfg.rtl_manual_gain as u32)?;
        if cfg.rtl_manual_gain {
            s.cmd(0x04, cfg.rtl_gain as u32)?;
        }
        s.cmd(0x0e, cfg.rtl_bias_tee as u32)?;
        Ok(s)
    }

    fn cmd(&mut self, c: u8, param: u32) -> Result<(), String> {
        let mut b = [0u8; 5];
        b[0] = c;
        b[1..].copy_from_slice(&param.to_be_bytes());
        self.stream.write_all(&b).map_err(|e| format!("rtl_tcp: {e}"))
    }
}

impl Source for RtlTcpSource {
    fn start(&mut self, tx: Sender<IqBlock>) -> Result<(), String> {
        let mut stream = self.stream.try_clone().map_err(|e| e.to_string())?;
        let running = self.running.clone();
        let freq = self.freq.clone();
        let rate = self.rate.clone();
        running.store(true, Ordering::SeqCst);
        self.thread = Some(
            std::thread::Builder::new()
                .name("rtltcp-rx".into())
                .spawn(move || {
                    while running.load(Ordering::Relaxed) {
                        // ~50 ms of 2-byte samples, rounded to 16 KiB, between 16 and 512 KiB
                        let r = rate.load(Ordering::Relaxed);
                        let len = ((r as usize * 2 / 20) / 16384).clamp(1, 32) * 16384;
                        let mut raw = vec![0u8; len];
                        if stream.read_exact(&mut raw).is_err() {
                            break;
                        }
                        let mut samples = Vec::new();
                        convert_u8(&raw, &mut samples);
                        if tx.send(IqBlock { samples, raw, frequency: freq.load(Ordering::Relaxed), rate: r }).is_err() {
                            break;
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
        // shutting the socket down makes the reader's blocking read_exact fail, so it exits
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    /// Sends only the settings that differ from the last applied config.
    fn apply(&mut self, cfg: &SourceConfig) -> Result<(), String> {
        let old = self.cfg.clone();
        if cfg.sample_rate != old.sample_rate {
            self.cmd(0x02, cfg.sample_rate)?;
            self.rate.store(cfg.sample_rate, Ordering::Relaxed);
        }
        if cfg.rtl_direct_sampling != old.rtl_direct_sampling {
            self.cmd(0x09, cfg.rtl_direct_sampling)?;
        }
        if hw_freq(cfg) != hw_freq(&old) {
            self.cmd(0x01, hw_freq(cfg) as u32)?;
            self.freq.store(hw_freq(cfg), Ordering::Relaxed);
        }
        if cfg.ppm != old.ppm {
            self.cmd(0x05, cfg.ppm as u32)?;
        }
        if cfg.rtl_agc != old.rtl_agc {
            self.cmd(0x08, cfg.rtl_agc as u32)?;
        }
        if cfg.rtl_manual_gain != old.rtl_manual_gain {
            self.cmd(0x03, cfg.rtl_manual_gain as u32)?;
        }
        if cfg.rtl_manual_gain && (cfg.rtl_gain != old.rtl_gain || cfg.rtl_manual_gain != old.rtl_manual_gain) {
            self.cmd(0x04, cfg.rtl_gain as u32)?;
        }
        if cfg.rtl_bias_tee != old.rtl_bias_tee {
            self.cmd(0x0e, cfg.rtl_bias_tee as u32)?;
        }
        self.cfg = cfg.clone();
        Ok(())
    }

    fn raw_format(&self) -> FileFormat {
        FileFormat::Cu8
    }

    fn description(&self) -> String {
        let t = match self.tuner {
            1 => "E4000",
            5 => "R820T",
            6 => "R828D",
            _ => "tuner",
        };
        format!("rtl_tcp {}:{} ({t})", self.cfg.tcp_host, self.cfg.tcp_port)
    }

    fn gains(&self) -> Vec<i32> {
        R820T_GAINS.to_vec()
    }
}

impl Drop for RtlTcpSource {
    fn drop(&mut self) {
        self.stop();
    }
}
