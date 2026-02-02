use std::collections::{BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, Once, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use std::{fs, io};

use bytes::Bytes;
use chumsky::Parser;
use either::Either;
use hoonc::{
    build_jam, build_jam_with_primed_parse_cache, initialize_with_default_cli,
    is_valid_file_or_dir, prime_parse_cache_public,
};
use nockapp::noun::slab::{slab_noun_equality, NockJammer, NounSlab};
use nockapp::one_punch::OnePunchWire;
use nockapp::save::JammedCheckpoint;
use nockapp::wire::Wire;
use nockapp::AtomExt;
use nockvm::noun::{Atom, Noun, D, T};
use nockvm_macros::tas;
use parser::ast::hoon as ast;
use parser::native_parser;
use parser::utils::{diff_noun, hoon_to_noun, print_noun, LineMap};
use rayon::prelude::*;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use walkdir::WalkDir;

const KERNEL_ENTRIES: &[&str] = &[
    "apps/dumbnet/outer.hoon", "apps/wallet/wallet.hoon", "apps/dumbnet/miner.hoon",
    "apps/peek/peek.hoon", "apps/bridge/bridge.hoon",
];

const HOON_DIR: &str = "../../hoon";
const MARKDOWN_HOON: &str = "../../hoon/common/markdown/markdown.hoon";

static DISABLE_METRICS: Once = Once::new();
static LOG_CAPTURE: OnceLock<LogCapture> = OnceLock::new();
static TEST_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn disable_metrics() {
    DISABLE_METRICS.call_once(|| {
        std::env::set_var("NOCKAPP_DISABLE_METRICS", "1");
    });
}

async fn test_permit() -> OwnedSemaphorePermit {
    let semaphore = TEST_SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(1)))
        .clone();
    semaphore
        .acquire_owned()
        .await
        .expect("test semaphore closed")
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
            eprintln!("test: failed to install tracing subscriber: {err}");
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
        .filter_map(|path| match parse_native_ast_err(path, root) {
            Ok(_) => None,
            Err(err) => Some((path.clone(), err)),
        })
        .collect();
    Ok(failures)
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

fn parse_native_ast_with_wer_and_dbug(
    path: &Path,
    wer: Vec<String>,
    dbug: bool,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let source = fs::read_to_string(path)?;
    let linemap = Arc::new(LineMap::new(&source));
    let parsed = native_parser(wer, dbug, linemap)
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

fn parse_native_hoon_with_wer_and_dbug(
    path: &Path,
    wer: Vec<String>,
    dbug: bool,
) -> Result<ast::Hoon, Box<dyn std::error::Error>> {
    let source = fs::read_to_string(path)?;
    let linemap = Arc::new(LineMap::new(&source));
    let parsed = native_parser(wer, dbug, linemap)
        .parse(source.as_str())
        .into_result()
        .map_err(|errs| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("native parser failed for {}: {:?}", path.display(), errs),
            )
        })?;
    Ok(parsed)
}

fn parse_native_hoon_with_dbug(
    path: &Path,
    deps_dir: &Path,
    dbug: bool,
) -> Result<ast::Hoon, Box<dyn std::error::Error>> {
    let wer = hoon_path_for_any(path, deps_dir);
    parse_native_hoon_with_wer_and_dbug(path, wer, dbug)
}

fn parse_native_ast_with_wer(
    path: &Path,
    wer: Vec<String>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    parse_native_ast_with_wer_and_dbug(path, wer, true)
}

fn parse_native_ast(path: &Path, deps_dir: &Path) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let wer = hoon_path_for_any(path, deps_dir);
    parse_native_ast_with_wer(path, wer)
}

fn parse_native_ast_with_dbug(
    path: &Path,
    deps_dir: &Path,
    dbug: bool,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let wer = hoon_path_for_any(path, deps_dir);
    parse_native_ast_with_wer_and_dbug(path, wer, dbug)
}

fn parse_native_ast_err(path: &Path, deps_dir: &Path) -> Result<Vec<u8>, String> {
    parse_native_ast(path, deps_dir).map_err(|err| err.to_string())
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

fn collect_native_asts_for_paths(
    deps_dir: &Path,
    paths: &[PathBuf],
) -> Result<HashMap<PathBuf, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut asts = HashMap::new();
    for path in paths {
        let canonical = path.canonicalize()?;
        if asts.contains_key(&canonical) {
            continue;
        }
        let jammed = parse_native_ast(&canonical, deps_dir)?;
        asts.insert(canonical, jammed);
    }
    Ok(asts)
}

fn collect_native_asts_for_paths_with_dbug(
    deps_dir: &Path,
    paths: &[PathBuf],
    dbug: bool,
) -> Result<HashMap<PathBuf, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut asts = HashMap::new();
    for path in paths {
        let canonical = path.canonicalize()?;
        if asts.contains_key(&canonical) {
            continue;
        }
        let jammed = parse_native_ast_with_dbug(&canonical, deps_dir, dbug)?;
        asts.insert(canonical, jammed);
    }
    Ok(asts)
}

fn entry_path_for_hoon(
    entry: &Path,
    deps_dir: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    let entry_abs = entry.canonicalize()?;
    let deps_abs = deps_dir.canonicalize()?;
    if let Ok(rel) = entry_abs.strip_prefix(&deps_abs) {
        let rel_str = rel.to_string_lossy();
        if rel_str.starts_with('/') {
            Ok(rel_str.to_string())
        } else {
            Ok(format!("/{rel_str}"))
        }
    } else {
        Ok(entry_abs.to_string_lossy().into_owned())
    }
}

fn build_directory_noun_for_parse(
    slab: &mut NounSlab,
    deps_dir: &Path,
) -> Result<Noun, Box<dyn std::error::Error>> {
    let directory = deps_dir.canonicalize()?;
    let directory_str = directory.to_string_lossy();
    let mut directory_noun = D(0);
    let walker = WalkDir::new(&directory).follow_links(true).into_iter();

    for entry_result in walker.filter_entry(is_valid_file_or_dir) {
        let entry = entry_result?;
        if !entry.metadata()?.is_file() {
            continue;
        }

        let path_str = entry
            .path()
            .to_str()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "dependency path contains invalid UTF-8",
                )
            })?
            .strip_prefix(directory_str.as_ref())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "dependency path does not share base prefix",
                )
            })?;

        let path_str = if path_str.starts_with('/') {
            path_str.to_string()
        } else {
            format!("/{path_str}")
        };

        let path_cord = Atom::from_value(slab, path_str)?.as_noun();
        let contents = fs::read(entry.path())?;
        let contents = Atom::from_value(slab, contents)?.as_noun();

        let entry_cell = T(slab, &[path_cord, contents]);
        directory_noun = T(slab, &[entry_cell, directory_noun]);
    }

    Ok(directory_noun)
}

fn expect_cell(noun: Noun, context: &str) -> Result<(Noun, Noun), String> {
    let cell = noun
        .as_cell()
        .map_err(|_| format!("{context} is not a cell"))?;
    Ok((cell.head(), cell.tail()))
}

fn path_noun_to_string(path: Noun) -> Result<String, String> {
    let mut parts = Vec::new();
    let mut cursor = path;
    loop {
        if noun_is_zero(&cursor) {
            break;
        }
        let (head, tail) = expect_cell(cursor, "path list")?;
        let atom = head
            .as_atom()
            .map_err(|_| "path element is not atom".to_string())?;
        let text = atom
            .into_string()
            .map_err(|_| "path element not valid cord".to_string())?;
        parts.push(text);
        cursor = tail;
    }
    Ok(format!("/{}", parts.join("/")))
}

fn tuple3(noun: Noun, context: &str) -> Result<(Noun, Noun, Noun), String> {
    let (a, rest) = expect_cell(noun, context)?;
    let (b, c) = expect_cell(rest, context)?;
    Ok((a, b, c))
}

fn pile_hoon(pil: Noun) -> Result<Noun, String> {
    let (_, rest) = expect_cell(pil, "pile")?;
    let (_, rest) = expect_cell(rest, "pile")?;
    let (_, rest) = expect_cell(rest, "pile")?;
    let (_, rest) = expect_cell(rest, "pile")?;
    let (_, hoon) = expect_cell(rest, "pile")?;
    Ok(hoon)
}

fn map_find_entry_by_path(map: Noun, target: &str) -> Result<Option<(Noun, Noun, Noun)>, String> {
    if noun_is_zero(&map) {
        return Ok(None);
    }
    let (node, rest) = expect_cell(map, "map node")?;
    let (left, right) = expect_cell(rest, "map children")?;
    let (key, val) = expect_cell(node, "map key/value")?;
    let (path, pil, deps) = tuple3(val, "map value")?;
    let path_string = path_noun_to_string(path)?;
    if path_string == target {
        return Ok(Some((key, pil, deps)));
    }
    if let Some(found) = map_find_entry_by_path(left, target)? {
        return Ok(Some(found));
    }
    map_find_entry_by_path(right, target)
}

struct ParseCacheEntry {
    path: String,
    pil: Noun,
}

fn collect_parse_cache_entries(map: Noun) -> Result<Vec<ParseCacheEntry>, String> {
    let mut entries = Vec::new();
    let mut stack = vec![map];

    while let Some(node) = stack.pop() {
        if noun_is_zero(&node) {
            continue;
        }
        let (node, rest) = expect_cell(node, "map node")?;
        let (left, right) = expect_cell(rest, "map children")?;
        let (key, val) = expect_cell(node, "map key/value")?;
        let (path, pil, _deps) = tuple3(val, "map value")?;
        let path_string = path_noun_to_string(path)?;
        let _ = key;
        entries.push(ParseCacheEntry {
            path: path_string,
            pil,
        });
        stack.push(left);
        stack.push(right);
    }

    Ok(entries)
}

fn resolve_parse_cache_path(path: &str, deps_dir: &Path) -> Option<PathBuf> {
    if path.is_empty() {
        return None;
    }

    let candidate = PathBuf::from(path);
    if candidate.is_absolute() && candidate.exists() {
        return Some(candidate);
    }

    let rel = path.trim_start_matches('/');
    let joined = deps_dir.join(rel);
    if joined.exists() {
        return Some(joined);
    }

    None
}

fn is_hoon_path(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("hoon")
}

fn is_state3_tag(noun: &Noun) -> bool {
    let Ok(atom) = noun.as_atom() else {
        return false;
    };
    if atom.as_u64().ok() == Some(3) {
        return true;
    }
    atom.into_string().ok().as_deref() == Some("3")
}

fn parse_cache_from_state(state: Noun) -> Result<Noun, String> {
    let mut stack = vec![state];
    let mut seen = HashSet::new();

    while let Some(noun) = stack.pop() {
        let raw = unsafe { noun.as_raw() };
        if !seen.insert(raw) {
            continue;
        }
        let Ok(cell) = noun.as_cell() else {
            continue;
        };
        if is_state3_tag(&cell.head()) {
            if let Ok((_tag, rest)) = expect_cell(noun, "state root") {
                if let Ok((_cached, rest)) = expect_cell(rest, "state cached") {
                    if let Ok((_bc, pc)) = expect_cell(rest, "state cache fields") {
                        return Ok(pc);
                    }
                }
            }
        }
        stack.push(cell.head());
        stack.push(cell.tail());
    }

    Err("state-3 parse cache not found in kernel state".to_string())
}

fn skip_dbug(mut noun: Noun) -> Noun {
    loop {
        let cell = match noun.cell() {
            Some(c) => c,
            None => return noun,
        };

        let head = match cell.head().as_atom() {
            Ok(a) => a,
            Err(_) => return noun,
        };

        if unsafe { !head.as_noun().raw_equals(&D(tas!(b"dbug"))) } {
            return noun;
        }

        let tail_cell = match cell.tail().as_cell() {
            Ok(c) => c,
            Err(_) => return noun,
        };

        noun = tail_cell.tail();
    }
}

fn strip_dbug_tree(slab: &mut NounSlab, noun: Noun) -> Noun {
    let noun = skip_dbug(noun);
    match noun.as_either_atom_cell() {
        Either::Left(_) => slab.copy_into(noun),
        Either::Right(cell) => {
            let head = strip_dbug_tree(slab, cell.head());
            let tail = strip_dbug_tree(slab, cell.tail());
            T(slab, &[head, tail])
        }
    }
}

struct Mismatch {
    axis: u64,
    expected: Noun,
    actual: Noun,
    parent_axis: Option<u64>,
    parent_expected: Option<Noun>,
    parent_actual: Option<Noun>,
}

