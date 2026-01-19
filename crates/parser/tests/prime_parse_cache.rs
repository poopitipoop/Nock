use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, io};

use chumsky::Parser;
use hoonc::{build_jam, build_jam_with_primed_parse_cache, is_valid_file_or_dir};
use nockapp::noun::slab::NounSlab;
use parser::native_parser;
use parser::utils::{hoon_to_noun, LineMap};
use rayon::prelude::*;
use walkdir::WalkDir;

const KERNEL_ENTRIES: &[&str] = &[
    "apps/dumbnet/outer.hoon", "apps/wallet/wallet.hoon", "apps/dumbnet/miner.hoon",
    "apps/peek/peek.hoon", "apps/bridge/bridge.hoon",
];

const MARKDOWN_INCLUDE: &[u8] = b"/common/markdown/markdown";

const HOON_DIR: &str = "../../hoon";
const MARKDOWN_HOON: &str = "../../hoon/common/markdown/markdown.hoon";

static DISABLE_METRICS: Once = Once::new();

fn disable_metrics() {
    DISABLE_METRICS.call_once(|| {
        std::env::set_var("NOCKAPP_DISABLE_METRICS", "1");
    });
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

fn markdown_hoon_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(MARKDOWN_HOON);
    Ok(path.canonicalize()?)
}

fn list_hoon_files(root: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let walker = WalkDir::new(root).follow_links(true).into_iter();
    let mut files = Vec::new();
    for entry_result in walker.filter_entry(is_valid_file_or_dir) {
        let entry = entry_result?;
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("hoon") {
            continue;
        }
        files.push(entry.path().canonicalize()?);
    }
    Ok(files)
}

fn parse_all_hoon_files(root: &Path) -> Result<Vec<(PathBuf, String)>, Box<dyn std::error::Error>> {
    let files = list_hoon_files(root)?;
    let failures: Vec<(PathBuf, String)> = files
        .par_iter()
        .filter_map(|path| match parse_native_ast_err(path) {
            Ok(_) => None,
            Err(err) => Some((path.clone(), err)),
        })
        .collect();
    Ok(failures)
}

fn ensure_entry_ast(
    asts: &mut HashMap<PathBuf, Vec<u8>>,
    entry: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let entry_path = entry.canonicalize()?;
    if asts.contains_key(&entry_path) {
        return Ok(());
    }

    let jammed = parse_native_ast(entry)?;
    asts.insert(entry_path, jammed);
    Ok(())
}

fn parse_native_ast(path: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let source = fs::read_to_string(path)?;
    let linemap = Arc::new(LineMap::new(&source));
    let wer = path
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect();
    let parsed = native_parser(wer, false, linemap)
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

fn parse_native_ast_err(path: &Path) -> Result<Vec<u8>, String> {
    parse_native_ast(path).map_err(|err| err.to_string())
}

fn collect_native_asts(
    deps_dir: &Path,
    entry: &Path,
) -> Result<HashMap<PathBuf, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut asts = HashMap::new();

    let walker = WalkDir::new(deps_dir).follow_links(true).into_iter();
    for entry_result in walker.filter_entry(is_valid_file_or_dir) {
        let entry = entry_result?;
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension().and_then(|ext| ext.to_str()) != Some("hoon") {
            continue;
        }

        let jammed = match parse_native_ast(entry.path()) {
            Ok(jammed) => jammed,
            Err(_) => continue,
        };
        asts.insert(entry.path().canonicalize()?, jammed);
    }

    ensure_entry_ast(&mut asts, entry)?;

    Ok(asts)
}

fn kernel_app_root(entry: &Path, deps_dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let rel = entry.strip_prefix(deps_dir)?;
    let mut components = rel.iter();
    let root = components.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "kernel entry has no path components",
        )
    })?;
    if root != OsStr::new("apps") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("kernel entry is not under apps/: {}", entry.display()),
        )
        .into());
    }
    let app = components.next().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("kernel entry missing app directory: {}", entry.display()),
        )
    })?;

    Ok(deps_dir.join("apps").join(app))
}

