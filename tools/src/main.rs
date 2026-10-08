//! Generates the Claude Code session context: compact code map, per-crate pub API, live git state
//!
//! Writes:
//!   .claude/context/codemap.md      compact map, injected at session start (Tier 1)
//!   .claude/context/api/<crate>.md  full pub API signatures, read on demand (Tier 2)
//!
//! With `--print`, also prints the codemap plus live git/shader state to stdout
//! (used by the SessionStart hook in .claude/settings.json).

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
    sync::LazyLock,
    time::SystemTime,
};

use regex::Regex;

const CRATES: [&str; 5] = ["lava", "lava-macros", "core", "game", "tools"];
const GENERATED: [&str; 1] = ["lava/src/bindings.rs"];
const FN_NAMES_MAX_LINES: usize = 300;
const PRINT_BUDGET: usize = 16_000; // bytes of stdout, ~4k tokens

macro_rules! re {
    ($name:ident, $pat:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pat).unwrap());
    };
}

re!(TYPE_RE, r"^\s*pub\s+(struct|enum|trait|type|union)\s+(\w+)");
re!(
    FN_RE,
    r#"^\s*pub\s+(?:const\s+)?(?:async\s+)?(?:unsafe\s+)?(?:extern\s+"[^"]*"\s+)?fn\s+(\w+)"#
);
re!(
    PLUGIN_RE,
    r"^\s*impl\s+Plugin\s+for\s+(\w+)|^\s*pub\s+fn\s+(\w+)\s*\(\s*app\s*:\s*&mut\s+App\s*\)"
);
re!(MACRO_RE, r"^\s*macro_rules!\s+(\w+)");
re!(IMPL_RE, r"^\s*(?:unsafe\s+)?impl\b");
re!(FIELD_RE, r"^\s*pub\s+(\w+)\s*:\s*(.+?),?\s*$");
re!(STR_RE, r#""(?:\\.|[^"\\])*""#);
re!(CHAR_RE, r"'(?:\\.|[^'\\])'");
re!(PATH_DEP_RE, r"(?m)^([\w-]+)\s*=\s*\{[^}]*path\s*=");
re!(SHADER_ATTR_RE, r#"\[shader\("(\w+)"\)\]"#);
re!(CALL_RE, r"(\w+)\s*\(");
re!(PUSH_CONSTANT_RE, r"\[\[vk::push_constant\]\]\s*(\w+)");
re!(INCLUDE_RE, r#"(?m)^\s*#include\s+"([^"]+)""#);
re!(MACRO_DEF_RE, r"(?m)^#define\s+(\w+)\((\w+)\)((?:.*\\\n)+.*)");
re!(MACRO_CALL_RE, r"^\s*(\w+)\((\w+)\)\s*$");
re!(SIG_END_RE, r"[{;]\s*$|\bwhere\b");
re!(SIG_TRAIL_RE, r"\s*(\{|;)\s*$");
re!(WS_RE, r"\s+");
re!(TRAILING_WHERE_RE, r"\s+where$");
re!(CLOSE_RE, r",\s*([)>])");
re!(OPEN_RE, r"([(<])\s+");
re!(IMPL_HEADER_RE, r"^\n(?:unsafe\s+)?impl\b");

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .unwrap()
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap().display().to_string()
}

fn sh(root: &Path, args: &[&str]) -> String {
    Command::new(args[0])
        .args(&args[1..])
        .current_dir(root)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

fn read(p: &Path) -> String {
    String::from_utf8_lossy(&fs::read(p).unwrap_or_default()).into_owned()
}

fn rust_files(root: &Path, krate: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n != "target") {
                    walk(&p, out);
                }
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(&root.join(krate), &mut out);
    out.sort();
    out
}

/// `shaders/<subdir>/*.slang`, sorted by full path.
fn slang_files(root: &Path, subdir: Option<&str>) -> Vec<PathBuf> {
    let dirs: Vec<PathBuf> = match subdir {
        Some(s) => vec![root.join("shaders").join(s)],
        None => fs::read_dir(root.join("shaders"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect(),
    };
    let mut out: Vec<PathBuf> = dirs
        .iter()
        .flat_map(|d| fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "slang"))
        .collect();
    out.sort();
    out
}

fn mtime(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn source_hash(root: &Path) -> String {
    let mut h = DefaultHasher::new();
    let mut paths: Vec<PathBuf> = std::env::current_exe().into_iter().collect();
    paths.extend(CRATES.iter().flat_map(|c| rust_files(root, c)));
    paths.extend(slang_files(root, None));
    paths.extend(CRATES.iter().map(|c| root.join(c).join("Cargo.toml")));
    for p in paths {
        let Ok(m) = fs::metadata(&p) else { continue };
        (p, m.modified().ok(), m.len()).hash(&mut h);
    }
    format!("{:016x}", h.finish())
}

fn strip_code(line: &str) -> String {
    let s = STR_RE.replace_all(line, "\"\"");
    let s = CHAR_RE.replace_all(&s, "''");
    s.split("//").next().unwrap_or("").to_string()
}

fn braces(code: &str) -> i64 {
    code.matches('{').count() as i64 - code.matches('}').count() as i64
}

fn dedup(xs: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for x in xs {
        if !out.contains(x) {
            out.push(x.clone());
        }
    }
    out
}

// ---------------------------------------------------------------- Tier 1 ----

struct FileSummary {
    lines: usize,
    doc: String,
    types: Vec<String>,
    fns: Vec<String>,
    plugins: Vec<String>,
    macros: Vec<String>,
}

fn file_summary(path: &Path) -> FileSummary {
    let text = read(path);
    let lines: Vec<&str> = text.lines().collect();
    let doc = lines
        .iter()
        .map(|l| l.trim())
        .find(|l| l.starts_with("//!"))
        .map(|l| l[3..].trim().to_string())
        .unwrap_or_default();
    let mut s = FileSummary {
        lines: lines.len(),
        doc,
        types: vec![],
        fns: vec![],
        plugins: vec![],
        macros: vec![],
    };
    for line in &lines {
        if let Some(c) = TYPE_RE.captures(line) {
            s.types.push(c[2].to_string());
        } else if let Some(c) = PLUGIN_RE.captures(line) {
            s.plugins
                .push(c.get(1).or(c.get(2)).unwrap().as_str().to_string());
        } else if let Some(c) = FN_RE.captures(line) {
            s.fns.push(c[1].to_string());
        } else if let Some(c) = MACRO_RE.captures(line) {
            s.macros.push(format!("{}!", &c[1]));
        }
    }
    s
}

fn codemap_entry(rel: &str, s: &FileSummary, with_fns: bool, with_types: bool) -> String {
    let mut head = format!("- `{rel}` ({})", s.lines);
    if !s.doc.is_empty() {
        head += &format!(" — {}", s.doc);
    }
    if GENERATED.contains(&rel) {
        return head + " [GENERATED by lava/build.rs, do not edit]";
    }
    let mut parts = Vec::new();
    if !s.plugins.is_empty() {
        parts.push(format!("plugins: {}", dedup(&s.plugins).join(", ")));
    }
    if with_types && !s.types.is_empty() {
        parts.push(format!("types: {}", dedup(&s.types).join(", ")));
    }
    if !s.macros.is_empty() {
        parts.push(format!("macros: {}", dedup(&s.macros).join(", ")));
    }
    if with_fns && !s.fns.is_empty() && s.lines <= FN_NAMES_MAX_LINES {
        parts.push(format!("fns: {}", dedup(&s.fns).join(", ")));
    }
    if parts.is_empty() {
        head
    } else {
        format!("{head}\n    {}", parts.join(" | "))
    }
}

fn crate_deps(root: &Path) -> String {
    CRATES
        .iter()
        .map(|c| {
            let toml = read(&root.join(c).join("Cargo.toml"));
            let deps: Vec<&str> = PATH_DEP_RE
                .captures_iter(&toml)
                .map(|m| m.get(1).unwrap().as_str())
                .collect();
            let deps = if deps.is_empty() {
                "(none)".to_string()
            } else {
                deps.join(", ")
            };
            format!("{c} -> {deps}")
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn shader_table(root: &Path) -> (Vec<String>, String) {
    let mut rows = Vec::new();
    for f in slang_files(root, Some("passes")) {
        let text = read(&f);
        let include_dir = f.parent().unwrap().with_file_name("include");
        let includes: Vec<String> = INCLUDE_RE
            .captures_iter(&text)
            .map(|m| read(&include_dir.join(&m[1])))
            .collect();
        // Expands calls of included one-parameter macros, which can define entry points.
        let expanded = text
            .lines()
            .map(|line| {
                MACRO_CALL_RE
                    .captures(line)
                    .and_then(|call| {
                        let def = includes
                            .iter()
                            .flat_map(|include| MACRO_DEF_RE.captures_iter(include))
                            .find(|def| def[1] == call[1])?;
                        Some(def[3].replace(&format!("{}##", &def[2]), &call[2]).replace('\\', ""))
                    })
                    .unwrap_or_else(|| line.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n");
        let lines: Vec<&str> = expanded.lines().collect();
        let (mut entries, mut pc) = (Vec::new(), "?".to_string());
        for (i, line) in lines.iter().enumerate() {
            if let Some(m) = SHADER_ATTR_RE.captures(line) {
                for nxt in lines.iter().skip(i + 1).take(5) {
                    let s = nxt.trim();
                    if s.is_empty() || s.starts_with('[') {
                        continue;
                    }
                    if let Some(n) = CALL_RE.captures(s) {
                        entries.push(format!("{}:{}", &m[1], &n[1]));
                    }
                    break;
                }
            }
            if let Some(m) = PUSH_CONSTANT_RE.captures(line) {
                pc = m[1].to_string();
            }
        }
        // Passes sharing their push constants declare them in an include.
        if pc == "?" {
            pc = includes
                .iter()
                .find_map(|include| Some(PUSH_CONSTANT_RE.captures(include)?[1].to_string()))
                .unwrap_or(pc);
        }
        let name = f.file_name().unwrap().to_string_lossy();
        rows.push(format!("- {name}: pc `{pc}`; {}", entries.join(", ")));
    }
    let incs = slang_files(root, Some("include"))
        .iter()
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(", ");
    (rows, incs)
}

fn build_codemap(root: &Path, with_fns: bool, types_max_lines: Option<usize>) -> String {
    let mut out = vec![
        "# Code map (auto-generated by the `tools` crate)".to_string(),
        format!("Crate deps: {}", crate_deps(root)),
        String::new(),
    ];
    for c in CRATES {
        out.push(format!("## {c}"));
        for f in rust_files(root, c) {
            let s = file_summary(&f);
            let with_types = types_max_lines.is_none_or(|max| s.lines <= max);
            out.push(codemap_entry(&rel(root, &f), &s, with_fns, with_types));
        }
        out.push(String::new());
    }
    let (rows, incs) = shader_table(root);
    out.push(
        "## shaders/passes (each → generated `<PascalName>` pass in lava/src/bindings.rs)".into(),
    );
    out.extend(rows);
    out.push(format!("includes (shaders/include): {incs}"));
    out.join("\n") + "\n"
}

// ---------------------------------------------------------------- Tier 2 ----

fn clean_signature(sig: &str) -> String {
    let sig = sig.split(" where ").next().unwrap();
    let sig = SIG_TRAIL_RE.replace(sig, "");
    let sig = WS_RE.replace_all(&sig, " ");
    let sig = TRAILING_WHERE_RE.replace(&sig, "");
    let sig = CLOSE_RE.replace_all(&sig, "$1");
    OPEN_RE.replace_all(&sig, "$1").into_owned()
}

fn api_for_file(path: &Path) -> String {
    let text = read(path);
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut depth: i64 = 0;
    let mut impl_stack: Vec<i64> = Vec::new(); // depth at which each open impl started
    let mut struct_open: Option<i64> = None; // depth of an open pub struct body
    let mut i = 0;
    while i < lines.len() {
        let code = strip_code(lines[i]);
        let stripped = code.trim();

        if IMPL_RE.is_match(&code) && code.contains('{') {
            impl_stack.push(depth);
            out.push(format!("\n{}", stripped.trim_end_matches('{').trim()));
        } else if TYPE_RE.is_match(&code) {
            let sig = stripped.trim_end_matches('{').trim();
            out.push(format!("\n{sig}"));
            if stripped.ends_with('{') && sig.contains("struct") {
                struct_open = Some(depth);
            }
        } else if let Some(c) = struct_open
            .filter(|&d| depth == d + 1)
            .and_then(|_| FIELD_RE.captures(&code))
        {
            out.push(format!("    {}: {}", &c[1], &c[2]));
        } else if FN_RE.is_match(&code) {
            let mut sig = stripped.to_string();
            let mut j = i;
            while !SIG_END_RE.is_match(&sig) && j + 1 < lines.len() && j - i < 15 {
                j += 1;
                sig += " ";
                sig += strip_code(lines[j]).trim();
            }
            let indent = if impl_stack.is_empty() { "" } else { "  " };
            out.push(format!("{indent}{}", clean_signature(&sig)));
            for line in &lines[i..=j] {
                depth += braces(&strip_code(line));
            }
            i = j + 1;
            while impl_stack.last().is_some_and(|&d| depth <= d) {
                impl_stack.pop();
            }
            continue;
        }

        depth += braces(&code);
        if struct_open.is_some_and(|d| depth <= d) {
            struct_open = None;
        }
        while impl_stack.last().is_some_and(|&d| depth <= d) {
            impl_stack.pop();
        }
        i += 1;
    }

    // drop impl headers with no pub fns under them
    let cleaned: Vec<&str> = out
        .iter()
        .enumerate()
        .filter(|(idx, line)| {
            !IMPL_HEADER_RE.is_match(line) || out.get(idx + 1).is_some_and(|n| n.starts_with("  "))
        })
        .map(|(_, l)| l.as_str())
        .collect();
    cleaned.join("\n").trim().to_string()
}

fn build_api(root: &Path, krate: &str) -> String {
    let mut out = vec![
        format!("# {krate} pub API (auto-generated by the `tools` crate)"),
        String::new(),
    ];
    for f in rust_files(root, krate) {
        if f.file_name().is_some_and(|n| n == "build.rs") {
            continue;
        }
        let body = api_for_file(&f);
        if !body.is_empty() {
            out.extend([
                format!("## {}", rel(root, &f)),
                "```rust".into(),
                body,
                "```".into(),
                String::new(),
            ]);
        }
    }
    out.join("\n")
}

fn api_file_name(krate: &str) -> String {
    format!("{}.md", krate.rsplit('/').next().unwrap())
}

// ------------------------------------------------------------------ live ----

fn live_state(root: &Path) -> String {
    let mut out = vec!["## Live state".to_string()];
    out.push(format!(
        "branch: {}",
        sh(root, &["git", "branch", "--show-current"]).trim()
    ));
    let status = sh(root, &["git", "status", "--short"]);
    let status: Vec<&str> = status.lines().collect();
    if !status.is_empty() {
        out.push(format!("git status ({} changed):", status.len()));
        out.extend(status.iter().take(30).map(|l| format!("  {l}")));
        if status.len() > 30 {
            out.push(format!("  ... {} more", status.len() - 30));
        }
    }
    out.push("recent commits:".into());
    out.extend(
        sh(root, &["git", "log", "--oneline", "-5"])
            .lines()
            .map(|l| format!("  {l}")),
    );

    let bindings = root.join("lava/src/bindings.rs");
    match mtime(&bindings) {
        None => out.push(
            "WARNING: lava/src/bindings.rs missing — run `cargo build` to generate it.".into(),
        ),
        Some(gen_time) => {
            let stale: Vec<String> = ["passes", "include"]
                .iter()
                .flat_map(|d| slang_files(root, Some(d)))
                .filter(|s| mtime(s).is_some_and(|t| t > gen_time))
                .map(|s| s.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            if !stale.is_empty() {
                out.push(format!(
                    "NOTE: shaders newer than generated bindings.rs (cargo build regenerates): {}",
                    stale.join(", ")
                ));
            }
        }
    }
    out.join("\n") + "\n"
}

// ------------------------------------------------------------------ main ----

fn main() {
    let root = root();
    let ctx = root.join(".claude/context");
    fs::create_dir_all(ctx.join("api")).unwrap();
    let hash_file = ctx.join(".hash");
    let codemap_file = ctx.join("codemap.md");

    let h = source_hash(&root);
    if !codemap_file.exists() || fs::read_to_string(&hash_file).ok().as_deref() != Some(&h) {
        let mut codemap = build_codemap(&root, true, None);
        // keep the injected map within budget: drop fn names, then types of big files
        for (with_fns, max) in [(false, None), (false, Some(500)), (false, Some(0))] {
            if codemap.len() <= PRINT_BUDGET - 2500 {
                break;
            }
            codemap = build_codemap(&root, with_fns, max);
        }
        fs::write(&codemap_file, codemap).unwrap();
        for c in CRATES {
            fs::write(ctx.join("api").join(api_file_name(c)), build_api(&root, c)).unwrap();
        }
        fs::write(&hash_file, h).unwrap();
    }

    if std::env::args().any(|a| a == "--print") {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(read(&codemap_file).as_bytes()).unwrap();
        stdout
            .write_all(
                b"Full pub signatures + struct fields: .claude/context/api/<crate>.md \
                  (read the relevant one before grepping for APIs).\n\
                  If any of this is stale, or your change makes it stale, fix the source in this \
                  task: the file's `//!` line, CLAUDE.md, or tools/src/main.rs \
                  (never .claude/context/* by hand). Then rerun it.\n\n",
            )
            .unwrap();
        stdout.write_all(live_state(&root).as_bytes()).unwrap();
    }
}