struct MismatchPath {
    path: Vec<u8>,
    expected: Noun,
    actual: Noun,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SpotData {
    path: String,
    p: (u64, u64),
    q: (u64, u64),
}

struct SpotMismatch {
    path: Vec<u8>,
    expected_spots: Vec<SpotData>,
    actual_spots: Vec<SpotData>,
    expected_node: Noun,
    actual_node: Noun,
}

fn collect_dbug_chain(noun: Noun) -> (Vec<SpotData>, Noun) {
    let mut spots = Vec::new();
    let mut cursor = noun;
    loop {
        let cell = match cursor.as_cell() {
            Ok(cell) => cell,
            Err(_) => return (spots, cursor),
        };
        let head = match cell.head().as_atom() {
            Ok(atom) => atom,
            Err(_) => return (spots, cursor),
        };
        if unsafe { !head.as_noun().raw_equals(&D(tas!(b"dbug"))) } {
            return (spots, cursor);
        }
        let tail_cell = match cell.tail().as_cell() {
            Ok(cell) => cell,
            Err(_) => return (spots, cursor),
        };
        let spot = tail_cell.head();
        if let Some((path, p, q)) = decode_spot(spot) {
            spots.push(SpotData { path, p, q });
        }
        cursor = tail_cell.tail();
    }
}

fn first_spot_mismatch(expected: Noun, actual: Noun, path: &mut Vec<u8>) -> Option<SpotMismatch> {
    let (expected_spots, expected_node) = collect_dbug_chain(expected);
    let (actual_spots, actual_node) = collect_dbug_chain(actual);
    if expected_spots != actual_spots {
        return Some(SpotMismatch {
            path: path.clone(),
            expected_spots,
            actual_spots,
            expected_node,
            actual_node,
        });
    }

    match (
        expected_node.as_either_atom_cell(),
        actual_node.as_either_atom_cell(),
    ) {
        (Either::Right(ec), Either::Right(ac)) => {
            path.push(0);
            if let Some(mismatch) = first_spot_mismatch(ec.head(), ac.head(), path) {
                return Some(mismatch);
            }
            path.pop();
            path.push(1);
            if let Some(mismatch) = first_spot_mismatch(ec.tail(), ac.tail(), path) {
                return Some(mismatch);
            }
            path.pop();
            None
        }
        _ => None,
    }
}

fn format_spot_line(spot: &SpotData) -> String {
    if let Some(text) = line_excerpt(&spot.path, spot.p.0) {
        return format!(
            "{} [{} {}] [{} {}] :: {}",
            spot.path, spot.p.0, spot.p.1, spot.q.0, spot.q.1, text
        );
    }
    format!(
        "{} [{} {}] [{} {}]",
        spot.path, spot.p.0, spot.p.1, spot.q.0, spot.q.1
    )
}

impl Mismatch {
    fn with_parent(mut self, axis: u64, expected: Noun, actual: Noun) -> Self {
        if self.parent_axis.is_none() {
            self.parent_axis = Some(axis);
            self.parent_expected = Some(expected);
            self.parent_actual = Some(actual);
        }
        self
    }
}

fn noun_at_axis(mut noun: Noun, axis: u64) -> Option<Noun> {
    if axis == 1 {
        return Some(noun);
    }
    let mut bits = Vec::new();
    let mut cursor = axis;
    while cursor > 1 {
        bits.push(cursor & 1);
        cursor >>= 1;
    }
    for bit in bits.into_iter().rev() {
        let cell = noun.as_cell().ok()?;
        noun = if bit == 0 { cell.head() } else { cell.tail() };
    }
    Some(noun)
}

fn noun_at_path(mut noun: Noun, path: &[u8]) -> Option<Noun> {
    for bit in path {
        let cell = noun.as_cell().ok()?;
        noun = if *bit == 0 { cell.head() } else { cell.tail() };
    }
    Some(noun)
}

fn decode_pint(noun: Noun) -> Option<((u64, u64), (u64, u64))> {
    let (p, q) = expect_cell(noun, "pint").ok()?;
    let (pl, pc) = expect_cell(p, "pint p").ok()?;
    let (ql, qc) = expect_cell(q, "pint q").ok()?;
    let pl = pl.as_atom().ok()?.as_u64().ok()?;
    let pc = pc.as_atom().ok()?.as_u64().ok()?;
    let ql = ql.as_atom().ok()?.as_u64().ok()?;
    let qc = qc.as_atom().ok()?.as_u64().ok()?;
    Some(((pl, pc), (ql, qc)))
}

fn decode_spot(noun: Noun) -> Option<(String, (u64, u64), (u64, u64))> {
    let (path, pint) = expect_cell(noun, "spot").ok()?;
    let path_string = path_noun_to_string(path).ok()?;
    let (p, q) = decode_pint(pint)?;
    Some((path_string, p, q))
}

fn line_excerpt(path: &str, line: u64) -> Option<String> {
    let path = Path::new(path);
    let contents = fs::read_to_string(path).ok()?;
    let line = line as usize;
    let text = contents.lines().nth(line.saturating_sub(1))?;
    Some(text.to_string())
}

fn describe_nearest_dbug_path(noun: Noun, path: &[u8]) -> Option<String> {
    let mut cursor = path.to_vec();
    loop {
        let node = noun_at_path(noun, &cursor)?;
        if let Ok(cell) = node.as_cell() {
            if let Ok(atom) = cell.head().as_atom() {
                if unsafe { atom.as_noun().raw_equals(&D(tas!(b"dbug"))) } {
                    let (spot, _rest) = expect_cell(cell.tail(), "dbug tail").ok()?;
                    if let Some((path, (pl, pc), (ql, qc))) = decode_spot(spot) {
                        if let Some(text) = line_excerpt(&path, pl) {
                            return Some(format!("{path} [{pl} {pc}] [{ql} {qc}] :: {text}"));
                        }
                        return Some(format!("{path} [{pl} {pc}] [{ql} {qc}]"));
                    }
                }
            }
        }
        if cursor.is_empty() {
            break;
        }
        cursor.pop();
    }
    None
}

fn describe_nearest_dbug(noun: Noun, axis: u64) -> Option<String> {
    let mut cursor = axis;
    loop {
        let node = noun_at_axis(noun, cursor)?;
        if let Ok(cell) = node.as_cell() {
            if let Ok(atom) = cell.head().as_atom() {
                if unsafe { atom.as_noun().raw_equals(&D(tas!(b"dbug"))) } {
                    let (spot, _rest) = expect_cell(cell.tail(), "dbug tail").ok()?;
                    if let Some((path, (pl, pc), (ql, qc))) = decode_spot(spot) {
                        return Some(format!("{path} [{pl} {pc}] [{ql} {qc}]"));
                    }
                }
            }
        }
        if cursor == 1 {
            break;
        }
        cursor >>= 1;
    }
    None
}

fn describe_nearest_dbug_with_excerpt(noun: Noun, axis: u64) -> Option<String> {
    let mut cursor = axis;
    loop {
        let node = noun_at_axis(noun, cursor)?;
        if let Ok(cell) = node.as_cell() {
            if let Ok(atom) = cell.head().as_atom() {
                if unsafe { atom.as_noun().raw_equals(&D(tas!(b"dbug"))) } {
                    let (spot, _rest) = expect_cell(cell.tail(), "dbug tail").ok()?;
                    if let Some((path, (pl, pc), (ql, qc))) = decode_spot(spot) {
                        if let Some(text) = line_excerpt(&path, pl) {
                            return Some(format!("{path} [{pl} {pc}] [{ql} {qc}] :: {text}"));
                        }
                        return Some(format!("{path} [{pl} {pc}] [{ql} {qc}]"));
                    }
                }
            }
        }
        if cursor == 1 {
            break;
        }
        cursor >>= 1;
    }
    None
}

fn format_axis_bits(axis: u64) -> String {
    if axis <= 1 {
        return String::new();
    }
    let mut bits = Vec::new();
    let mut cursor = axis;
    while cursor > 1 {
        bits.push(if cursor & 1 == 1 { '1' } else { '0' });
        cursor >>= 1;
    }
    bits.reverse();
    bits.into_iter().collect()
}

fn find_mismatch_axis(a: Noun, b: Noun, axis: u64) -> Option<Mismatch> {
    let a = skip_dbug(a);
    let b = skip_dbug(b);

    if slab_noun_equality(&a, &b) {
        return None;
    }

    match (a.as_either_atom_cell(), b.as_either_atom_cell()) {
        (Either::Right(ac), Either::Right(bc)) => {
            if let Some(mismatch) = find_mismatch_axis(ac.head(), bc.head(), axis * 2) {
                return Some(mismatch.with_parent(axis, a, b));
            }
            if let Some(mismatch) = find_mismatch_axis(ac.tail(), bc.tail(), axis * 2 + 1) {
                return Some(mismatch.with_parent(axis, a, b));
            }
            Some(Mismatch {
                axis,
                expected: a,
                actual: b,
                parent_axis: None,
                parent_expected: None,
                parent_actual: None,
            })
        }
        _ => Some(Mismatch {
            axis,
            expected: a,
            actual: b,
            parent_axis: None,
            parent_expected: None,
            parent_actual: None,
        }),
    }
}

fn find_mismatch_axis_raw(a: Noun, b: Noun, axis: u64) -> Option<Mismatch> {
    if slab_noun_equality(&a, &b) {
        return None;
    }

    match (a.as_either_atom_cell(), b.as_either_atom_cell()) {
        (Either::Right(ac), Either::Right(bc)) => {
            if let Some(mismatch) = find_mismatch_axis_raw(ac.head(), bc.head(), axis * 2) {
                return Some(mismatch.with_parent(axis, a, b));
            }
            if let Some(mismatch) = find_mismatch_axis_raw(ac.tail(), bc.tail(), axis * 2 + 1) {
                return Some(mismatch.with_parent(axis, a, b));
            }
            Some(Mismatch {
                axis,
                expected: a,
                actual: b,
                parent_axis: None,
                parent_expected: None,
                parent_actual: None,
            })
        }
        _ => Some(Mismatch {
            axis,
            expected: a,
            actual: b,
            parent_axis: None,
            parent_expected: None,
            parent_actual: None,
        }),
    }
}

fn find_mismatch_path_raw(a: Noun, b: Noun, path: &mut Vec<u8>) -> Option<MismatchPath> {
    if slab_noun_equality(&a, &b) {
        return None;
    }

    match (a.as_either_atom_cell(), b.as_either_atom_cell()) {
        (Either::Right(ac), Either::Right(bc)) => {
            path.push(0);
            if let Some(mismatch) = find_mismatch_path_raw(ac.head(), bc.head(), path) {
                return Some(mismatch);
            }
            path.pop();
            path.push(1);
            if let Some(mismatch) = find_mismatch_path_raw(ac.tail(), bc.tail(), path) {
                return Some(mismatch);
            }
            path.pop();
            Some(MismatchPath {
                path: path.clone(),
                expected: a,
                actual: b,
            })
        }
        _ => Some(MismatchPath {
            path: path.clone(),
            expected: a,
            actual: b,
        }),
    }
}

fn format_path_bits(path: &[u8]) -> String {
    let mut out = String::with_capacity(path.len());
    for bit in path {
        out.push(if *bit == 0 { '0' } else { '1' });
    }
    out
}

fn find_latest_checkpoint(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut latest: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let modified = entry.metadata()?.modified()?;
        if latest
            .as_ref()
            .map(|(time, _)| modified > *time)
            .unwrap_or(true)
        {
            latest = Some((modified, entry.path()));
        }
    }
    latest
        .map(|(_, path)| path)
        .ok_or_else(|| format!("No checkpoint found in {}", dir.display()).into())
}

fn load_state_from_checkpoint(path: &Path) -> Result<NounSlab, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    let checkpoint = JammedCheckpoint::decode_from_bytes(&bytes)?;
    let mut slab = NounSlab::new();
    let root = slab.cue_into(checkpoint.state_jam.0.clone())?;
    slab.set_root(root);
    Ok(slab)
}

async fn parse_hoon_with_hoonc(
    entry: &PathBuf,
    deps_dir: &PathBuf,
) -> Result<NounSlab, Box<dyn std::error::Error>> {
    let nockapp_home = temp_out_dir("hoonc-state")?;
    let _home_guard = EnvVarGuard::set("NOCKAPP_HOME", nockapp_home.to_string_lossy().as_ref());
    let _prewarm_guard = EnvVarGuard::set("HOONC_DISABLE_PREWARM", "1");
    let (mut nockapp, _out_path) =
        initialize_with_default_cli(entry.clone(), deps_dir.clone(), None, false, true).await?;
    let entry_string = entry_path_for_hoon(entry, deps_dir)?;
    let entry_contents = fs::read(entry)?;

    let mut slab = NounSlab::new();
    let entry_path = Atom::from_value(&mut slab, entry_string)?.as_noun();
    let entry_contents = Atom::from_value(&mut slab, entry_contents)?.as_noun();
    let directory_noun = build_directory_noun_for_parse(&mut slab, deps_dir)?;

    let parse_poke = T(
        &mut slab,
        &[D(tas!(b"parse")), entry_path, entry_contents, directory_noun],
    );
    slab.set_root(parse_poke);
    nockapp.poke(OnePunchWire::Poke.to_wire(), slab).await?;

    nockapp
        .save_blocking()
        .await
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err.to_string()))?;

    let checkpoints_dir = nockapp_home.join("hoonc").join("checkpoints");
    let checkpoint_path = find_latest_checkpoint(&checkpoints_dir)?;
    load_state_from_checkpoint(&checkpoint_path)
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
        "prime-dir: hoon mismatch", "hoonc: compile failed", "hoonc: build failed", "syntax error",
        "hoonc: missing dependency", "nockapp exited with error code", "Exit(1)", "-find.",
    ];
    markers.iter().any(|marker| logs.contains(marker))
}

fn noun_is_zero(noun: &Noun) -> bool {
    unsafe { noun.raw_equals(&D(0)) }
}

fn term_from_noun(noun: &Noun) -> Option<String> {
    if noun_is_zero(noun) {
        return Some("$".to_string());
    }
    let atom = noun.as_atom().ok()?;
    atom.into_string().ok()
}

