//! Benchmark comparing kernel build times with and without the native parser.
//!
//! Run with: cargo bench -p parser --bench kernel_build
//!
//! This benchmark measures end-to-end build times for the dumbnet outer kernel
//! using both the standard Hoon parser and the native Rust parser for parse cache priming.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use std::{fs, io};

use chumsky::Parser;
use hoonc::{build_jam, is_valid_file_or_dir};
use nockapp::noun::slab::NounSlab;
use parser::native_parser;
use parser::utils::{hoon_to_noun, LineMap};
use tokio::runtime::Runtime;
use walkdir::WalkDir;

const DUMBNET_OUTER: &str = "apps/dumbnet/outer.hoon";
const HOON_DIR: &str = "../../hoon";

static DISABLE_METRICS: Once = Once::new();
static LOG_CAPTURE: OnceLock<LogCapture> = OnceLock::new();

struct BenchFlags {
    native_only: bool,
}

impl BenchFlags {
    fn from_env() -> Self {
        let mut native_only = false;
        for arg in std::env::args().skip(1) {
            if arg == "--native-only" || arg == "--only-native" {
                native_only = true;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--native-only=") {
                native_only = parse_bool_flag(value).unwrap_or(true);
            }
        }
        Self { native_only }
    }
}

fn parse_bool_flag(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn disable_metrics() {
    DISABLE_METRICS.call_once(|| {
        std::env::set_var("NOCKAPP_DISABLE_METRICS", "1");
    });
}

struct LogCapture {
    buffer: Arc<Mutex<Vec<u8>>>,
}

impl LogCapture {
    fn clear(&self) {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.clear();
        }
    }

    fn take_string(&self) -> String {
        let mut buffer = self.buffer.lock().unwrap_or_else(|err| err.into_inner());
        let output = String::from_utf8_lossy(&buffer).to_string();
        buffer.clear();
        output
    }
}

struct TeeWriter {
    buffer: Arc<Mutex<Vec<u8>>>,
    tee_stdout: bool,
}

impl Write for TeeWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Ok(mut buffer) = self.buffer.lock() {
            buffer.extend_from_slice(buf);
        }
        if self.tee_stdout {
            let mut stdout = io::stdout().lock();
            stdout.write_all(buf)?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.tee_stdout {
            io::stdout().lock().flush()?;
        }
        Ok(())
    }
}

fn init_logging(tee_stdout: bool) -> &'static LogCapture {
    LOG_CAPTURE.get_or_init(|| {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let writer_buffer = Arc::clone(&buffer);
        let make_writer = move || TeeWriter {
            buffer: Arc::clone(&writer_buffer),
            tee_stdout,
        };
        if let Err(err) = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_target(false)
            .with_writer(make_writer)
            .try_init()
        {
            eprintln!("bench: failed to install tracing subscriber: {err}");
        }
        LogCapture { buffer }
    })
}

struct EnvVarGuard {
    key: &'static str,
    previous: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let previous = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, previous }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.previous {
            Some(value) => std::env::set_var(self.key, value),
            None => std::env::remove_var(self.key),
        }
    }
}

fn temp_out_dir(prefix: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let mut path = std::env::temp_dir();
    path.push(format!("{prefix}-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&path)?;
    Ok(path)
}

fn repo_hoon_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(HOON_DIR);
    Ok(path.canonicalize()?)
}

fn hoon_path_for_file(
    path: &Path,
    deps_dir: &Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let rel = path.strip_prefix(deps_dir).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "native parser path is not under hoon dir: {}",
                path.display()
            ),
        )
    })?;
    Ok(rel
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect())
}

fn hoon_path_for_any(path: &Path, deps_dir: &Path) -> Vec<String> {
    match hoon_path_for_file(path, deps_dir) {
        Ok(segments) => segments,
        Err(_) => hoon_path_for_absolute(path),
    }
}