fn kernel_includes_markdown(
    entry: &Path,
    deps_dir: &Path,
) -> Result<bool, Box<dyn std::error::Error>> {
    let app_root = kernel_app_root(entry, deps_dir)?;
    let walker = WalkDir::new(&app_root).follow_links(true).into_iter();
    for entry_result in walker {
        let entry = entry_result?;
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().extension() != Some(OsStr::new("hoon")) {
            continue;
        }
        let contents = fs::read(entry.path())?;
        if contents
            .windows(MARKDOWN_INCLUDE.len())
            .any(|chunk| chunk == MARKDOWN_INCLUDE)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn kernel_entries(deps_dir: &Path) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    KERNEL_ENTRIES
        .iter()
        .map(|entry| Ok(deps_dir.join(entry).canonicalize()?))
        .collect()
}

#[tokio::test]
async fn primed_parse_cache_matches_regular_build() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let nockapp_home = temp_out_dir("nockapp-home")?;
    let _env_guard = EnvVarGuard::set("NOCKAPP_HOME", nockapp_home.to_string_lossy().as_ref());

    let entry = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hoonc/hoon/hoon-138.hoon");
    let deps_dir = repo_hoon_dir()?;
    let entry = entry.canonicalize()?;

    let native_asts = collect_native_asts(&deps_dir, &entry)?;
    let regular_out_dir = temp_out_dir("hoonc-regular")?;
    let primed_out_dir = temp_out_dir("hoonc-primed")?;

    let regular_jam = build_jam(
        &entry,
        deps_dir.clone(),
        Some(regular_out_dir.clone()),
        true,
        true,
    )
    .await
    .map_err(|err| {
        io::Error::new(
            io::ErrorKind::Other,
            format!(
                "regular build failed (out_dir: {}): {err}",
                regular_out_dir.display()
            ),
        )
    })?;
    let primed_jam = build_jam_with_primed_parse_cache(
        entry,
        deps_dir,
        Some(primed_out_dir.clone()),
        true,
        true,
        &native_asts,
    )
    .await
    .map_err(|err| {
        io::Error::new(
            io::ErrorKind::Other,
            format!(
                "primed build failed (out_dir: {}): {err}",
                primed_out_dir.display()
            ),
        )
    })?;

    assert_eq!(regular_jam, primed_jam);
    Ok(())
}

#[test]
fn native_parser_parses_markdown_hoon() -> Result<(), Box<dyn std::error::Error>> {
    let path = markdown_hoon_path()?;
    parse_native_ast(&path)?;
    Ok(())
}

#[test]
fn native_parser_parses_all_hoon_files() -> Result<(), Box<dyn std::error::Error>> {
    let root = repo_hoon_dir()?;
    let failures = parse_all_hoon_files(&root)?;

    if failures.is_empty() {
        return Ok(());
    }

    let mut report = String::new();
    report.push_str("native parser failures:\n");
    for (path, error) in failures {
        report.push_str(&format!("- {}\n    {error}\n", path.display()));
    }
    Err(io::Error::new(io::ErrorKind::Other, report).into())
}

#[tokio::test]
async fn primed_parse_cache_matches_regular_build_for_kernels(
) -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let deps_dir = repo_hoon_dir()?;
    let entries = kernel_entries(&deps_dir)?;
    let first_entry = entries
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no kernel entries defined"))?;
    let mut native_asts = collect_native_asts(&deps_dir, first_entry)?;
    let mut tested = Vec::new();

    for entry in entries {
        ensure_entry_ast(&mut native_asts, &entry)?;

        let regular_home = temp_out_dir("nockapp-home-regular")?;
        let primed_home = temp_out_dir("nockapp-home-primed")?;

        let regular_out_dir = temp_out_dir("hoonc-regular")?;
        let primed_out_dir = temp_out_dir("hoonc-primed")?;

        println!("testing kernel: {}", entry.display());

        let regular_jam = {
            let _env_guard =
                EnvVarGuard::set("NOCKAPP_HOME", regular_home.to_string_lossy().as_ref());
            build_jam(
                &entry,
                deps_dir.clone(),
                Some(regular_out_dir.clone()),
                false,
                false,
            )
            .await
            .map_err(|err| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "kernel build failed (entry: {}, out_dir: {}): {err}",
                        entry.display(),
                        regular_out_dir.display()
                    ),
                )
            })?
        };

        let primed_jam = {
            let _env_guard =
                EnvVarGuard::set("NOCKAPP_HOME", primed_home.to_string_lossy().as_ref());
            build_jam_with_primed_parse_cache(
                entry.clone(),
                deps_dir.clone(),
                Some(primed_out_dir.clone()),
                false,
                false,
                &native_asts,
            )
            .await
            .map_err(|err| {
                io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "kernel primed build failed (entry: {}, out_dir: {}): {err}",
                        entry.display(),
                        primed_out_dir.display()
                    ),
                )
            })?
        };

        assert_eq!(
            regular_jam,
            primed_jam,
            "kernel jam mismatch for {}",
            entry.display()
        );
        tested.push(entry);
    }

    assert!(
        !tested.is_empty(),
        "no kernel entries eligible for primed parse-cache test"
    );
    Ok(())
}