fn validate_limb_noun(noun: &Noun) -> Result<(), String> {
    if noun.as_atom().is_ok() {
        return Ok(());
    }

    let cell = noun
        .as_cell()
        .map_err(|_| "limb is neither atom nor cell".to_string())?;
    let head = cell.head();
    let tail = cell.tail();
    let tag_atom = head
        .as_atom()
        .map_err(|_| "limb head not atom".to_string())?;
    let tag_u64 = tag_atom.as_u64().ok();
    let tag_label = tag_u64
        .map(|value| value.to_string())
        .or_else(|| term_from_noun(&head))
        .unwrap_or_else(|| "<non-term>".to_string());
    match tag_u64 {
        Some(0) | Some(38) => {
            if tail.as_atom().is_err() {
                return Err("limb %& axis is not atom".to_string());
            }
            Ok(())
        }
        Some(1) | Some(124) => {
            let tail_cell = tail
                .as_cell()
                .map_err(|_| "limb %| tail is not cell".to_string())?;
            let p = tail_cell.head();
            let q = tail_cell.tail();
            if p.as_atom().is_err() {
                return Err("limb %| p is not atom".to_string());
            }
            if noun_is_zero(&q) {
                return Ok(());
            }
            let q_cell = q
                .as_cell()
                .map_err(|_| "limb %| q is not unit".to_string())?;
            let q_head = q_cell.head();
            let q_tail = q_cell.tail();
            if !noun_is_zero(&q_head) {
                return Err("limb %| q head is not 0".to_string());
            }
            if term_from_noun(&q_tail).is_none() {
                return Err("limb %| q tail is not term".to_string());
            }
            Ok(())
        }
        _ => Err(format!("limb tag not 0/1/&/|: {tag_label}")),
    }
}

fn validate_wing_noun(noun: &Noun) -> Result<(), String> {
    if noun_is_zero(noun) {
        return Ok(());
    }
    let mut cursor = *noun;
    loop {
        let cell = cursor
            .as_cell()
            .map_err(|_| "wing tail is not list".to_string())?;
        let head = cell.head();
        let tail = cell.tail();
        validate_limb_noun(&head)?;
        if noun_is_zero(&tail) {
            return Ok(());
        }
        cursor = tail;
    }
}

fn validate_hoon_tag(noun: &Noun) -> Result<(), String> {
    let cell = noun.as_cell().map_err(|_| "hoon is atom".to_string())?;
    let head = cell.head();
    let tag = term_from_noun(&head).ok_or_else(|| "hoon head not term".to_string())?;
    if tag.is_empty() {
        return Err("hoon tag is empty".to_string());
    }
    Ok(())
}

fn validate_cnts_noun(noun: &Noun) -> Result<(), String> {
    let cell = noun
        .as_cell()
        .map_err(|_| "cnts noun is atom".to_string())?;
    let head = cell.head();
    let tail = cell.tail();
    let tag = term_from_noun(&head).ok_or_else(|| "cnts head not term".to_string())?;
    if tag != "cnts" {
        return Err(format!("expected cnts tag, got {tag}"));
    }
    let tail_cell = tail
        .as_cell()
        .map_err(|_| "cnts tail is not cell".to_string())?;
    let wing = tail_cell.head();
    let pairs = tail_cell.tail();
    validate_wing_noun(&wing)?;
    if noun_is_zero(&pairs) {
        return Ok(());
    }
    let mut cursor = pairs;
    let mut idx = 0usize;
    loop {
        let pair_cell = cursor
            .as_cell()
            .map_err(|_| format!("cnts list tail is not cell at {idx}"))?;
        let item = pair_cell.head();
        let rest = pair_cell.tail();
        let item_cell = item
            .as_cell()
            .map_err(|_| format!("cnts list item is not cell at {idx}"))?;
        let item_wing = item_cell.head();
        let item_hoon = item_cell.tail();
        validate_wing_noun(&item_wing)
            .map_err(|err| format!("cnts list item wing {idx} invalid: {err}"))?;
        validate_hoon_tag(&item_hoon)
            .map_err(|err| format!("cnts list item hoon {idx} invalid: {err}"))?;
        if noun_is_zero(&rest) {
            break;
        }
        cursor = rest;
        idx += 1;
    }
    Ok(())
}

fn native_subset_for_paths(
    entry: &PathBuf,
    native_asts: &HashMap<PathBuf, Vec<u8>>,
    paths: &[PathBuf],
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
    for path in paths {
        let jam = native_asts.get(path).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("native AST missing for {}", path.display()),
            )
        })?;
        subset.insert(path.clone(), jam.clone());
    }
    Ok(subset)
}

fn native_subset_for_prefix(
    entry: &PathBuf,
    native_asts: &HashMap<PathBuf, Vec<u8>>,
    sorted_paths: &[PathBuf],
    prefix_len: usize,
) -> Result<HashMap<PathBuf, Vec<u8>>, Box<dyn std::error::Error>> {
    let subset_paths = &sorted_paths[..prefix_len];
    native_subset_for_paths(entry, native_asts, subset_paths)
}

async fn prime_with_subset(
    entry: &PathBuf,
    deps_dir: &PathBuf,
    subset: &HashMap<PathBuf, Vec<u8>>,
    log_capture: &LogCapture,
) -> Result<PrimeAttempt, Box<dyn std::error::Error>> {
    let nockapp_home = temp_out_dir("bisect-home")?;
    let out_dir = temp_out_dir("bisect-out")?;
    let _env_guard = EnvVarGuard::set("NOCKAPP_HOME", nockapp_home.to_string_lossy().as_ref());
    let _prewarm_guard = EnvVarGuard::set("HOONC_DISABLE_PREWARM", "1");

    log_capture.clear();
    let build_result = build_jam_with_primed_parse_cache(
        entry.clone(),
        deps_dir.clone(),
        Some(out_dir),
        false,
        true,
        subset,
    )
    .await;
    let logs = log_capture.take_string();

    let warned = logs.contains("hoonc: warning: input is not a proper cause");
    let failed = build_result.is_err() || detect_prime_failure(&logs);
    let pc_size = parse_pc_size(&logs);
    if failed && !logs.is_empty() {
        println!("prime logs:\n{logs}");
    }
    if let Err(err) = build_result {
        println!("prime build error: {err}");
    }
    Ok(PrimeAttempt {
        warned,
        failed,
        pc_size,
    })
}

async fn prime_only_with_subset(
    entry: &PathBuf,
    deps_dir: &PathBuf,
    subset: &HashMap<PathBuf, Vec<u8>>,
    log_capture: &LogCapture,
) -> Result<PrimeAttempt, Box<dyn std::error::Error>> {
    let nockapp_home = temp_out_dir("prime-only-home")?;
    let _env_guard = EnvVarGuard::set("NOCKAPP_HOME", nockapp_home.to_string_lossy().as_ref());
    let _prewarm_guard = EnvVarGuard::set("HOONC_DISABLE_PREWARM", "1");

    log_capture.clear();
    let (mut nockapp, _out_path) =
        initialize_with_default_cli(entry.clone(), deps_dir.clone(), None, false, true).await?;
    let prime_result = prime_parse_cache_public(&mut nockapp, entry, deps_dir, subset).await;
    let logs = log_capture.take_string();

    let warned = logs.contains("hoonc: warning: input is not a proper cause");
    let failed = prime_result.is_err() || detect_prime_failure(&logs);
    let pc_size = parse_pc_size(&logs);
    if failed && !logs.is_empty() {
        println!("prime-only logs:\n{logs}");
    }
    if let Err(err) = prime_result {
        println!("prime-only error: {err}");
    }
    Ok(PrimeAttempt {
        warned,
        failed,
        pc_size,
    })
}

async fn build_regular_jam_for_bisect(
    entry: &PathBuf,
    deps_dir: &PathBuf,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let nockapp_home = temp_out_dir("bisect-regular-home")?;
    let out_dir = temp_out_dir("bisect-regular-out")?;
    let _env_guard = EnvVarGuard::set("NOCKAPP_HOME", nockapp_home.to_string_lossy().as_ref());
    let _prewarm_guard = EnvVarGuard::set("HOONC_DISABLE_PREWARM", "1");
    Ok(build_jam(entry, deps_dir.clone(), Some(out_dir), false, true).await?)
}

async fn build_primed_jam_with_subset(
    entry: &PathBuf,
    deps_dir: &PathBuf,
    subset: &HashMap<PathBuf, Vec<u8>>,
    log_capture: &LogCapture,
) -> Result<(Vec<u8>, String, Option<usize>), Box<dyn std::error::Error>> {
    let nockapp_home = temp_out_dir("bisect-primed-home")?;
    let out_dir = temp_out_dir("bisect-primed-out")?;
    let _env_guard = EnvVarGuard::set("NOCKAPP_HOME", nockapp_home.to_string_lossy().as_ref());
    let _prewarm_guard = EnvVarGuard::set("HOONC_DISABLE_PREWARM", "1");

    log_capture.clear();
    let jam = build_jam_with_primed_parse_cache(
        entry.clone(),
        deps_dir.clone(),
        Some(out_dir),
        false,
        true,
        subset,
    )
    .await?;
    let logs = log_capture.take_string();
    let pc_size = parse_pc_size(&logs);
    Ok((jam, logs, pc_size))
}

fn format_variant_inventory(counts: &HashMap<String, usize>, max_items: usize) -> String {
    let mut items: Vec<_> = counts.iter().collect();
    items.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let mut out = String::new();
    for (name, count) in items.into_iter().take(max_items) {
        if !out.is_empty() {
            out.push_str(", ");
        }
        out.push_str(name);
        out.push('=');
        out.push_str(&count.to_string());
    }
    out
}

