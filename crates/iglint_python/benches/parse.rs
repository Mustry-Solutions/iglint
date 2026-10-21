//! Parser throughput.
//!
//! usage: cargo bench -p iglint_python --bench parse [-- DIR]
//!
//! Parses every `.py` file under DIR (or `IGLINT_BENCH_DIR`; by default the
//! repository's oracle case files) repeatedly on one thread and reports
//! MB/s for tokenizing and for parsing.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn collect(dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(&path, out);
        } else if path
            .extension()
            .is_some_and(|ext| ext == "py" || ext == "cases")
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            out.push(text);
        }
    }
}

fn measure(name: &str, bytes: usize, mut run: impl FnMut()) {
    // Warm up, then run for about two seconds.
    run();
    let start = Instant::now();
    let mut iterations = 0u32;
    while start.elapsed() < Duration::from_secs(2) {
        run();
        iterations += 1;
    }
    let seconds = start.elapsed().as_secs_f64();
    let megabytes = bytes as f64 * f64::from(iterations) / 1e6;
    println!("{name:10} {:8.1} MB/s", megabytes / seconds);
}

fn main() {
    let dir = std::env::args()
        .skip(1)
        .find(|arg| !arg.starts_with('-'))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("IGLINT_BENCH_DIR").map(PathBuf::from))
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/oracle"));
    let mut sources = Vec::new();
    collect(&dir, &mut sources);
    let bytes: usize = sources.iter().map(String::len).sum();
    println!(
        "{} sources, {:.2} MB from {}",
        sources.len(),
        bytes as f64 / 1e6,
        dir.display()
    );
    measure("tokenize", bytes, || {
        for source in &sources {
            std::hint::black_box(iglint_python::tokenize(source));
        }
    });
    measure("parse", bytes, || {
        for source in &sources {
            std::hint::black_box(iglint_python::parse_module(source));
        }
    });
}
