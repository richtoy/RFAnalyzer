# RF Analyzer for Windows (x64)

A desktop port of this RFAnalyzer fork, written in Rust (egui). Same feature set as the Android fork:
spectrum + waterfall, AM / NFM / WFM / LSB / USB / CW demodulation, squelch, raw IQ recording and
playback, FM stereo + RDS (with a full details window), and DAB/DAB+ with scanning, slideshow, DLS
and technical screens (services, ensemble/CU map, signal/constellation, now playing).

## Running

`rfanalyzer.exe` is self-contained (no extra DLLs). Settings live in `%APPDATA%\RFAnalyzer`,
recordings go to `%USERPROFILE%\RFAnalyzer recordings` by default.

### SDR drivers
- **RTL-SDR (incl. Blog V4)**: install the WinUSB driver once with [Zadig](https://zadig.akeo.ie/)
  (Options > List all devices, pick "Bulk-In, Interface (Interface 0)", WinUSB, Replace driver).
- **HackRF**: works with the standard WinUSB driver the HackRF installer / Zadig provides.
- **rtl_tcp**: any rtl_tcp server, host:port in the Source tab.
- **File**: cu8 / cs8 / cs16 / cf32 IQ files; frequency and rate are read from names like
  `..._97000000Hz_2304000sps.cu8`.

Good DAB settings: RTL-SDR AGC off, gain 30-40 dB; HackRF amp on, LNA 24, VGA 40.

### Command line
`rfanalyzer.exe [file] [--file F] [--mode am|nfm|wfm|lsb|usb|cw|dab] [--freq 97.3M] [--dab-channel 12C] [--start]`

Testing helpers: `--fm-scan` (scan the FM band into the presets, log in `%APPDATA%\RFAnalyzer\fm_scan_log.txt`), `--scan N [--scan-lna-step D]` (DAB scans, log in `dab_scan_log.txt`), `--quit-after-scan`.

Shortcuts: F5/Space start/stop, arrows step, PgUp/PgDn big steps, M mute.

## Building

Cross-compiled from Linux (mingw-w64, posix threads):

    rustup target add x86_64-pc-windows-gnu
    apt install mingw-w64
    cargo build --release --target x86_64-pc-windows-gnu

build.rs compiles the DAB decoder (`../nativedab`), librtlsdr and libhackrf (from this repo) and
vendored libusb statically. Features: `dab`, `static-sdr` (both default).

Native build on Windows: MSVC does not work (libhackrf needs pthreads), use the GNU toolchain with
WinLibs MinGW-w64 (POSIX/UCRT): `rustup target add x86_64-pc-windows-gnu`, put WinLibs `bin` first
on PATH, set `CC_x86_64_pc_windows_gnu=gcc`, `CXX_x86_64_pc_windows_gnu=g++`,
`AR_x86_64_pc_windows_gnu=ar`, `CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=gcc`, then
`cargo build --release --target x86_64-pc-windows-gnu`. `cargo test` runs the DSP/RDS/DAB-model unit tests.