async fn bisect_first_failing_path(
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
    if let Ok(filter) = std::env::var("HOONC_PRIME_BISECT_FILTER") {
        let filter = filter.trim();
        if !filter.is_empty() {
            paths.retain(|path| path.to_string_lossy().contains(filter));
        }
    }
    if let Some(limit) = std::env::var("HOONC_PRIME_BISECT_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    {
        paths.truncate(limit);
    }

    let full_attempt = {
        let subset = native_subset_for_prefix(entry, native_asts, &paths, paths.len())?;
        prime_with_subset(entry, deps_dir, &subset, log_capture).await?
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
        let attempt = prime_with_subset(entry, deps_dir, &subset, log_capture).await?;
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

async fn bisect_first_jam_mismatch_path(
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
    if let Ok(filter) = std::env::var("HOONC_PRIME_BISECT_FILTER") {
        let filter = filter.trim();
        if !filter.is_empty() {
            paths.retain(|path| path.to_string_lossy().contains(filter));
        }
    }
    if let Some(limit) = std::env::var("HOONC_PRIME_BISECT_LIMIT")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
    {
        paths.truncate(limit);
    }

    let regular_jam = build_regular_jam_for_bisect(entry, deps_dir).await?;

    let full_subset = native_subset_for_prefix(entry, native_asts, &paths, paths.len())?;
    let (full_primed, _logs, pc_size) =
        build_primed_jam_with_subset(entry, deps_dir, &full_subset, log_capture).await?;
    let full_mismatch = full_primed != regular_jam;
    println!(
        "bisect-jam: full set mismatch={} pc-size={:?}",
        full_mismatch, pc_size
    );
    if !full_mismatch {
        return Ok(None);
    }

    let mut low = 0usize;
    let mut high = paths.len();
    while low < high {
        let mid = (low + high) / 2;
        let subset = native_subset_for_prefix(entry, native_asts, &paths, mid)?;
        let (primed, _logs, pc_size) =
            build_primed_jam_with_subset(entry, deps_dir, &subset, log_capture).await?;
        let mismatch = primed != regular_jam;
        println!(
            "bisect-jam: prefix={} mismatch={} pc-size={:?}",
            mid, mismatch, pc_size
        );
        if mismatch {
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

fn resolve_hoon_path(deps_dir: &Path, path: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let raw = PathBuf::from(path);
    let resolved = if raw.is_absolute() {
        raw
    } else {
        deps_dir.join(raw)
    };
    Ok(resolved.canonicalize()?)
}

fn parse_native_hoon(
    path: &Path,
    deps_dir: &Path,
) -> Result<ast::Hoon, Box<dyn std::error::Error>> {
    let source = fs::read_to_string(path)?;
    let linemap = Arc::new(LineMap::new(&source));
    let wer = hoon_path_for_any(path, deps_dir);
    let parsed = native_parser(wer, true, linemap)
        .parse(source.as_str())
        .into_result()
        .map_err(|errs| {
            io::Error::new(
                io::ErrorKind::Other,
                format!("native parser failed for {}: {:?}", path.display(), errs),
            )
        })?;
    Ok(parsed)
}

fn hoon_variant_label(hoon: &ast::Hoon) -> String {
    let debug = format!("{hoon:?}");
    let end = debug
        .find(|c: char| c == '(' || c == '{' || c == '[')
        .unwrap_or_else(|| debug.len());
    debug[..end].to_string()
}

fn collect_hoon_variants(hoon: &ast::Hoon, counts: &mut HashMap<String, usize>) {
    use ast::Hoon::*;

    let label = hoon_variant_label(hoon);
    *counts.entry(label).or_insert(0) += 1;

    match hoon {
        Pair(a, b) => {
            collect_hoon_variants(a, counts);
            collect_hoon_variants(b, counts);
        }
        ZapZap
        | Axis(_)
        | Base(_)
        | Bust(_)
        | Eror(_)
        | Leaf(_, _)
        | Limb(_)
        | Rock(_, _)
        | Sand(_, _)
        | Wing(_) => {}
        Dbug(_, h)
        | Note(_, h)
        | Fits(h, _)
        | Lost(h)
        | BarDot(h)
        | BarHep(h)
        | BarWut(h)
        | DotLus(h)
        | DotWut(h)
        | KetBar(h)
        | KetPam(h)
        | KetSig(h)
        | KetWut(h)
        | SigBuc(_, h)
        | SigLus(_, h)
        | SigFas(_, h)
        | MicFas(h)
        | WutZap(h)
        | ZapGar(h)
        | ZapTis(h)
        | ZapWut(_, h) => {
            collect_hoon_variants(h, counts);
        }
        Hand(typ, _) => collect_hoon_variants_in_type(typ, counts),
        Knit(woofs) => {
            for woof in woofs {
                collect_hoon_variants_in_woof(woof, counts);
            }
        }
        Tell(hoons) | Yell(hoons) | ColSig(hoons) | ColTar(hoons) | TisSig(hoons)
        | WutBar(hoons) | WutPam(hoons) => {
            for h in hoons {
                collect_hoon_variants(h, counts);
            }
        }
        Tune(term_or_tune) => collect_hoon_variants_in_term_or_tune(term_or_tune, counts),
        Xray(manx) => collect_hoon_variants_in_manx(manx, counts),
        BarBuc(_, spec) | KetTar(spec) | KetCol(spec) => {
            collect_hoon_variants_in_spec(spec, counts);
        }
        BarCab(spec, alas, tomes) => {
            collect_hoon_variants_in_spec(spec, counts);
            collect_hoon_variants_in_alas(alas, counts);
            collect_hoon_variants_in_tomes(tomes, counts);
        }
        BarCol(a, b)
        | CenDot(a, b)
        | CenHep(a, b)
        | ColCab(a, b)
        | ColHep(a, b)
        | DotTar(a, b)
        | DotTis(a, b)
        | KetDot(a, b)
        | KetLus(a, b)
        | SigBar(a, b)
        | SigCab(a, b)
        | SigPam(_, a, b)
        | SigTis(a, b)
        | SigZap(a, b)
        | TisDot(_, a, b)
        | TisGal(a, b)
        | TisHep(a, b)
        | TisGar(a, b)
        | TisLus(a, b)
        | TisCom(a, b)
        | WutKet(_, a, b)
        | WutGal(a, b)
        | WutGar(a, b)
        | WutPat(_, a, b)
        | WutSig(_, a, b)
        | ZapCom(a, b)
        | ZapMic(a, b)
        | ZapPat(_, a, b) => {
            collect_hoon_variants(a, counts);
            collect_hoon_variants(b, counts);
        }
        BarCen(_, tomes) | BarPat(_, tomes) => {
            collect_hoon_variants_in_tomes(tomes, counts);
        }
        BarKet(h, tomes) => {
            collect_hoon_variants(h, counts);
            collect_hoon_variants_in_tomes(tomes, counts);
        }
        BarSig(spec, h)
        | BarTar(spec, h)
        | BarTis(spec, h)
        | DotKet(spec, h)
        | KetHep(spec, h)
        | MicMic(spec, h)
        | TisBar(spec, h)
        | ZapGal(spec, h) => {
            collect_hoon_variants_in_spec(spec, counts);
            collect_hoon_variants(h, counts);
        }
        ColKet(a, b, c, d) | CenKet(a, b, c, d) => {
            collect_hoon_variants(a, counts);
            collect_hoon_variants(b, counts);
            collect_hoon_variants(c, counts);
            collect_hoon_variants(d, counts);
        }
        ColLus(a, b, c)
        | CenLus(a, b, c)
        | WutCol(a, b, c)
        | WutDot(a, b, c)
        | SigWut(_, a, b, c)
        | TisWut(_, a, b, c) => {
            collect_hoon_variants(a, counts);
            collect_hoon_variants(b, counts);
            collect_hoon_variants(c, counts);
        }
        CenCab(_, items) | CenTis(_, items) => {
            for (_, h) in items {
                collect_hoon_variants(h, counts);
            }
        }
        CenCol(a, hoons) | CenSig(_, a, hoons) | MicSig(a, hoons) | MicCol(a, hoons) => {
            collect_hoon_variants(a, counts);
            for h in hoons {
                collect_hoon_variants(h, counts);
            }
        }
        CenTar(_, h, items) => {
            collect_hoon_variants(h, counts);
            for (_, item) in items {
                collect_hoon_variants(item, counts);
            }
        }
        KetTis(skin, h) => {
            collect_hoon_variants_in_skin(skin, counts);
            collect_hoon_variants(h, counts);
        }
        SigCen(_, a, tyre, b) => {
            collect_hoon_variants(a, counts);
            collect_hoon_variants_in_tyre(tyre, counts);
            collect_hoon_variants(b, counts);
        }
        SigGal(term_or_pair, h) | SigGar(term_or_pair, h) => {
            collect_hoon_variants_in_term_or_pair(term_or_pair, counts);
            collect_hoon_variants(h, counts);
        }
        MicTis(marl) => collect_hoon_variants_in_marl(marl, counts),
        MicGal(spec, a, b, c) => {
            collect_hoon_variants_in_spec(spec, counts);
            collect_hoon_variants(a, counts);
            collect_hoon_variants(b, counts);
            collect_hoon_variants(c, counts);
        }
        TisCol(items, h) => {
            for (_, item) in items {
                collect_hoon_variants(item, counts);
            }
            collect_hoon_variants(h, counts);
        }
        TisFas(skin, a, b) | TisMic(skin, a, b) => {
            collect_hoon_variants_in_skin(skin, counts);
            collect_hoon_variants(a, counts);
            collect_hoon_variants(b, counts);
        }
        TisKet(skin, _, a, b) => {
            collect_hoon_variants_in_skin(skin, counts);
            collect_hoon_variants(a, counts);
            collect_hoon_variants(b, counts);
        }
        TisTar((_, spec), a, b) => {
            if let Some(spec) = spec {
                collect_hoon_variants_in_spec(spec, counts);
            }
            collect_hoon_variants(a, counts);
            collect_hoon_variants(b, counts);
        }
        WutHep(_, pairs) => {
            for (spec, h) in pairs {
                collect_hoon_variants_in_spec(spec, counts);
                collect_hoon_variants(h, counts);
            }
        }
        WutLus(_, h, pairs) => {
            collect_hoon_variants(h, counts);
            for (spec, item) in pairs {
                collect_hoon_variants_in_spec(spec, counts);
                collect_hoon_variants(item, counts);
            }
        }
        WutHax(skin, _) => collect_hoon_variants_in_skin(skin, counts),
        WutTis(spec, _) => collect_hoon_variants_in_spec(spec, counts),
    }
}

fn collect_hoon_variants_in_alas(alas: &ast::Alas, counts: &mut HashMap<String, usize>) {
    for (_, h) in alas {
        collect_hoon_variants(h, counts);
    }
}

fn collect_hoon_variants_in_tomes(
    tomes: &HashMap<String, ast::Tome>,
    counts: &mut HashMap<String, usize>,
) {
    for tome in tomes.values() {
        collect_hoon_variants_in_tome(tome, counts);
    }
}

fn collect_hoon_variants_in_tome(tome: &ast::Tome, counts: &mut HashMap<String, usize>) {
    for hoon in tome.1.values() {
        collect_hoon_variants(hoon, counts);
    }
}

fn collect_hoon_variants_in_tyre(tyre: &ast::Tyre, counts: &mut HashMap<String, usize>) {
    for (_, hoon) in tyre {
        collect_hoon_variants(hoon, counts);
    }
}

fn collect_hoon_variants_in_term_or_tune(
    term_or_tune: &ast::TermOrTune,
    counts: &mut HashMap<String, usize>,
) {
    match term_or_tune {
        ast::TermOrTune::Term(_) => {}
        ast::TermOrTune::Tune(tune) => collect_hoon_variants_in_tune(tune, counts),
    }
}

fn collect_hoon_variants_in_tune(tune: &ast::Tune, counts: &mut HashMap<String, usize>) {
    for hoon in tune.0.values().flatten() {
        collect_hoon_variants(hoon, counts);
    }
    for hoon in &tune.1 {
        collect_hoon_variants(hoon, counts);
    }
}

fn collect_hoon_variants_in_term_or_pair(
    term_or_pair: &ast::TermOrPair,
    counts: &mut HashMap<String, usize>,
) {
    if let ast::TermOrPair::Pair(_, hoon) = term_or_pair {
        collect_hoon_variants(hoon, counts);
    }
}

fn collect_hoon_variants_in_spec(spec: &ast::Spec, counts: &mut HashMap<String, usize>) {
    use ast::Spec::*;

    match spec {
        Base(_) | Leaf(_, _) | Like(_, _) | Loop(_) => {}
        Dbug(_, spec) | Made(_, spec) | Name(_, spec) | Over(_, spec) | BucLus(_, spec) => {
            collect_hoon_variants_in_spec(spec, counts)
        }
        Make(hoon, specs) => {
            collect_hoon_variants(hoon, counts);
            for spec in specs {
                collect_hoon_variants_in_spec(spec, counts);
            }
        }
        BucGar(a, b) | BucGal(a, b) | BucHep(a, b) | BucKet(a, b) | BucPat(a, b) => {
            collect_hoon_variants_in_spec(a, counts);
            collect_hoon_variants_in_spec(b, counts);
        }
        BucBuc(spec, map)
        | BucDot(spec, map)
        | BucFas(spec, map)
        | BucTic(spec, map)
        | BucZap(spec, map) => {
            collect_hoon_variants_in_spec(spec, counts);
            for spec in map.values() {
                collect_hoon_variants_in_spec(spec, counts);
            }
        }
        BucBar(spec, hoon) | BucPam(spec, hoon) => {
            collect_hoon_variants_in_spec(spec, counts);
            collect_hoon_variants(hoon, counts);
        }
        BucCab(hoon) | BucMic(hoon) => collect_hoon_variants(hoon, counts),
        BucCol(spec, specs) | BucCen(spec, specs) | BucWut(spec, specs) => {
            collect_hoon_variants_in_spec(spec, counts);
            for spec in specs {
                collect_hoon_variants_in_spec(spec, counts);
            }
        }
        BucSig(hoon, spec) => {
            collect_hoon_variants(hoon, counts);
            collect_hoon_variants_in_spec(spec, counts);
        }
        BucTis(skin, spec) => {
            collect_hoon_variants_in_skin(skin, counts);
            collect_hoon_variants_in_spec(spec, counts);
        }
    }
}

fn collect_hoon_variants_in_skin(skin: &ast::Skin, counts: &mut HashMap<String, usize>) {
    use ast::Skin::*;

    match skin {
        Term(_) | Base(_) | Leaf(_, _) | Wash(_) => {}
        Cell(a, b) => {
            collect_hoon_variants_in_skin(a, counts);
            collect_hoon_variants_in_skin(b, counts);
        }
        Dbug(_, skin) | Name(_, skin) | Over(_, skin) => {
            collect_hoon_variants_in_skin(skin, counts);
        }
        Spec(spec, skin) => {
            collect_hoon_variants_in_spec(spec, counts);
            collect_hoon_variants_in_skin(skin, counts);
        }
    }
}

fn collect_cnts_nodes(node: &ast::Hoon, out: &mut Vec<ast::Hoon>) {
    use ast::Hoon::*;

    if let CenTis(_, _) = node {
        out.push(node.clone());
    }

    match node {
        Pair(a, b) => {
            collect_cnts_nodes(a, out);
            collect_cnts_nodes(b, out);
        }
        ZapZap
        | Axis(_)
        | Base(_)
        | Bust(_)
        | Eror(_)
        | Leaf(_, _)
        | Limb(_)
        | Rock(_, _)
        | Sand(_, _)
        | Wing(_) => {}
        Dbug(_, h)
        | Note(_, h)
        | Fits(h, _)
        | Lost(h)
        | BarDot(h)
        | BarHep(h)
        | BarWut(h)
        | DotLus(h)
        | DotWut(h)
        | KetBar(h)
        | KetPam(h)
        | KetSig(h)
        | KetWut(h)
        | SigBuc(_, h)
        | SigLus(_, h)
        | SigFas(_, h)
        | MicFas(h)
        | WutZap(h)
        | ZapGar(h)
        | ZapTis(h)
        | ZapWut(_, h) => collect_cnts_nodes(h, out),
        Hand(typ, _) => collect_cnts_nodes_in_type(typ, out),
        Knit(woofs) => {
            for woof in woofs {
                collect_cnts_nodes_in_woof(woof, out);
            }
        }
        Tell(hoons) | Yell(hoons) | ColSig(hoons) | ColTar(hoons) | TisSig(hoons)
        | WutBar(hoons) | WutPam(hoons) => {
            for h in hoons {
                collect_cnts_nodes(h, out);
            }
        }
        Tune(term_or_tune) => collect_cnts_nodes_in_term_or_tune(term_or_tune, out),
        Xray(manx) => collect_cnts_nodes_in_manx(manx, out),
        BarBuc(_, spec) | KetTar(spec) | KetCol(spec) => {
            collect_cnts_nodes_in_spec(spec, out);
        }
        BarCab(spec, alas, tomes) => {
            collect_cnts_nodes_in_spec(spec, out);
            collect_cnts_nodes_in_alas(alas, out);
            collect_cnts_nodes_in_tomes(tomes, out);
        }
        BarCol(a, b)
        | CenDot(a, b)
        | CenHep(a, b)
        | ColCab(a, b)
        | ColHep(a, b)
        | DotTar(a, b)
        | DotTis(a, b)
        | KetDot(a, b)
        | KetLus(a, b)
        | SigBar(a, b)
        | SigCab(a, b)
        | SigPam(_, a, b)
        | SigTis(a, b)
        | SigZap(a, b)
        | TisDot(_, a, b)
        | TisGal(a, b)
        | TisHep(a, b)
        | TisGar(a, b)
        | TisLus(a, b)
        | TisCom(a, b)
        | WutKet(_, a, b)
        | WutGal(a, b)
        | WutGar(a, b)
        | WutPat(_, a, b)
        | WutSig(_, a, b)
        | ZapCom(a, b)
        | ZapMic(a, b)
        | ZapPat(_, a, b) => {
            collect_cnts_nodes(a, out);
            collect_cnts_nodes(b, out);
        }
        BarCen(_, tomes) | BarPat(_, tomes) => {
            collect_cnts_nodes_in_tomes(tomes, out);
        }
        BarKet(h, tomes) => {
            collect_cnts_nodes(h, out);
            collect_cnts_nodes_in_tomes(tomes, out);
        }
        BarSig(spec, h)
        | BarTar(spec, h)
        | BarTis(spec, h)
        | DotKet(spec, h)
        | KetHep(spec, h)
        | MicMic(spec, h)
        | TisBar(spec, h)
        | ZapGal(spec, h) => {
            collect_cnts_nodes_in_spec(spec, out);
            collect_cnts_nodes(h, out);
        }
        ColKet(a, b, c, d) | CenKet(a, b, c, d) => {
            collect_cnts_nodes(a, out);
            collect_cnts_nodes(b, out);
            collect_cnts_nodes(c, out);
            collect_cnts_nodes(d, out);
        }
        ColLus(a, b, c)
        | CenLus(a, b, c)
        | WutCol(a, b, c)
        | WutDot(a, b, c)
        | SigWut(_, a, b, c)
        | TisWut(_, a, b, c) => {
            collect_cnts_nodes(a, out);
            collect_cnts_nodes(b, out);
            collect_cnts_nodes(c, out);
        }
        CenCab(_, items) | CenTis(_, items) => {
            for (_, h) in items {
                collect_cnts_nodes(h, out);
            }
        }
        CenCol(a, hoons) | CenSig(_, a, hoons) | MicSig(a, hoons) | MicCol(a, hoons) => {
            collect_cnts_nodes(a, out);
            for h in hoons {
                collect_cnts_nodes(h, out);
            }
        }
        CenTar(_, h, items) => {
            collect_cnts_nodes(h, out);
            for (_, item) in items {
                collect_cnts_nodes(item, out);
            }
        }
        KetTis(skin, h) => {
            collect_cnts_nodes_in_skin(skin, out);
            collect_cnts_nodes(h, out);
        }
        SigCen(_, a, tyre, b) => {
            collect_cnts_nodes(a, out);
            collect_cnts_nodes_in_tyre(tyre, out);
            collect_cnts_nodes(b, out);
        }
        SigGal(term_or_pair, h) | SigGar(term_or_pair, h) => {
            collect_cnts_nodes_in_term_or_pair(term_or_pair, out);
            collect_cnts_nodes(h, out);
        }
        MicTis(marl) => collect_cnts_nodes_in_marl(marl, out),
        MicGal(spec, a, b, c) => {
            collect_cnts_nodes_in_spec(spec, out);
            collect_cnts_nodes(a, out);
            collect_cnts_nodes(b, out);
            collect_cnts_nodes(c, out);
        }
        TisCol(items, h) => {
            for (_, item) in items {
                collect_cnts_nodes(item, out);
            }
            collect_cnts_nodes(h, out);
        }
        TisFas(skin, a, b) | TisMic(skin, a, b) => {
            collect_cnts_nodes_in_skin(skin, out);
            collect_cnts_nodes(a, out);
            collect_cnts_nodes(b, out);
        }
        TisKet(skin, _, a, b) => {
            collect_cnts_nodes_in_skin(skin, out);
            collect_cnts_nodes(a, out);
            collect_cnts_nodes(b, out);
        }
        TisTar((_, spec), a, b) => {
            if let Some(spec) = spec {
                collect_cnts_nodes_in_spec(spec, out);
            }
            collect_cnts_nodes(a, out);
            collect_cnts_nodes(b, out);
        }
        WutHep(_, pairs) => {
            for (spec, h) in pairs {
                collect_cnts_nodes_in_spec(spec, out);
                collect_cnts_nodes(h, out);
            }
        }
        WutLus(_, h, pairs) => {
            collect_cnts_nodes(h, out);
            for (spec, item) in pairs {
                collect_cnts_nodes_in_spec(spec, out);
                collect_cnts_nodes(item, out);
            }
        }
        WutHax(skin, _) => collect_cnts_nodes_in_skin(skin, out),
        WutTis(spec, _) => collect_cnts_nodes_in_spec(spec, out),
    }
}

fn collect_cnts_nodes_in_alas(alas: &ast::Alas, out: &mut Vec<ast::Hoon>) {
    for (_, h) in alas {
        collect_cnts_nodes(h, out);
    }
}

fn collect_cnts_nodes_in_tomes(tomes: &HashMap<String, ast::Tome>, out: &mut Vec<ast::Hoon>) {
    for tome in tomes.values() {
        collect_cnts_nodes_in_tome(tome, out);
    }
}

fn collect_cnts_nodes_in_tome(tome: &ast::Tome, out: &mut Vec<ast::Hoon>) {
    for hoon in tome.1.values() {
        collect_cnts_nodes(hoon, out);
    }
}

fn collect_cnts_nodes_in_tyre(tyre: &ast::Tyre, out: &mut Vec<ast::Hoon>) {
    for (_, hoon) in tyre {
        collect_cnts_nodes(hoon, out);
    }
}

fn collect_cnts_nodes_in_term_or_pair(term_or_pair: &ast::TermOrPair, out: &mut Vec<ast::Hoon>) {
    if let ast::TermOrPair::Pair(_, hoon) = term_or_pair {
        collect_cnts_nodes(hoon, out);
    }
}

fn collect_cnts_nodes_in_term_or_tune(term_or_tune: &ast::TermOrTune, out: &mut Vec<ast::Hoon>) {
    if let ast::TermOrTune::Tune(tune) = term_or_tune {
        collect_cnts_nodes_in_tune(tune, out);
    }
}

fn collect_cnts_nodes_in_tune(tune: &ast::Tune, out: &mut Vec<ast::Hoon>) {
    for hoon in tune.0.values().flatten() {
        collect_cnts_nodes(hoon, out);
    }
    for hoon in &tune.1 {
        collect_cnts_nodes(hoon, out);
    }
}

fn collect_cnts_nodes_in_spec(spec: &ast::Spec, out: &mut Vec<ast::Hoon>) {
    use ast::Spec::*;

    match spec {
        Base(_) | Leaf(_, _) | Like(_, _) | Loop(_) => {}
        Dbug(_, spec) | Made(_, spec) | Name(_, spec) | Over(_, spec) | BucLus(_, spec) => {
            collect_cnts_nodes_in_spec(spec, out)
        }
        Make(hoon, specs) => {
            collect_cnts_nodes(hoon, out);
            for spec in specs {
                collect_cnts_nodes_in_spec(spec, out);
            }
        }
        BucGar(a, b) | BucGal(a, b) | BucHep(a, b) | BucKet(a, b) | BucPat(a, b) => {
            collect_cnts_nodes_in_spec(a, out);
            collect_cnts_nodes_in_spec(b, out);
        }
        BucBuc(spec, map)
        | BucDot(spec, map)
        | BucFas(spec, map)
        | BucTic(spec, map)
        | BucZap(spec, map) => {
            collect_cnts_nodes_in_spec(spec, out);
            for spec in map.values() {
                collect_cnts_nodes_in_spec(spec, out);
            }
        }
        BucBar(spec, hoon) | BucPam(spec, hoon) => {
            collect_cnts_nodes_in_spec(spec, out);
            collect_cnts_nodes(hoon, out);
        }
        BucCab(hoon) | BucMic(hoon) => collect_cnts_nodes(hoon, out),
        BucCol(spec, specs) | BucCen(spec, specs) | BucWut(spec, specs) => {
            collect_cnts_nodes_in_spec(spec, out);
            for spec in specs {
                collect_cnts_nodes_in_spec(spec, out);
            }
        }
        BucSig(hoon, spec) => {
            collect_cnts_nodes(hoon, out);
            collect_cnts_nodes_in_spec(spec, out);
        }
        BucTis(skin, spec) => {
            collect_cnts_nodes_in_skin(skin, out);
            collect_cnts_nodes_in_spec(spec, out);
        }
    }
}

fn collect_cnts_nodes_in_skin(skin: &ast::Skin, out: &mut Vec<ast::Hoon>) {
    use ast::Skin::*;

    match skin {
        Term(_) | Base(_) | Leaf(_, _) | Wash(_) => {}
        Cell(a, b) => {
            collect_cnts_nodes_in_skin(a, out);
            collect_cnts_nodes_in_skin(b, out);
        }
        Dbug(_, skin) | Name(_, skin) | Over(_, skin) => {
            collect_cnts_nodes_in_skin(skin, out);
        }
        Spec(spec, skin) => {
            collect_cnts_nodes_in_spec(spec, out);
            collect_cnts_nodes_in_skin(skin, out);
        }
    }
}

fn collect_cnts_nodes_in_type(typ: &ast::Type, out: &mut Vec<ast::Hoon>) {
    use ast::Type::*;

    match typ {
        NounExpr | Void | ParsedAtom(_, _) => {}
        Cell(a, b) => {
            collect_cnts_nodes_in_type(a, out);
            collect_cnts_nodes_in_type(b, out);
        }
        Core(a, coil) => {
            collect_cnts_nodes_in_type(a, out);
            collect_cnts_nodes_in_coil(coil, out);
        }
        Face(_, typ) => collect_cnts_nodes_in_type(typ, out),
        Fork(types) => {
            for typ in types {
                collect_cnts_nodes_in_type(typ, out);
            }
        }
        Hint((a, _), b) => {
            collect_cnts_nodes_in_type(a, out);
            collect_cnts_nodes_in_type(b, out);
        }
        Hold(typ, hoon) => {
            collect_cnts_nodes_in_type(typ, out);
            collect_cnts_nodes(hoon, out);
        }
    }
}

fn collect_cnts_nodes_in_coil(coil: &ast::Coil, out: &mut Vec<ast::Hoon>) {
    collect_cnts_nodes_in_type(&coil.q, out);
    collect_cnts_nodes_in_semi_noun_expr(&coil.r.0, out);
    for tome in coil.r.1.values() {
        collect_cnts_nodes_in_tome(tome, out);
    }
}

fn collect_cnts_nodes_in_woof(woof: &ast::Woof, out: &mut Vec<ast::Hoon>) {
    if let ast::Woof::Hoon(hoon) = woof {
        collect_cnts_nodes(hoon, out);
    }
}

fn collect_cnts_nodes_in_semi_noun_expr(expr: &ast::SemiNounExpr, out: &mut Vec<ast::Hoon>) {
    collect_cnts_nodes_in_stencil(&expr.0, out);
}

fn collect_cnts_nodes_in_stencil(stencil: &ast::Stencil, out: &mut Vec<ast::Hoon>) {
    match stencil {
        ast::Stencil::Half { left, rite } => {
            collect_cnts_nodes_in_stencil(left, out);
            collect_cnts_nodes_in_stencil(rite, out);
        }
        ast::Stencil::Full { blocks: _ } => {}
        ast::Stencil::Lazy { resolve, .. } => {
            collect_cnts_nodes_in_spec(&resolve.0, out);
            collect_cnts_nodes_in_spec(&resolve.1, out);
        }
    }
}

fn collect_cnts_nodes_in_manx(manx: &ast::Manx, out: &mut Vec<ast::Hoon>) {
    collect_cnts_nodes_in_marx(&manx.g, out);
    collect_cnts_nodes_in_marl(&manx.c, out);
}

fn collect_cnts_nodes_in_marl(marl: &ast::Marl, out: &mut Vec<ast::Hoon>) {
    for tuna in marl {
        match tuna {
            ast::Tuna::Manx(manx) => collect_cnts_nodes_in_manx(manx, out),
            ast::Tuna::TunaTail(tail) => collect_cnts_nodes_in_tuna_tail(tail, out),
        }
    }
}

fn collect_cnts_nodes_in_tuna_tail(tail: &ast::TunaTail, out: &mut Vec<ast::Hoon>) {
    match tail {
        ast::TunaTail::Tape(hoon)
        | ast::TunaTail::Manx(hoon)
        | ast::TunaTail::Marl(hoon)
        | ast::TunaTail::Call(hoon) => collect_cnts_nodes(hoon, out),
    }
}

fn collect_cnts_nodes_in_marx(marx: &ast::Marx, out: &mut Vec<ast::Hoon>) {
    collect_cnts_nodes_in_mart(&marx.a, out);
}

fn collect_cnts_nodes_in_mart(mart: &ast::Mart, out: &mut Vec<ast::Hoon>) {
    for (_, beers) in mart {
        for beer in beers {
            collect_cnts_nodes_in_beer(beer, out);
        }
    }
}

fn collect_cnts_nodes_in_beer(beer: &ast::Beer, out: &mut Vec<ast::Hoon>) {
    if let ast::Beer::Hoon(hoon) = beer {
        collect_cnts_nodes(hoon, out);
    }
}

fn collect_hoon_variants_in_type(typ: &ast::Type, counts: &mut HashMap<String, usize>) {
    use ast::Type::*;

    match typ {
        NounExpr | Void | ParsedAtom(_, _) => {}
        Cell(a, b) => {
            collect_hoon_variants_in_type(a, counts);
            collect_hoon_variants_in_type(b, counts);
        }
        Core(typ, coil) => {
            collect_hoon_variants_in_type(typ, counts);
            collect_hoon_variants_in_coil(coil, counts);
        }
        Face(_, typ) => collect_hoon_variants_in_type(typ, counts),
        Fork(types) => {
            for typ in types {
                collect_hoon_variants_in_type(typ, counts);
            }
        }
        Hint((typ, _), other) => {
            collect_hoon_variants_in_type(typ, counts);
            collect_hoon_variants_in_type(other, counts);
        }
        Hold(typ, hoon) => {
            collect_hoon_variants_in_type(typ, counts);
            collect_hoon_variants(hoon, counts);
        }
    }
}

fn collect_hoon_variants_in_coil(coil: &ast::Coil, counts: &mut HashMap<String, usize>) {
    collect_hoon_variants_in_type(&coil.q, counts);
    collect_hoon_variants_in_semi_noun_expr(&coil.r.0, counts);
    for tome in coil.r.1.values() {
        collect_hoon_variants_in_tome(tome, counts);
    }
}

fn collect_hoon_variants_in_semi_noun_expr(
    expr: &ast::SemiNounExpr,
    counts: &mut HashMap<String, usize>,
) {
    collect_hoon_variants_in_stencil(&expr.0, counts);
}

fn collect_hoon_variants_in_stencil(stencil: &ast::Stencil, counts: &mut HashMap<String, usize>) {
    match stencil {
        ast::Stencil::Half { left, rite } => {
            collect_hoon_variants_in_stencil(left, counts);
            collect_hoon_variants_in_stencil(rite, counts);
        }
        ast::Stencil::Full { blocks: _ } => {}
        ast::Stencil::Lazy { resolve, .. } => {
            collect_hoon_variants_in_spec(&resolve.0, counts);
            collect_hoon_variants_in_spec(&resolve.1, counts);
        }
    }
}

fn collect_hoon_variants_in_manx(manx: &ast::Manx, counts: &mut HashMap<String, usize>) {
    collect_hoon_variants_in_marx(&manx.g, counts);
    collect_hoon_variants_in_marl(&manx.c, counts);
}

fn collect_hoon_variants_in_marx(marx: &ast::Marx, counts: &mut HashMap<String, usize>) {
    collect_hoon_variants_in_mart(&marx.a, counts);
}

fn collect_hoon_variants_in_mart(mart: &ast::Mart, counts: &mut HashMap<String, usize>) {
    for (_, beers) in mart {
        for beer in beers {
            collect_hoon_variants_in_beer(beer, counts);
        }
    }
}

fn collect_hoon_variants_in_beer(beer: &ast::Beer, counts: &mut HashMap<String, usize>) {
    if let ast::Beer::Hoon(hoon) = beer {
        collect_hoon_variants(hoon, counts);
    }
}

fn collect_hoon_variants_in_woof(woof: &ast::Woof, counts: &mut HashMap<String, usize>) {
    if let ast::Woof::Hoon(hoon) = woof {
        collect_hoon_variants(hoon, counts);
    }
}

fn collect_hoon_variants_in_marl(marl: &ast::Marl, counts: &mut HashMap<String, usize>) {
    for tuna in marl {
        collect_hoon_variants_in_tuna(tuna, counts);
    }
}

fn collect_hoon_variants_in_tuna(tuna: &ast::Tuna, counts: &mut HashMap<String, usize>) {
    match tuna {
        ast::Tuna::Manx(manx) => collect_hoon_variants_in_manx(manx, counts),
        ast::Tuna::TunaTail(tail) => collect_hoon_variants_in_tuna_tail(tail, counts),
    }
}

fn collect_hoon_variants_in_tuna_tail(tail: &ast::TunaTail, counts: &mut HashMap<String, usize>) {
    match tail {
        ast::TunaTail::Tape(hoon)
        | ast::TunaTail::Manx(hoon)
        | ast::TunaTail::Marl(hoon)
        | ast::TunaTail::Call(hoon) => collect_hoon_variants(hoon, counts),
    }
}

fn inventory_hoon_variants(
    path: &Path,
    deps_dir: &Path,
) -> Result<HashMap<String, usize>, Box<dyn std::error::Error>> {
    let hoon = parse_native_hoon(path, deps_dir)?;
    let mut counts = HashMap::new();
    collect_hoon_variants(&hoon, &mut counts);
    Ok(counts)
}

fn sorted_variant_diffs(
    base: &HashMap<String, usize>,
    target: &HashMap<String, usize>,
) -> Vec<(String, i64, usize, usize)> {
    let mut keys = BTreeSet::new();
    keys.extend(base.keys().cloned());
    keys.extend(target.keys().cloned());

    let mut diffs = Vec::new();
    for key in keys {
        let base_count = base.get(&key).copied().unwrap_or(0);
        let target_count = target.get(&key).copied().unwrap_or(0);
        if base_count != target_count {
            let diff = target_count as i64 - base_count as i64;
            diffs.push((key, diff, base_count, target_count));
        }
    }

    diffs.sort_by(|a, b| b.1.abs().cmp(&a.1.abs()).then_with(|| a.0.cmp(&b.0)));
    diffs
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
    let _permit = test_permit().await;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let log_capture = init_logging(true);
    log_capture.clear();
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

    if regular_jam != primed_jam {
        let logs = log_capture.take_string();
        let mut report = String::new();
        report.push_str("primed build output mismatch\n");
        for line in logs.lines() {
            if line.contains("prime-dir: hoon")
                || line.contains("prime-dir: using parsed hoon")
                || line.contains("prime-native:")
                || line.contains("prime-dir: hash collision")
            {
                report.push_str(line);
                report.push('\n');
            }
        }
        if report.lines().count() <= 1 {
            report.push_str("no prime-dir mismatch logs captured\n");
        }
        let mut regular_slab: NounSlab<NockJammer> = NounSlab::new();
        let mut primed_slab: NounSlab<NockJammer> = NounSlab::new();
        match (
            regular_slab.cue_into(Bytes::from(regular_jam.clone())),
            primed_slab.cue_into(Bytes::from(primed_jam.clone())),
        ) {
            (Ok(regular_root), Ok(primed_root)) => {
                if let Some(mismatch) = find_mismatch_axis(regular_root, primed_root, 1) {
                    report.push_str(&format!("jam mismatch axis: {}\n", mismatch.axis));
                    report.push_str(&format!(
                        "jam expected:\n{}\n",
                        print_noun(&mismatch.expected, 20, 0)
                    ));
                    report.push_str(&format!(
                        "jam actual:\n{}\n",
                        print_noun(&mismatch.actual, 20, 0)
                    ));
                    if let Some(parent_axis) = mismatch.parent_axis {
                        report.push_str(&format!("jam parent axis: {}\n", parent_axis));
                        if let (Some(parent_expected), Some(parent_actual)) =
                            (mismatch.parent_expected, mismatch.parent_actual)
                        {
                            report.push_str(&format!(
                                "jam parent expected:\n{}\n",
                                print_noun(&parent_expected, 20, 0)
                            ));
                            report.push_str(&format!(
                                "jam parent actual:\n{}\n",
                                print_noun(&parent_actual, 20, 0)
                            ));
                        }
                    }
                } else {
                    report.push_str("jam mismatch, but no differing axis found\n");
                }
            }
            (Err(err), _) => {
                report.push_str(&format!("failed to cue regular jam: {err}\n"));
            }
            (_, Err(err)) => {
                report.push_str(&format!("failed to cue primed jam: {err}\n"));
            }
        }
        return Err(io::Error::new(io::ErrorKind::Other, report).into());
    }
    Ok(())
}

#[test]
fn native_parser_parses_markdown_hoon() -> Result<(), Box<dyn std::error::Error>> {
    let root = repo_hoon_dir()?;
    let path = markdown_hoon_path()?;
    parse_native_ast(&path, &root)?;
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
    let _permit = test_permit().await;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let log_capture = init_logging(true);
    let deps_dir = repo_hoon_dir()?;
    let entries = kernel_entries(&deps_dir)?;
    let first_entry = entries
        .first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no kernel entries defined"))?;
    let mut native_asts = collect_native_asts(&deps_dir, first_entry)?;
    let mut tested = Vec::new();

    for entry in entries {
        ensure_entry_ast(&mut native_asts, &entry, &deps_dir)?;

        let regular_home = temp_out_dir("nockapp-home-regular")?;
        let primed_home = temp_out_dir("nockapp-home-primed")?;

        let regular_out_dir = temp_out_dir("hoonc-regular")?;
        let primed_out_dir = temp_out_dir("hoonc-primed")?;

        println!("testing kernel: {}", entry.display());
        log_capture.clear();

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

        if regular_jam != primed_jam {
            let logs = log_capture.take_string();
            let mut report = String::new();
            report.push_str(&format!("kernel jam mismatch for {}\n", entry.display()));
            report.push_str(&format!(
                "regular_out_dir: {}\nprimed_out_dir: {}\n",
                regular_out_dir.display(),
                primed_out_dir.display()
            ));
            for line in logs.lines() {
                if line.contains("prime-dir: hoon")
                    || line.contains("prime-dir: using parsed hoon")
                    || line.contains("prime-native:")
                    || line.contains("prime-dir: hash collision")
                    || line.contains("parse-dir: hash collision")
                {
                    report.push_str(line);
                    report.push('\n');
                }
            }
            if report.lines().count() <= 3 {
                report.push_str("no prime-dir mismatch logs captured\n");
            }
            let mut regular_slab: NounSlab<NockJammer> = NounSlab::new();
            let mut primed_slab: NounSlab<NockJammer> = NounSlab::new();
            match (
                regular_slab.cue_into(Bytes::from(regular_jam.clone())),
                primed_slab.cue_into(Bytes::from(primed_jam.clone())),
            ) {
                (Ok(regular_root), Ok(primed_root)) => {
                    if let Some(mismatch) = find_mismatch_axis(regular_root, primed_root, 1) {
                        report.push_str(&format!("jam mismatch axis: {}\n", mismatch.axis));
                        report.push_str(&format!(
                            "jam expected:\n{}\n",
                            print_noun(&mismatch.expected, 20, 0)
                        ));
                        report.push_str(&format!(
                            "jam actual:\n{}\n",
                            print_noun(&mismatch.actual, 20, 0)
                        ));
                        if let Some(raw_mismatch) =
                            find_mismatch_axis_raw(regular_root, primed_root, 1)
                        {
                            report.push_str(&format!(
                                "jam mismatch axis (with dbug): {}\n",
                                raw_mismatch.axis
                            ));
                            if let Some(desc) =
                                describe_nearest_dbug_with_excerpt(regular_root, raw_mismatch.axis)
                            {
                                report.push_str(&format!("dbug expected: {desc}\n"));
                            }
                            if let Some(desc) =
                                describe_nearest_dbug_with_excerpt(primed_root, raw_mismatch.axis)
                            {
                                report.push_str(&format!("dbug actual:   {desc}\n"));
                            }
                        }
                        if let Some(parent_axis) = mismatch.parent_axis {
                            report.push_str(&format!("jam parent axis: {}\n", parent_axis));
                            if let (Some(parent_expected), Some(parent_actual)) =
                                (mismatch.parent_expected, mismatch.parent_actual)
                            {
                                report.push_str(&format!(
                                    "jam parent expected:\n{}\n",
                                    print_noun(&parent_expected, 20, 0)
                                ));
                                report.push_str(&format!(
                                    "jam parent actual:\n{}\n",
                                    print_noun(&parent_actual, 20, 0)
                                ));
                            }
                        }
                    } else {
                        report.push_str("jam mismatch, but no differing axis found\n");
                    }
                }
                (Err(err), _) => {
                    report.push_str(&format!("failed to cue regular jam: {err}\n"));
                }
                (_, Err(err)) => {
                    report.push_str(&format!("failed to cue primed jam: {err}\n"));
                }
            }
            return Err(io::Error::new(io::ErrorKind::Other, report).into());
        }
        tested.push(entry);
    }

    assert!(
        !tested.is_empty(),
        "no kernel entries eligible for primed parse-cache test"
    );
    Ok(())
}

#[tokio::test]
async fn primed_parse_cache_primes_native_ztd_eight() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let log_capture = init_logging(false);

    let deps_dir = repo_hoon_dir()?;
    let entry = resolve_hoon_path(&deps_dir, KERNEL_ENTRIES[0])?;
    let target = resolve_hoon_path(&deps_dir, "common/ztd/eight.hoon")?;
    let subset = collect_native_asts_for_paths(&deps_dir, &[entry.clone(), target.clone()])?;

    let attempt = prime_only_with_subset(&entry, &deps_dir, &subset, log_capture).await?;
    if attempt.failed {
        let variants = inventory_hoon_variants(&target, &deps_dir)?;
        println!(
            "hoon variants for {}: {}",
            target.display(),
            format_variant_inventory(&variants, 12)
        );
    }

    assert!(
        !attempt.failed,
        "prime-only failed for {}",
        target.display()
    );
    assert!(
        attempt.pc_size.unwrap_or(0) >= 1,
        "expected parse cache size >= 1, got {:?}",
        attempt.pc_size
    );
    Ok(())
}

#[tokio::test]
#[ignore]
async fn primed_parse_cache_primes_native_ztd_eight_no_dbug(
) -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let log_capture = init_logging(false);

    let deps_dir = repo_hoon_dir()?;
    let entry = resolve_hoon_path(&deps_dir, KERNEL_ENTRIES[0])?;
    let target = resolve_hoon_path(&deps_dir, "common/ztd/eight.hoon")?;
    let subset = collect_native_asts_for_paths_with_dbug(
        &deps_dir,
        &[entry.clone(), target.clone()],
        false,
    )?;

    let attempt = prime_only_with_subset(&entry, &deps_dir, &subset, log_capture).await?;
    if attempt.failed {
        let variants = inventory_hoon_variants(&target, &deps_dir)?;
        println!(
            "hoon variants for {}: {}",
            target.display(),
            format_variant_inventory(&variants, 12)
        );
    }

    assert!(
        !attempt.failed,
        "prime-only failed for {} with dbug disabled",
        target.display()
    );
    assert!(
        attempt.pc_size.unwrap_or(0) >= 1,
        "expected parse cache size >= 1, got {:?}",
        attempt.pc_size
    );
    Ok(())
}

#[tokio::test]
async fn primed_parse_cache_builds_with_native_ztd_eight() -> Result<(), Box<dyn std::error::Error>>
{
    disable_metrics();
    let _permit = test_permit().await;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let log_capture = init_logging(false);

    let deps_dir = repo_hoon_dir()?;
    let entry = resolve_hoon_path(&deps_dir, KERNEL_ENTRIES[0])?;
    let target = resolve_hoon_path(&deps_dir, "common/ztd/eight.hoon")?;
    let subset = collect_native_asts_for_paths(&deps_dir, &[entry.clone(), target.clone()])?;

    let attempt = prime_with_subset(&entry, &deps_dir, &subset, log_capture).await?;
    if attempt.failed {
        let variants = inventory_hoon_variants(&target, &deps_dir)?;
        println!(
            "hoon variants for {}: {}",
            target.display(),
            format_variant_inventory(&variants, 12)
        );
    }

    assert!(
        !attempt.failed,
        "primed build failed for {} with native ASTs injected",
        target.display()
    );
    assert!(
        attempt.pc_size.unwrap_or(0) >= 1,
        "expected parse cache size >= 1, got {:?}",
        attempt.pc_size
    );
    Ok(())
}

async fn assert_native_ast_matches_hoonc_parse(
    target: &PathBuf,
    deps_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_slab = parse_hoon_with_hoonc(&target, &deps_dir).await?;
    let state_noun = unsafe { *state_slab.root() };
    let pc = parse_cache_from_state(state_noun)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let target_path = entry_path_for_hoon(&target, &deps_dir)?;
    let (_, pil, _deps) = map_find_entry_by_path(pc, &target_path)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("parse cache missing {}", target.display()),
            )
        })?;
    let hoonc_hoon = pile_hoon(pil).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    let native_hoon = parse_native_hoon_with_dbug(&target, &deps_dir, true)?;
    let mut slab = NounSlab::new();
    let native_noun = hoon_to_noun(&mut slab, &native_hoon);

    let mut hoonc_clean_slab = NounSlab::new();
    let hoonc_clean = strip_dbug_tree(&mut hoonc_clean_slab, hoonc_hoon);
    let mut native_clean_slab = NounSlab::new();
    let native_clean = strip_dbug_tree(&mut native_clean_slab, native_noun);

    let mut printed = false;
    if diff_noun(&hoonc_clean, &native_clean, &mut printed).is_err() {
        if let Some(mismatch) = find_mismatch_axis(hoonc_clean, native_clean, 1) {
            println!("mismatch axis: {}", mismatch.axis);
            println!(
                "expected@{}: {}",
                mismatch.axis,
                print_noun(&mismatch.expected, 20, 0)
            );
            println!(
                "actual@{}:   {}",
                mismatch.axis,
                print_noun(&mismatch.actual, 20, 0)
            );
            if let Some(parent_axis) = mismatch.parent_axis {
                if let (Some(expected), Some(actual)) =
                    (mismatch.parent_expected, mismatch.parent_actual)
                {
                    println!(
                        "expected parent@{parent_axis}: {}",
                        print_noun(&expected, 12, 0)
                    );
                    println!(
                        "actual parent@{parent_axis}:   {}",
                        print_noun(&actual, 12, 0)
                    );
                }
            }
        }
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("native AST mismatched hoonc parse for {}", target.display()),
        )
        .into());
    }

    let mut printed_raw = false;
    if diff_noun(&hoonc_hoon, &native_noun, &mut printed_raw).is_err() {
        println!(
            "note: native vs hoonc differs in dbug spot data for {}",
            target.display()
        );
    }

    Ok(())
}

