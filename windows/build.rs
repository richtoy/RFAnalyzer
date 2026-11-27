// Builds the DAB/DAB+ decoder (dab-cmdline + faad2 + KISS FFT from ../nativedab, with this fork's
// fixes) into a static library, plus the C API in native/dab_capi.cpp.
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=native/dab_capi.cpp");
    if std::env::var("CARGO_FEATURE_STATIC_SDR").is_ok() {
        build_sdr_libs();
    }
    if std::env::var("CARGO_FEATURE_DAB").is_err() {
        return;
    }
    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("../nativedab/src/main/cpp");
    let root = canon(&root).expect("nativedab sources not found (../nativedab/src/main/cpp)");
    println!("cargo:rerun-if-changed={}", root.display());
    let target = std::env::var("TARGET").unwrap();
    let windows = target.contains("windows");
    let msvc = target.contains("msvc");

    // faad2
    let mut faad = cc::Build::new();
    faad.include(root.join("faad2/include")).include(root.join("faad2/libfaad")).warnings(false).opt_level(3);
    for d in ["HAVE_INTTYPES_H", "HAVE_MEMCPY", "HAVE_STRING_H", "HAVE_SYS_STAT_H", "HAVE_SYS_TYPES_H", "HAVE_LRINTF"] {
        faad.define(d, "1");
    }
    if !msvc {
        faad.define("HAVE_STRINGS_H", "1");
    }
    faad.define("PACKAGE_VERSION", "\"2.11-rfa\"");
    for e in std::fs::read_dir(root.join("faad2/libfaad")).unwrap() {
        let p = e.unwrap().path();
        if p.extension().map(|x| x == "c").unwrap_or(false) {
            faad.file(p);
        }
    }

    // dab-cmdline sources, as listed in nativedab's CMakeLists.txt
    let cmake = std::fs::read_to_string(root.join("CMakeLists.txt")).unwrap();
    let start = cmake.find("set(DAB_SOURCES").unwrap();
    let block = &cmake[start..];
    let block = &block[..block.find(')').unwrap()];
    let sources: Vec<String> = block
        .lines()
        .skip(1)
        .map(|l| l.trim().replace("${DAB}", "dab"))
        .filter(|l| !l.is_empty())
        .collect();

    let includes = [
        "",
        "dab",
        "dab/includes",
        "dab/includes/ofdm",
        "dab/includes/backend",
        "dab/includes/backend/audio",
        "dab/includes/backend/data",
        "dab/includes/backend/data/mot",
        "dab/includes/backend/data/epg",
        "dab/includes/support",
        "dab/includes/support/viterbi-spiral",
        "kissfft",
        "faad2/include",
    ];

    let mut c = cc::Build::new();
    c.warnings(false).opt_level(3);
    let mut cpp = cc::Build::new();
    cpp.cpp(true).warnings(false).opt_level(3).std("c++17");
    for b in [&mut c, &mut cpp] {
        for i in includes {
            b.include(root.join(i));
        }
        if windows {
            b.define("_USE_MATH_DEFINES", None);
        }
    }
    for s in &sources {
        let p = root.join(s);
        if s.ends_with(".c") {
            c.file(&p);
        } else {
            cpp.file(&p);
        }
    }
    cpp.file(Path::new("native/dab_capi.cpp"));
    // static link order: dab (C++) -> its C helpers -> faad2
    cpp.compile("dab_rfa");
    c.compile("dab_rfa_c");
    faad.compile("faad_rfa");

    if windows && !msvc {
        // static C++ runtime and winpthreads so the exe needs no MinGW DLLs
        println!("cargo:rustc-link-arg=-static-libgcc");
        println!("cargo:rustc-link-arg=-static-libstdc++");
        println!("cargo:rustc-link-arg=-Wl,-Bstatic");
        println!("cargo:rustc-link-lib=static=stdc++");
        println!("cargo:rustc-link-lib=static=winpthread");
        println!("cargo:rustc-link-arg=-Wl,-Bdynamic");
    } else if !windows {
        println!("cargo:rustc-link-lib=stdc++");
    }
}

/// librtlsdr (with RTL-SDR Blog V4 support) and libhackrf from the Android app's sources, linked
/// statically against the libusb that libusb1-sys builds (WinUSB backend on Windows).
fn build_sdr_libs() {
    let base = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("..");
    let rtl = canon(&base.join("librtlsdr/src/main/cpp/librtlsdr")).expect("librtlsdr sources");
    let hrf = canon(&base.join("libhackrf/src/main/cpp/libhackrf")).expect("libhackrf sources");
    println!("cargo:rerun-if-changed={}", rtl.display());
    println!("cargo:rerun-if-changed={}", hrf.display());
    let usb_inc = std::env::var("DEP_USB_1.0_INCLUDE").expect("libusb include dir from libusb1-sys");
    let mut b = cc::Build::new();
    b.warnings(false)
        .opt_level(2)
        .include(&usb_inc)
        .include(Path::new("native/compat"))
        .include(&rtl)
        .include(&hrf)
        .define("rtlsdr_STATIC", None);
    for f in ["librtlsdr.c", "tuner_e4k.c", "tuner_fc0012.c", "tuner_fc0013.c", "tuner_fc2580.c", "tuner_r82xx.c"] {
        b.file(rtl.join(f));
    }
    b.file(hrf.join("hackrf.c"));
    b.compile("sdr_rfa");
}

/// canonicalize() without the Windows `\\?\` verbatim prefix, which C compilers cannot open.
fn canon(p: &Path) -> std::io::Result<PathBuf> {
    let c = p.canonicalize()?;
    let stripped = c
        .to_str()
        .and_then(|s| s.strip_prefix(r"\\?\"))
        .filter(|rest| !rest.starts_with("UNC\\"))
        .map(PathBuf::from);
    Ok(stripped.unwrap_or(c))
}