fn hoon_path_for_absolute(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(seg) => Some(seg.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn parse_native_ast_with_wer(
    path: &Path,
    wer: Vec<String>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let source = fs::read_to_string(path)?;
    let linemap = Arc::new(LineMap::new(&source));
    let parsed = native_parser(wer, true, linemap)
        .parse(source.as_str())
        .into_result()
        .map_err(|errs| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("native parser failed for {}: {:?}", path.display(), errs),
            )
        })?;

    let mut slab = NounSlab::new();
    let noun = hoon_to_noun(&mut slab, &parsed);
    slab.set_root(noun);
    Ok(slab.jam().to_vec())
}

fn parse_native_ast(path: &Path, deps_dir: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let wer = hoon_path_for_any(path, deps_dir);
    parse_native_ast_with_wer(path, wer)
}

fn ensure_entry_ast(
    asts: &mut HashMap<PathBuf, Vec<u8>>,
    entry: &Path,
    deps_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let entry_path = entry.canonicalize()?;
    if asts.contains_key(&entry_path) {
        return Ok(());
    }

    let jammed = parse_native_ast(entry, deps_dir)?;
    asts.insert(entry_path, jammed);
    Ok(())
}

fn collect_native_asts(
    deps_dir: &Path,
    entry: &Path,
) -> Result<HashMap<PathBuf, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut asts = HashMap::new();
    let entry_path = entry.canonicalize()?;

    let walker = WalkDir::new(deps_dir).follow_links(true).into_iter();
    for entry_result in walker.filter_entry(is_valid_file_or_dir) {
        let entry = entry_result?;
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("hoon") {
            continue;
        }

        let canonical = entry.path().canonicalize()?;
        if canonical == entry_path {
            continue;
        }
        let jammed = match parse_native_ast(entry.path(), deps_dir) {
            Ok(jammed) => jammed,
            Err(_) => continue,
        };
        asts.insert(canonical, jammed);
    }

    ensure_entry_ast(&mut asts, entry, deps_dir)?;

    Ok(asts)
}

struct PrimeAttempt {
    warned: bool,
    failed: bool,
    pc_size: Option<usize>,
}

fn parse_pc_size(logs: &str) -> Option<usize> {
    let marker = "prime-dir: pc-size ";
    logs.lines().find_map(|line| {
        let idx = line.find(marker)?;
        let rest = &line[idx + marker.len()..];
        let number = rest.split_whitespace().next()?.trim_matches('"');
        number.parse().ok()
    })
}

fn detect_prime_failure(logs: &str) -> bool {
    let markers = [
        "hoonc: warning: input is not a proper cause", "prime-native: hoon mold failed",
        "prime-dir: hoon mismatch", "syntax error", "hoonc: missing dependency",
    ];
    markers.iter().any(|marker| logs.contains(marker))
}

fn native_subset_for_prefix(
    entry: &PathBuf,
    native_asts: &HashMap<PathBuf, Vec<u8>>,
    sorted_paths: &[PathBuf],
    prefix_len: usize,
) -> Result<HashMap<PathBuf, Vec<u8>>, Box<dyn std::error::Error>> {
    let entry_path = entry.canonicalize()?;
    let entry_jam = native_asts.get(&entry_path).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("native AST missing for entry {}", entry.display()),
        )
    })?;

    let mut subset = HashMap::new();
    subset.insert(entry_path, entry_jam.clone());
    for path in sorted_paths.iter().take(prefix_len) {
        if let Some(jam) = native_asts.get(path) {
            subset.insert(path.clone(), jam.clone());
        }
    }
    Ok(subset)
}