async fn assert_native_ast_matches_hoonc_parse_with_dbug(
    target: &PathBuf,
    deps_dir: &PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let state_slab = parse_hoon_with_hoonc(&target, &deps_dir).await?;
    let state_noun = unsafe { *state_slab.root() };
    let pc = parse_cache_from_state(state_noun)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let target_path = entry_path_for_hoon(&target, &deps_dir)?;
    let (_, pil, _deps) = map_find_entry_by_path(pc, &target_path)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("parse cache missing {}", target.display()),
            )
        })?;
    let hoonc_hoon = pile_hoon(pil).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    let native_hoon = parse_native_hoon_with_dbug(&target, &deps_dir, true)?;
    let mut slab = NounSlab::new();
    let native_noun = hoon_to_noun(&mut slab, &native_hoon);

    let mut hoonc_clean_slab = NounSlab::new();
    let hoonc_clean = strip_dbug_tree(&mut hoonc_clean_slab, hoonc_hoon);
    let mut native_clean_slab = NounSlab::new();
    let native_clean = strip_dbug_tree(&mut native_clean_slab, native_noun);

    let mut printed = false;
    if diff_noun(&hoonc_clean, &native_clean, &mut printed).is_err() {
        if let Some(mismatch) = find_mismatch_axis(hoonc_clean, native_clean, 1) {
            println!("mismatch axis: {}", mismatch.axis);
            println!(
                "expected@{}: {}",
                mismatch.axis,
                print_noun(&mismatch.expected, 20, 0)
            );
            println!(
                "actual@{}:   {}",
                mismatch.axis,
                print_noun(&mismatch.actual, 20, 0)
            );
            if let Some(parent_axis) = mismatch.parent_axis {
                if let (Some(expected), Some(actual)) =
                    (mismatch.parent_expected, mismatch.parent_actual)
                {
                    println!(
                        "expected parent@{parent_axis}: {}",
                        print_noun(&expected, 12, 0)
                    );
                    println!(
                        "actual parent@{parent_axis}:   {}",
                        print_noun(&actual, 12, 0)
                    );
                }
            }
        }
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("native AST mismatched hoonc parse for {}", target.display()),
        )
        .into());
    }

    if slab_noun_equality(&hoonc_hoon, &native_noun) {
        return Ok(());
    }

    if let Some(mismatch) = find_mismatch_axis_raw(hoonc_hoon, native_noun, 1) {
        println!("dbug mismatch axis: {}", mismatch.axis);
        println!(
            "expected@{}: {}",
            mismatch.axis,
            print_noun(&mismatch.expected, 20, 0)
        );
        println!(
            "actual@{}:   {}",
            mismatch.axis,
            print_noun(&mismatch.actual, 20, 0)
        );
        if let Some(parent_axis) = mismatch.parent_axis {
            if let (Some(expected), Some(actual)) =
                (mismatch.parent_expected, mismatch.parent_actual)
            {
                println!(
                    "expected parent@{parent_axis}: {}",
                    print_noun(&expected, 12, 0)
                );
                println!(
                    "actual parent@{parent_axis}:   {}",
                    print_noun(&actual, 12, 0)
                );
            }
        }
        if let Some(desc) = describe_nearest_dbug(hoonc_hoon, mismatch.axis) {
            println!("expected nearest dbug: {desc}");
        }
        if let Some(desc) = describe_nearest_dbug(native_noun, mismatch.axis) {
            println!("actual nearest dbug:   {desc}");
        }
    }
    let mut path = Vec::new();
    if let Some(mismatch) = find_mismatch_path_raw(hoonc_hoon, native_noun, &mut path) {
        let path_bits = format_path_bits(&mismatch.path);
        println!(
            "dbug mismatch path len={} bits={}",
            mismatch.path.len(),
            path_bits
        );
        println!("expected@path: {}", print_noun(&mismatch.expected, 20, 0));
        println!("actual@path:   {}", print_noun(&mismatch.actual, 20, 0));
        if let Some((_last, parent_path)) = mismatch.path.split_last() {
            if let (Some(expected), Some(actual)) = (
                noun_at_path(hoonc_hoon, parent_path),
                noun_at_path(native_noun, parent_path),
            ) {
                println!("expected parent@path: {}", print_noun(&expected, 12, 0));
                println!("actual parent@path:   {}", print_noun(&actual, 12, 0));
            }
        }
        if let Some(desc) = describe_nearest_dbug_path(hoonc_hoon, &mismatch.path) {
            println!("expected nearest dbug: {desc}");
        }
        if let Some(desc) = describe_nearest_dbug_path(native_noun, &mismatch.path) {
            println!("actual nearest dbug:   {desc}");
        }
    }

    let mut spot_path = Vec::new();
    if let Some(mismatch) = first_spot_mismatch(hoonc_hoon, native_noun, &mut spot_path) {
        println!(
            "dbug spot mismatch path len={} bits={}",
            mismatch.path.len(),
            mismatch
                .path
                .iter()
                .map(|b| if *b == 0 { '0' } else { '1' })
                .collect::<String>()
        );
        for (idx, spot) in mismatch.expected_spots.iter().enumerate() {
            println!("expected spot[{idx}]: {}", format_spot_line(spot));
        }
        for (idx, spot) in mismatch.actual_spots.iter().enumerate() {
            println!("actual spot[{idx}]:   {}", format_spot_line(spot));
        }
        println!(
            "expected node: {}",
            print_noun(&mismatch.expected_node, 6, 0)
        );
        println!("actual node:   {}", print_noun(&mismatch.actual_node, 6, 0));
    }

    Err(io::Error::new(
        io::ErrorKind::Other,
        format!(
            "native AST mismatched hoonc parse (dbug) for {}",
            target.display()
        ),
    )
    .into())
}

#[tokio::test]
async fn native_ast_matches_hoonc_parse_for_bridge() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;

    let deps_dir = repo_hoon_dir()?;
    let target_rel = std::env::var("HOONC_COMPARE_TARGET")
        .unwrap_or_else(|_| "apps/bridge/bridge.hoon".to_string());
    let target = resolve_hoon_path(&deps_dir, &target_rel)?;
    assert_native_ast_matches_hoonc_parse(&target, &deps_dir).await
}

#[tokio::test]
async fn native_ast_matches_hoonc_parse_for_target_dbug() -> Result<(), Box<dyn std::error::Error>>
{
    disable_metrics();
    let _permit = test_permit().await;

    let deps_dir = repo_hoon_dir()?;
    let target_rel = std::env::var("HOONC_COMPARE_DBUG_TARGET")
        .unwrap_or_else(|_| "common/zeke.hoon".to_string());
    let target = resolve_hoon_path(&deps_dir, &target_rel)?;
    assert_native_ast_matches_hoonc_parse_with_dbug(&target, &deps_dir).await
}

#[tokio::test]
async fn native_ast_matches_hoonc_parse_for_ztd_eight() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;

    let deps_dir = repo_hoon_dir()?;
    let target = resolve_hoon_path(&deps_dir, "common/ztd/eight.hoon")?;
    assert_native_ast_matches_hoonc_parse(&target, &deps_dir).await
}