fn prime_with_subset(
    rt: &Runtime,
    entry: &PathBuf,
    deps_dir: &PathBuf,
    subset: &HashMap<PathBuf, Vec<u8>>,
    log_capture: &LogCapture,
) -> Result<PrimeAttempt, Box<dyn std::error::Error>> {
    let nockapp_home = temp_out_dir("bench-bisect-home")?;
    let out_dir = temp_out_dir("bench-bisect-out")?;
    let _env_guard = EnvVarGuard::set("NOCKAPP_HOME", nockapp_home.to_string_lossy().as_ref());
    let _prewarm_guard = EnvVarGuard::set("HOONC_DISABLE_PREWARM", "1");

    log_capture.clear();
    let build_result = rt.block_on(async {
        hoonc::build_jam_with_primed_parse_cache(
            entry.clone(),
            deps_dir.clone(),
            Some(out_dir),
            false,
            true,
            subset,
        )
        .await
    });

    let logs = log_capture.take_string();
    let warned = logs.contains("hoonc: warning: input is not a proper cause");
    let failed = build_result.is_err() || detect_prime_failure(&logs);
    let pc_size = parse_pc_size(&logs);
    if let Err(err) = build_result {
        if !logs.is_empty() {
            println!("bisect logs:\n{logs}");
        }
        println!("bisect build error: {err}");
    }
    Ok(PrimeAttempt {
        warned,
        failed,
        pc_size,
    })
}