#[tokio::test]
async fn native_ast_matches_hoonc_parse_for_ztd_one_dbug() -> Result<(), Box<dyn std::error::Error>>
{
    disable_metrics();
    let _permit = test_permit().await;

    let deps_dir = repo_hoon_dir()?;
    let target = resolve_hoon_path(&deps_dir, "common/ztd/one.hoon")?;
    assert_native_ast_matches_hoonc_parse_with_dbug(&target, &deps_dir).await
}

#[tokio::test]
async fn native_ast_matches_hoonc_parse_for_zose_dbug() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;

    let deps_dir = repo_hoon_dir()?;
    let target = resolve_hoon_path(&deps_dir, "common/zose.hoon")?;
    assert_native_ast_matches_hoonc_parse_with_dbug(&target, &deps_dir).await
}

#[tokio::test]
async fn native_ast_matches_hoonc_parse_for_markdown() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;

    let deps_dir = repo_hoon_dir()?;
    let target = resolve_hoon_path(&deps_dir, "common/markdown/markdown.hoon")?;
    assert_native_ast_matches_hoonc_parse(&target, &deps_dir).await
}

#[tokio::test]
async fn native_ast_matches_hoonc_parse_for_hoon_138_dbug() -> Result<(), Box<dyn std::error::Error>>
{
    disable_metrics();
    let _permit = test_permit().await;

    let deps_dir = repo_hoon_dir()?;
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../hoonc/hoon/hoon-138.hoon")
        .canonicalize()?;
    assert_native_ast_matches_hoonc_parse_with_dbug(&target, &deps_dir).await
}

#[test]
fn native_cnts_noun_matches_mold_for_ztd_eight() -> Result<(), Box<dyn std::error::Error>> {
    let deps_dir = repo_hoon_dir()?;
    let target = resolve_hoon_path(&deps_dir, "common/ztd/eight.hoon")?;
    let hoon = parse_native_hoon_with_dbug(&target, &deps_dir, false)?;

    let mut cnts_nodes = Vec::new();
    collect_cnts_nodes(&hoon, &mut cnts_nodes);

    if cnts_nodes.is_empty() {
        return Err(
            io::Error::new(io::ErrorKind::Other, "no %cnts nodes found in ztd/eight").into(),
        );
    }

    for (idx, node) in cnts_nodes.iter().enumerate() {
        let mut slab = NounSlab::new();
        let noun = hoon_to_noun(&mut slab, node);
        if let Err(err) = validate_cnts_noun(&noun) {
            let summary = match node {
                ast::Hoon::CenTis(wing, pairs) => {
                    format!("wing_len={} pairs_len={}", wing.len(), pairs.len())
                }
                _ => String::new(),
            };
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("cnts node {idx} invalid: {err} {summary}\nnode={node:?}",),
            )
            .into());
        }
    }

    Ok(())
}