fn bisect_first_failing_path(
    rt: &Runtime,
    entry: &PathBuf,
    deps_dir: &PathBuf,
    native_asts: &HashMap<PathBuf, Vec<u8>>,
    log_capture: &LogCapture,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let entry_path = entry.canonicalize()?;
    let mut paths: Vec<PathBuf> = native_asts
        .keys()
        .filter(|path| **path != entry_path)
        .cloned()
        .collect();
    paths.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

    let full_attempt = {
        let subset = native_subset_for_prefix(entry, native_asts, &paths, paths.len())?;
        prime_with_subset(rt, entry, deps_dir, &subset, log_capture)?
    };
    println!(
        "bisect: full set warned={} failed={} pc-size={:?}",
        full_attempt.warned, full_attempt.failed, full_attempt.pc_size
    );
    if !full_attempt.failed {
        return Ok(None);
    }

    let mut low = 0usize;
    let mut high = paths.len();
    while low < high {
        let mid = (low + high) / 2;
        let subset = native_subset_for_prefix(entry, native_asts, &paths, mid)?;
        let attempt = prime_with_subset(rt, entry, deps_dir, &subset, log_capture)?;
        println!(
            "bisect: prefix={} warned={} failed={} pc-size={:?}",
            mid, attempt.warned, attempt.failed, attempt.pc_size
        );
        if attempt.failed {
            high = mid;
        } else {
            low = mid + 1;
        }
    }

    let failing_path = if low == 0 {
        entry_path
    } else {
        paths[low - 1].clone()
    };
    Ok(Some(failing_path))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let flags = BenchFlags::from_env();

    std::env::set_var("RUST_LOG", "info");
    // Enable logs so we can see cache hits/misses from hoonc.
    // The Hoon ~& prints will show "parsing <path>" or "reusing parse cache entry for <path>".
    let log_capture = init_logging(true);

    let deps_dir = repo_hoon_dir()?;
    let entry = deps_dir.join(DUMBNET_OUTER).canonicalize()?;

    println!("=== Dumbnet Kernel Build Benchmark ===\n");
    println!("Entry: {}\n", entry.display());
    if flags.native_only {
        println!("Mode: native-only (--native-only)\n");
    }

    let rt = Runtime::new()?;

    // Collect native ASTs
    println!("Collecting native ASTs for primed builds...");
    let ast_start = Instant::now();
    let native_asts = collect_native_asts(&deps_dir, &entry)?;
    let ast_duration = ast_start.elapsed();
    println!(
        "Collected {} native ASTs in {:.2}s\n",
        native_asts.len(),
        ast_duration.as_secs_f64()
    );

    if std::env::var("HOONC_PRIME_BISECT").is_ok() {
        println!("Bisecting native ASTs to find first failing path...");
        match bisect_first_failing_path(&rt, &entry, &deps_dir, &native_asts, log_capture) {
            Ok(Some(path)) => {
                println!("First failing path: {}\n", path.display());
            }
            Ok(None) => {
                println!("No failing path detected in native ASTs.\n");
            }
            Err(err) => {
                println!("Bisect failed: {err}\n");
            }
        }
        log_capture.clear();
    }

    let hoon_duration = if flags.native_only {
        None
    } else {
        // Benchmark Hoon parser build
        println!("Running Hoon parser build (new=true)...");
        let nockapp_home = temp_out_dir("bench-hoon-home")?;
        let out_dir = temp_out_dir("bench-hoon-out")?;
        let _env_guard = EnvVarGuard::set("NOCKAPP_HOME", nockapp_home.to_string_lossy().as_ref());

        let start = Instant::now();
        rt.block_on(async {
            build_jam(
                &entry,
                deps_dir.clone(),
                Some(out_dir),
                false, // arbitrary
                true,  // new
            )
            .await?;
            Ok::<(), Box<dyn std::error::Error>>(())
        })?;
        let duration = start.elapsed();
        println!("Hoon parser build: {:.2}s\n", duration.as_secs_f64());
        Some(duration)
    };

    // Benchmark native parser build with timing breakdown
    println!("Running native parser build (new=true)...");
    let (prime_duration, native_build_duration) = {
        let nockapp_home = temp_out_dir("bench-native-home")?;
        let out_dir = temp_out_dir("bench-native-out")?;
        let _env_guard = EnvVarGuard::set("NOCKAPP_HOME", nockapp_home.to_string_lossy().as_ref());

        rt.block_on(async {
            use hoonc::{initialize_with_default_cli, run_build};

            // Initialize nockapp
            let (mut nockapp, out_path) = initialize_with_default_cli(
                entry.clone(),
                deps_dir.clone(),
                Some(out_dir),
                false, // arbitrary
                true,  // new
            )
            .await?;

            // Time just the prime poke
            let prime_start = Instant::now();
            hoonc::prime_parse_cache_public(&mut nockapp, &entry, &deps_dir, &native_asts).await?;
            let prime_dur = prime_start.elapsed();

            // Time just the build
            let build_start = Instant::now();
            run_build(nockapp, Some(out_path)).await?;
            let build_dur = build_start.elapsed();

            Ok::<_, Box<dyn std::error::Error>>((prime_dur, build_dur))
        })?
    };
    let native_duration = prime_duration + native_build_duration;
    println!("  Prime poke: {:.2}s", prime_duration.as_secs_f64());
    println!("  Build: {:.2}s", native_build_duration.as_secs_f64());
    println!(
        "Native parser total: {:.2}s\n",
        native_duration.as_secs_f64()
    );

    // Summary
    println!("=== Results ===\n");
    println!(
        "Native AST collection:  {:>8.2}s",
        ast_duration.as_secs_f64()
    );
    if let Some(hoon_duration) = hoon_duration {
        println!(
            "Hoon parser build:      {:>8.2}s",
            hoon_duration.as_secs_f64()
        );
    } else {
        println!("Hoon parser build:      skipped");
    }
    println!(
        "Native parser build:    {:>8.2}s",
        native_duration.as_secs_f64()
    );
    println!(
        "Native + AST collection:{:>8.2}s",
        native_duration.as_secs_f64() + ast_duration.as_secs_f64()
    );

    if let Some(hoon_duration) = hoon_duration {
        let diff = hoon_duration.as_secs_f64() - native_duration.as_secs_f64();
        let pct = (diff / hoon_duration.as_secs_f64()) * 100.0;
        println!(
            "\nDifference: {:.2}s ({:.1}% {})",
            diff.abs(),
            pct.abs(),
            if diff > 0.0 { "faster" } else { "slower" }
        );

        let total_diff = hoon_duration.as_secs_f64()
            - (native_duration.as_secs_f64() + ast_duration.as_secs_f64());
        let total_pct = (total_diff / hoon_duration.as_secs_f64()) * 100.0;
        println!(
            "With AST collection: {:.2}s ({:.1}% {})",
            total_diff.abs(),
            total_pct.abs(),
            if total_diff > 0.0 { "faster" } else { "slower" }
        );
    }

    Ok(())
}