#[tokio::test]
#[ignore]
async fn bisect_primed_parse_cache_failure() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let log_capture = init_logging(true);

    let deps_dir = repo_hoon_dir()?;
    let entry = deps_dir.join(KERNEL_ENTRIES[0]).canonicalize()?;
    let native_asts = collect_native_asts(&deps_dir, &entry)?;

    let failing = bisect_first_failing_path(&entry, &deps_dir, &native_asts, log_capture).await?;
    match failing {
        Some(path) => println!("first failing path: {}", path.display()),
        None => println!("no failing path found"),
    }
    Ok(())
}

#[tokio::test]
#[ignore]
async fn bisect_primed_parse_cache_jam_mismatch() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let log_capture = init_logging(true);

    let deps_dir = repo_hoon_dir()?;
    let entry = deps_dir.join(KERNEL_ENTRIES[0]).canonicalize()?;
    let native_asts = collect_native_asts(&deps_dir, &entry)?;

    let failing =
        bisect_first_jam_mismatch_path(&entry, &deps_dir, &native_asts, log_capture).await?;
    match failing {
        Some(path) => println!("first jam mismatch path: {}", path.display()),
        None => println!("no jam mismatch found"),
    }
    Ok(())
}

#[tokio::test]
#[ignore]
async fn prime_single_path_debug() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let log_capture = init_logging(true);

    let deps_dir = repo_hoon_dir()?;
    let entry_rel =
        std::env::var("HOONC_PRIME_ENTRY").unwrap_or_else(|_| KERNEL_ENTRIES[0].to_string());
    let entry = resolve_hoon_path(&deps_dir, &entry_rel)?;
    let target_rel = std::env::var("HOONC_PRIME_SINGLE_PATH")
        .unwrap_or_else(|_| "apps/bridge/bridge.hoon".to_string());
    let target = resolve_hoon_path(&deps_dir, &target_rel)?;

    let native_asts = collect_native_asts(&deps_dir, &entry)?;
    let subset = native_subset_for_paths(&entry, &native_asts, std::slice::from_ref(&target))?;
    let attempt = prime_with_subset(&entry, &deps_dir, &subset, log_capture).await?;

    println!(
        "prime single path entry={} target={} warned={} failed={} pc-size={:?}",
        entry.display(),
        target.display(),
        attempt.warned,
        attempt.failed,
        attempt.pc_size
    );
    Ok(())
}

#[tokio::test]
#[ignore]
async fn debug_native_ast_mismatch_for_entry() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }

    let deps_dir = repo_hoon_dir()?;
    let entry_rel =
        std::env::var("HOONC_PRIME_ENTRY").unwrap_or_else(|_| KERNEL_ENTRIES[0].to_string());
    let entry = resolve_hoon_path(&deps_dir, &entry_rel)?;

    let state_slab = parse_hoon_with_hoonc(&entry, &deps_dir).await?;
    let state_noun = unsafe { *state_slab.root() };
    let pc = parse_cache_from_state(state_noun)
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
    let entries =
        collect_parse_cache_entries(pc).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;

    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for entry in entries {
        let Some(path) = resolve_parse_cache_path(&entry.path, &deps_dir) else {
            continue;
        };
        if !is_hoon_path(&path) {
            continue;
        }
        let canonical = path.canonicalize()?;
        if !seen.insert(canonical.clone()) {
            continue;
        }
        paths.push((entry.path, canonical, entry.pil));
    }
    paths.sort_by(|a, b| a.0.cmp(&b.0));

    for (path_str, path, pil) in paths {
        let hoonc_hoon = pile_hoon(pil).map_err(|err| io::Error::new(io::ErrorKind::Other, err))?;
        let native_hoon = parse_native_hoon_with_dbug(&path, &deps_dir, true)?;
        let mut native_slab = NounSlab::new();
        let native_noun = hoon_to_noun(&mut native_slab, &native_hoon);

        let mut hoonc_clean_slab = NounSlab::new();
        let hoonc_clean = strip_dbug_tree(&mut hoonc_clean_slab, hoonc_hoon);
        let mut native_clean_slab = NounSlab::new();
        let native_clean = strip_dbug_tree(&mut native_clean_slab, native_noun);

        let mut printed = false;
        if diff_noun(&hoonc_clean, &native_clean, &mut printed).is_err() {
            if let Some(mismatch) = find_mismatch_axis(hoonc_clean, native_clean, 1) {
                println!("mismatch path: {}", path_str);
                println!("resolved path: {}", path.display());
                println!("mismatch axis: {}", mismatch.axis);
                println!(
                    "expected@{}: {}",
                    mismatch.axis,
                    print_noun(&mismatch.expected, 20, 0)
                );
                println!(
                    "actual@{}:   {}",
                    mismatch.axis,
                    print_noun(&mismatch.actual, 20, 0)
                );
                if let Some(parent_axis) = mismatch.parent_axis {
                    if let (Some(expected), Some(actual)) =
                        (mismatch.parent_expected, mismatch.parent_actual)
                    {
                        println!(
                            "expected parent@{parent_axis}: {}",
                            print_noun(&expected, 12, 0)
                        );
                        println!(
                            "actual parent@{parent_axis}:   {}",
                            print_noun(&actual, 12, 0)
                        );
                    }
                }
            }
            return Err(io::Error::new(
                io::ErrorKind::Other,
                format!("native AST mismatched hoonc parse for {}", path.display()),
            )
            .into());
        }
    }

    Ok(())
}

#[tokio::test]
#[ignore]
async fn enumerate_failing_primed_paths() -> Result<(), Box<dyn std::error::Error>> {
    disable_metrics();
    let _permit = test_permit().await;
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    let log_capture = init_logging(true);

    let deps_dir = repo_hoon_dir()?;
    let entry_rel =
        std::env::var("HOONC_PRIME_ENTRY").unwrap_or_else(|_| KERNEL_ENTRIES[0].to_string());
    let entry = resolve_hoon_path(&deps_dir, &entry_rel)?;
    let entry_path = entry.canonicalize()?;

    let native_asts = collect_native_asts(&deps_dir, &entry)?;
    let mut paths: Vec<PathBuf> = native_asts
        .keys()
        .filter(|path| **path != entry_path)
        .cloned()
        .collect();
    paths.sort_by(|a, b| a.to_string_lossy().cmp(&b.to_string_lossy()));

    let mut failing = Vec::new();
    for path in &paths {
        let subset = native_subset_for_paths(&entry, &native_asts, std::slice::from_ref(path))?;
        let attempt = prime_with_subset(&entry, &deps_dir, &subset, log_capture).await?;
        if attempt.failed {
            println!(
                "failing path: {} pc-size={:?}",
                path.display(),
                attempt.pc_size
            );
            failing.push(path.clone());
        }
    }

    println!("failing paths: {}", failing.len());
    Ok(())
}

#[test]
#[ignore]
fn debug_jam_mismatch_from_env() -> Result<(), Box<dyn std::error::Error>> {
    let expected_path = std::env::var("HOONC_JAM_EXPECTED")
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "HOONC_JAM_EXPECTED missing"))?;
    let actual_path = std::env::var("HOONC_JAM_ACTUAL")
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "HOONC_JAM_ACTUAL missing"))?;

    let expected_bytes = fs::read(&expected_path)?;
    let actual_bytes = fs::read(&actual_path)?;

    let mut expected_slab: NounSlab<NockJammer> = NounSlab::new();
    let mut actual_slab: NounSlab<NockJammer> = NounSlab::new();
    let expected_root = expected_slab.cue_into(Bytes::from(expected_bytes))?;
    let actual_root = actual_slab.cue_into(Bytes::from(actual_bytes))?;

    let strip_dbug = std::env::var("HOONC_JAM_STRIP_DBUG").is_ok();
    let (expected_root, actual_root) = if strip_dbug {
        let mut expected_clean_slab = NounSlab::new();
        let mut actual_clean_slab = NounSlab::new();
        (
            strip_dbug_tree(&mut expected_clean_slab, expected_root),
            strip_dbug_tree(&mut actual_clean_slab, actual_root),
        )
    } else {
        (expected_root, actual_root)
    };

    if let Some(mismatch) = find_mismatch_axis(expected_root, actual_root, 1) {
        println!("jam mismatch axis: {}", mismatch.axis);
        println!("jam expected:\n{}", print_noun(&mismatch.expected, 12, 0));
        println!("jam actual:\n{}", print_noun(&mismatch.actual, 12, 0));
        let mut ancestor_axis = mismatch.parent_axis;
        if let Some(raw_mismatch) = find_mismatch_axis_raw(expected_root, actual_root, 1) {
            println!("jam mismatch axis (with dbug): {}", raw_mismatch.axis);
            let axis_bits = format_axis_bits(raw_mismatch.axis);
            if !axis_bits.is_empty() {
                println!("jam mismatch axis bits (with dbug): {axis_bits}");
            }
            if let Some(desc) = describe_nearest_dbug_with_excerpt(expected_root, raw_mismatch.axis)
            {
                println!("dbug expected: {desc}");
            }
            if let Some(desc) = describe_nearest_dbug_with_excerpt(actual_root, raw_mismatch.axis) {
                println!("dbug actual:   {desc}");
            }
            if raw_mismatch.parent_axis.is_some() {
                ancestor_axis = raw_mismatch.parent_axis;
            }
        }
        if let Some(parent_axis) = ancestor_axis {
            println!("jam parent axis: {parent_axis}");
            if let (Some(expected), Some(actual)) =
                (mismatch.parent_expected, mismatch.parent_actual)
            {
                println!("jam parent expected:\n{}", print_noun(&expected, 10, 0));
                println!("jam parent actual:\n{}", print_noun(&actual, 10, 0));
            }
            let mut ancestor_axis = parent_axis;
            for depth in 1..=3 {
                ancestor_axis >>= 1;
                if ancestor_axis < 1 {
                    break;
                }
                if let Some(expected) = noun_at_axis(expected_root, ancestor_axis) {
                    println!(
                        "expected ancestor@{ancestor_axis} depth={depth}: {}",
                        print_noun(&expected, 10, 0)
                    );
                } else {
                    println!("expected ancestor@{ancestor_axis} depth={depth}: <missing>");
                }
                if let Some(actual) = noun_at_axis(actual_root, ancestor_axis) {
                    println!(
                        "actual ancestor@{ancestor_axis} depth={depth}:   {}",
                        print_noun(&actual, 10, 0)
                    );
                } else {
                    println!("actual ancestor@{ancestor_axis} depth={depth}:   <missing>");
                }
            }
        }
        let mut path = Vec::new();
        if let Some(path_mismatch) = find_mismatch_path_raw(expected_root, actual_root, &mut path) {
            let path_bits = format_path_bits(&path_mismatch.path);
            println!(
                "jam mismatch path len={} bits={}",
                path_mismatch.path.len(),
                path_bits
            );
            println!(
                "jam expected@path:\n{}",
                print_noun(&path_mismatch.expected, 12, 0)
            );
            println!(
                "jam actual@path:\n{}",
                print_noun(&path_mismatch.actual, 12, 0)
            );
            if let Some((_last, parent_path)) = path_mismatch.path.split_last() {
                if let Some(expected) = noun_at_path(expected_root, parent_path) {
                    println!(
                        "jam expected parent@path:\n{}",
                        print_noun(&expected, 10, 0)
                    );
                }
                if let Some(actual) = noun_at_path(actual_root, parent_path) {
                    println!("jam actual parent@path:\n{}", print_noun(&actual, 10, 0));
                }
            }
            if let Some(desc) = describe_nearest_dbug_path(expected_root, &path_mismatch.path) {
                println!("dbug expected (path): {desc}");
            }
            if let Some(desc) = describe_nearest_dbug_path(actual_root, &path_mismatch.path) {
                println!("dbug actual (path):   {desc}");
            }
        }
    } else {
        println!("jam mismatch, but no differing axis found");
    }

    Ok(())
}

#[test]
#[ignore]
fn compare_hoon_variant_inventory() -> Result<(), Box<dyn std::error::Error>> {
    let deps_dir = repo_hoon_dir()?;
    let target_rel = std::env::var("HOONC_VARIANT_TARGET")
        .unwrap_or_else(|_| "apps/bridge/bridge.hoon".to_string());
    let base_rel =
        std::env::var("HOONC_VARIANT_BASE").unwrap_or_else(|_| KERNEL_ENTRIES[0].to_string());
    let target = resolve_hoon_path(&deps_dir, &target_rel)?;
    let base = resolve_hoon_path(&deps_dir, &base_rel)?;

    let target_counts = inventory_hoon_variants(&target, &deps_dir)?;
    let base_counts = inventory_hoon_variants(&base, &deps_dir)?;
    let diffs = sorted_variant_diffs(&base_counts, &target_counts);

    println!("variant inventory target: {}", target.display());
    println!("variant inventory base:   {}", base.display());

    println!("variants only in target:");
    for (name, _, _, target_count) in diffs.iter().filter(|(_, _, b, _)| *b == 0) {
        println!("  {name}: {target_count}");
    }

    println!("variants only in base:");
    for (name, _, base_count, _) in diffs.iter().filter(|(_, _, _, t)| *t == 0) {
        println!("  {name}: {base_count}");
    }

    println!("variant count diffs (target - base):");
    for (name, diff, base_count, target_count) in diffs {
        if base_count != 0 && target_count != 0 {
            println!("  {name}: {diff} (base={base_count}, target={target_count})");
        }
    }

    Ok(())
}
