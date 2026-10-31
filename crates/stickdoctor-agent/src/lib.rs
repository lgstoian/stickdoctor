//! Machine-readable breadcrumbs for AI coding agents.
//!
//! This crate has no disk access and no UI. It reads the StickDoctor sources
//! and parses the `@sd` annotations described in `docs/AGENT_ANNOTATIONS.md`,
//! so agents and the test suite share one definition of "what the code says
//! about itself". [`validate`] is the false-green guard: it fails the build when
//! an annotation is malformed, a breadcrumb is missing, a cross-reference
//! dangles, or a test contains no assertion.
//!
//! @sd file module="crates/stickdoctor-agent/src/lib.rs" role="parses and validates @sd breadcrumbs" entry="scan" exit="validate"
//! @sd anchor name=agent.lib

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Version of the annotation schema enforced by [`validate`].
pub const SCHEMA_VERSION: &str = "1";

/// One parsed `@sd` annotation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Annotation {
    pub kind: String,
    pub file: String,
    pub line: usize,
    pub keys: Vec<(String, String)>,
}

impl Annotation {
    /// The value of `key`, if present.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.keys
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// A `fn` item discovered in the sources.
#[derive(Debug, Clone)]
pub struct FnDef {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub body: String,
}

/// A `#[test]` function discovered in the sources.
#[derive(Debug, Clone)]
pub struct TestFn {
    pub name: String,
    pub file: String,
    pub line: usize,
    pub body: String,
    pub should_panic: bool,
    pub ignored: bool,
    pub has_assert: bool,
}

/// A source file and its raw text (needed for rules that look at string
/// literals, which [`mask_source`] blanks out).
#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: String,
    pub raw: String,
}

/// Everything the annotations say about the workspace.
#[derive(Debug, Default, Clone)]
pub struct AgentMap {
    pub annotations: Vec<Annotation>,
    pub functions: Vec<FnDef>,
    pub tests: Vec<TestFn>,
    pub sources: Vec<SourceFile>,
    pub files: Vec<String>,
}

impl AgentMap {
    /// All annotations of one `kind`, in file order.
    pub fn annotations_of<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Annotation> {
        self.annotations.iter().filter(move |a| a.kind == kind)
    }
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// The annotation kinds understood by [`validate`].
pub const KINDS: &[&str] = &[
    "file",
    "flow",
    "invariant",
    "verify",
    "hazard",
    "anchor",
    "ref",
];

/// Keys accepted for `kind`. `None` means the kind is unknown.
pub fn allowed_keys(kind: &str) -> Option<&'static [&'static str]> {
    match kind {
        "file" => Some(&["module", "role", "entry", "exit", "depends"]),
        "flow" => Some(&["pipeline", "id", "step", "label", "next", "verify"]),
        "invariant" => Some(&["id", "text", "test"]),
        "verify" => Some(&["id", "fn", "proves", "negative_test"]),
        "hazard" => Some(&["text", "guard"]),
        "anchor" => Some(&["name", "text"]),
        "ref" => Some(&["name"]),
        _ => None,
    }
}

/// Keys that must be present for `kind`.
pub fn required_keys(kind: &str) -> &'static [&'static str] {
    match kind {
        "file" => &["module", "role"],
        "flow" => &["pipeline", "id", "step", "label"],
        "invariant" => &["id", "text", "test"],
        "verify" => &["id", "fn", "proves", "negative_test"],
        "hazard" => &["text"],
        "anchor" => &["name"],
        "ref" => &["name"],
        _ => &[],
    }
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

/// The repository root that contains this crate (`crates/stickdoctor-agent`).
pub fn workspace_root() -> Option<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

/// Scan the workspace that contains this crate (the repository root).
pub fn scan_workspace() -> Result<AgentMap, String> {
    let root = workspace_root().ok_or_else(|| "cannot locate the workspace root".to_string())?;
    scan(&root)
}

/// Scan every `.rs` file under `root`, skipping build output.
pub fn scan(root: &Path) -> Result<AgentMap, String> {
    let mut map = AgentMap::default();
    let mut rs_files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries =
            std::fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if matches!(name.as_str(), "target" | ".git" | "node_modules") {
                    continue;
                }
                stack.push(path);
            } else if name.ends_with(".rs") {
                rs_files.push(path);
            }
        }
    }

    rs_files.sort();
    for path in rs_files {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(path.as_path())
            .to_string_lossy()
            .replace('\\', "/");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        map.files.push(rel.clone());
        map.sources.push(SourceFile {
            path: rel.clone(),
            raw: raw.clone(),
        });
        parse_file(&rel, &raw, &mut map)?;
    }
    Ok(map)
}

fn parse_file(rel: &str, src: &str, map: &mut AgentMap) -> Result<(), String> {
    let mut lineno = 0usize;
    for_each_line(src, |_, raw| {
        lineno += 1;
        parse_annotation_line(rel, lineno, raw, map)
    })?;
    collect_items(rel, &mask_source(src), map);
    Ok(())
}

/// Call `f` for every line, giving the byte offset of the line start and the
/// line text without its `\n`. Handles `\r\n` correctly (unlike `str::lines`).
fn for_each_line(
    src: &str,
    mut f: impl FnMut(usize, &str) -> Result<(), String>,
) -> Result<(), String> {
    let bytes = src.as_bytes();
    let mut start = 0usize;
    let mut i = 0usize;
    while i <= bytes.len() {
        if i == bytes.len() || bytes[i] == b'\n' {
            f(start, &src[start..i])?;
            start = i + 1;
        }
        i += 1;
    }
    Ok(())
}

fn parse_annotation_line(
    rel: &str,
    lineno: usize,
    raw: &str,
    map: &mut AgentMap,
) -> Result<(), String> {
    let Some(text) = strip_comment(raw) else {
        return Ok(());
    };
    let Some(rest) = text.strip_prefix("@sd") else {
        return Ok(());
    };
    if !rest.starts_with(char::is_whitespace) {
        return Ok(());
    }
    let rest = rest.trim_start();
    let Some(space) = rest.find(char::is_whitespace) else {
        return Err(format!("{rel}:{lineno}: `@sd` needs a kind"));
    };
    let kind = rest[..space].to_string();
    if allowed_keys(&kind).is_none() {
        return Err(format!("{rel}:{lineno}: unknown `@sd` kind `{kind}`"));
    }
    let keys = parse_pairs(&rest[space..]).map_err(|e| format!("{rel}:{lineno}: {e}"))?;
    map.annotations.push(Annotation {
        kind,
        file: rel.to_string(),
        line: lineno,
        keys,
    });
    Ok(())
}

fn strip_comment(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if let Some(r) = t.strip_prefix("//!") {
        return Some(r.trim_start());
    }
    if let Some(r) = t.strip_prefix("///") {
        return Some(r.trim_start());
    }
    if let Some(r) = t.strip_prefix("//") {
        return Some(r.trim_start());
    }
    None
}

/// Parse `key=value` pairs where a value may be `"quoted text"`.
fn parse_pairs(s: &str) -> Result<Vec<(String, String)>, String> {
    let mut pairs = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else {
            return Err(format!("expected `key=value`, found `{rest}`"));
        };
        let key = rest[..eq].trim();
        if key.is_empty() || key.contains(char::is_whitespace) {
            return Err(format!("invalid key in `{rest}`"));
        }
        rest = rest[eq + 1..].trim_start();
        let value;
        if let Some(stripped) = rest.strip_prefix('"') {
            let Some(end) = stripped.find('"') else {
                return Err("unterminated quoted value".to_string());
            };
            value = stripped[..end].to_string();
            rest = stripped[end + 1..].trim_start();
        } else {
            let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
            value = rest[..end].to_string();
            rest = rest[end..].trim_start();
        }
        pairs.push((key.to_string(), value));
    }
    Ok(pairs)
}

// ---------------------------------------------------------------------------
// Item collection
// ---------------------------------------------------------------------------

/// Blank out comments and string/char literals while preserving byte offsets
/// and newlines, so brace matching and `fn` detection see only real code.
fn mask_source(src: &str) -> String {
    let b = src.as_bytes();
    let n = b.len();
    let mut out = b.to_vec();
    let mut i = 0usize;

    let at_boundary = |b: &[u8], i: usize| -> bool {
        i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')
    };

    while i < n {
        // line comment
        if b[i] == b'/' && i + 1 < n && b[i + 1] == b'/' {
            while i < n && b[i] != b'\n' {
                out[i] = b' ';
                i += 1;
            }
            continue;
        }
        // block comment
        if b[i] == b'/' && i + 1 < n && b[i + 1] == b'*' {
            out[i] = b' ';
            out[i + 1] = b' ';
            i += 2;
            while i < n {
                if b[i] == b'*' && i + 1 < n && b[i + 1] == b'/' {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    i += 2;
                    break;
                }
                if b[i] != b'\n' {
                    out[i] = b' ';
                }
                i += 1;
            }
            continue;
        }
        // raw string: r"..." / r#"..."# / br"..."
        if (b[i] == b'r' || (b[i] == b'b' && i + 1 < n && b[i + 1] == b'r')) && at_boundary(b, i) {
            let mut k = i;
            if b[k] == b'b' {
                k += 1;
            }
            let mut j = k + 1;
            let mut hashes = 0usize;
            while j < n && b[j] == b'#' {
                hashes += 1;
                j += 1;
            }
            if j < n && b[j] == b'"' {
                out[i..=j].fill(b' ');
                i = j + 1;
                while i < n {
                    if b[i] == b'"' {
                        let mut ok = true;
                        for h in 0..hashes {
                            if i + 1 + h >= n || b[i + 1 + h] != b'#' {
                                ok = false;
                                break;
                            }
                        }
                        if ok {
                            out[i] = b' ';
                            for h in 0..hashes {
                                out[i + 1 + h] = b' ';
                            }
                            i = i + 1 + hashes;
                            break;
                        }
                    }
                    if b[i] != b'\n' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
                continue;
            }
        }
        // normal string
        if b[i] == b'"' {
            out[i] = b' ';
            i += 1;
            while i < n {
                if b[i] == b'\\' {
                    out[i] = b' ';
                    if i + 1 < n {
                        out[i + 1] = b' ';
                    }
                    i += 2;
                    continue;
                }
                if b[i] == b'"' {
                    out[i] = b' ';
                    i += 1;
                    break;
                }
                if b[i] != b'\n' {
                    out[i] = b' ';
                }
                i += 1;
            }
            continue;
        }
        // char literal (but not a lifetime such as 'a)
        if b[i] == b'\'' {
            let is_char = i + 2 < n && (b[i + 1] == b'\\' || b[i + 2] == b'\'');
            if is_char {
                out[i] = b' ';
                i += 1;
                while i < n {
                    if b[i] == b'\\' {
                        out[i] = b' ';
                        if i + 1 < n {
                            out[i + 1] = b' ';
                        }
                        i += 2;
                        continue;
                    }
                    if b[i] == b'\'' {
                        out[i] = b' ';
                        i += 1;
                        break;
                    }
                    if b[i] != b'\n' {
                        out[i] = b' ';
                    }
                    i += 1;
                }
                continue;
            }
            i += 1;
            continue;
        }
        i += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

/// Blank out comments while preserving string and char literals, so code-level
/// checks (for example "no bare `Command::new(...)`") do not match a comment
/// that merely mentions the spelling.
fn mask_comments(src: &str) -> String {
    let b = src.as_bytes();
    let n = b.len();
    let mut out = b.to_vec();
    let mut i = 0usize;
    while i < n {
        // line comment
        if b[i] == b'/' && i + 1 < n && b[i + 1] == b'/' {
            while i < n && b[i] != b'\n' {
                out[i] = b' ';
                i += 1;
            }
            continue;
        }
        // block comment
        if b[i] == b'/' && i + 1 < n && b[i + 1] == b'*' {
            out[i] = b' ';
            out[i + 1] = b' ';
            i += 2;
            while i < n {
                if b[i] == b'*' && i + 1 < n && b[i + 1] == b'/' {
                    out[i] = b' ';
                    out[i + 1] = b' ';
                    i += 2;
                    break;
                }
                if b[i] != b'\n' {
                    out[i] = b' ';
                }
                i += 1;
            }
            continue;
        }
        // string literal: copy verbatim so its contents are never read as code
        if b[i] == b'"' {
            out[i] = b'"';
            i += 1;
            while i < n {
                let c = b[i];
                out[i] = c;
                i += 1;
                if c == b'\\' && i < n {
                    out[i] = b[i];
                    i += 1;
                    continue;
                }
                if c == b'"' {
                    break;
                }
            }
            continue;
        }
        // char literal or lifetime: copy a bounded run verbatim
        if b[i] == b'\'' {
            let mut j = i;
            while j < n && j - i < 16 && b[j] != b'\n' {
                let c = b[j];
                out[j] = c;
                j += 1;
                if c == b'\\' && j < n {
                    out[j] = b[j];
                    j += 1;
                    continue;
                }
                if c == b'\'' && j > i + 1 {
                    break;
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn collect_items(rel: &str, masked: &str, map: &mut AgentMap) {
    let bytes = masked.as_bytes();
    let mut offset = 0usize;
    let mut lineno = 0usize;
    let mut attrs = String::new();
    let mut i = 0usize;

    while i <= bytes.len() {
        if i == bytes.len() || bytes[i] == b'\n' {
            lineno += 1;
            let line = &masked[offset..i];
            let t = line.trim();
            if t.starts_with("#[") {
                attrs.push_str(t);
                attrs.push(' ');
            } else if let Some(name) = extract_fn_name(line) {
                let body = extract_body(masked, offset).unwrap_or_default();
                let has_assert = has_assertion(&body);
                map.functions.push(FnDef {
                    name: name.clone(),
                    file: rel.to_string(),
                    line: lineno,
                    body: body.clone(),
                });
                if attrs.contains("#[test]") {
                    map.tests.push(TestFn {
                        name,
                        file: rel.to_string(),
                        line: lineno,
                        body,
                        should_panic: attrs.contains("should_panic"),
                        ignored: attrs.contains("ignore"),
                        has_assert,
                    });
                }
                attrs.clear();
            } else if !t.is_empty() {
                attrs.clear();
            }
            offset = i + 1;
        }
        i += 1;
    }
}

/// The name of the `fn` item on this line, if any.
fn extract_fn_name(line: &str) -> Option<String> {
    let bytes = line.as_bytes();
    let mut from = 0usize;
    while let Some(pos) = line[from..].find("fn") {
        let abs = from + pos;
        let before_ok =
            abs == 0 || !(bytes[abs - 1].is_ascii_alphanumeric() || bytes[abs - 1] == b'_');
        let after = abs + 2;
        let after_ok = after < bytes.len() && bytes[after].is_ascii_whitespace();
        if before_ok && after_ok {
            let rest = line[after..].trim_start();
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
        from = abs + 2;
    }
    None
}

/// The brace-balanced body of the `fn` that starts at or after `from`.
fn extract_body(masked: &str, from: usize) -> Option<String> {
    let start = masked[from..].find('{')? + from;
    let b = masked.as_bytes();
    let mut depth = 0i32;
    let mut i = start;
    while i < b.len() {
        match b[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(masked[start..=i].to_string());
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn has_assertion(body: &str) -> bool {
    const TOKENS: &[&str] = &[
        "assert",
        "debug_assert",
        ".unwrap(",
        ".expect(",
        "panic!",
        "unreachable!",
    ];
    TOKENS.iter().any(|t| body.contains(t))
}

// ---------------------------------------------------------------------------
// Validation (the false-green guard)
// ---------------------------------------------------------------------------

/// All problems found in the workspace's annotations and tests. Empty means the
/// breadcrumbs are consistent and no test is vacuous.
pub fn validate(map: &AgentMap) -> Vec<String> {
    let mut errors = Vec::new();
    validate_schema(map, &mut errors);
    validate_file_coverage(map, &mut errors);
    validate_anchors(map, &mut errors);
    validate_flows(map, &mut errors);
    validate_refs(map, &mut errors);
    validate_tests(map, &mut errors);
    validate_verification_strength(map, &mut errors);
    validate_security(map, &mut errors);
    errors
}

fn validate_schema(map: &AgentMap, errors: &mut Vec<String>) {
    for a in &map.annotations {
        let Some(allowed) = allowed_keys(&a.kind) else {
            errors.push(format!(
                "{}:{}: unknown `@sd` kind `{}`",
                a.file, a.line, a.kind
            ));
            continue;
        };
        for req in required_keys(&a.kind) {
            if a.get(req).is_none() {
                errors.push(format!(
                    "{}:{}: `@sd {}` is missing required key `{}`",
                    a.file, a.line, a.kind, req
                ));
            }
        }
        for (k, _) in &a.keys {
            if !allowed.contains(&k.as_str()) {
                errors.push(format!(
                    "{}:{}: `@sd {}` has unknown key `{}`",
                    a.file, a.line, a.kind, k
                ));
            }
        }
    }
}

fn validate_file_coverage(map: &AgentMap, errors: &mut Vec<String>) {
    for f in &map.files {
        if is_test_path(f) {
            continue;
        }
        let described = map
            .annotations
            .iter()
            .any(|a| a.kind == "file" && a.get("module").map(|m| m == f.as_str()).unwrap_or(false));
        if !described {
            errors.push(format!(
                "{f}: missing breadcrumb — add `//! @sd file module=\"{f}\" role=\"...\"`"
            ));
        }
    }
    for a in &map.annotations {
        if a.kind != "file" {
            continue;
        }
        let Some(module) = a.get("module") else {
            continue;
        };
        if module != a.file {
            errors.push(format!(
                "{}:{}: `@sd file` module `{module}` does not name the file it lives in",
                a.file, a.line
            ));
        }
        if !map.files.iter().any(|f| f == module) {
            errors.push(format!(
                "{}:{}: `@sd file` module `{module}` was not scanned",
                a.file, a.line
            ));
        }
    }
}

fn validate_anchors(map: &AgentMap, errors: &mut Vec<String>) {
    let mut anchors = HashSet::new();
    for a in &map.annotations {
        if a.kind == "anchor" {
            let name = a.get("name").unwrap_or("");
            if !anchors.insert(name.to_string()) {
                errors.push(format!("{}:{}: duplicate anchor `{name}`", a.file, a.line));
            }
        }
    }
    for a in &map.annotations {
        if a.kind == "ref" {
            let name = a.get("name").unwrap_or("");
            if !anchors.contains(name) {
                errors.push(format!(
                    "{}:{}: `@sd ref` `{name}` does not resolve to an anchor",
                    a.file, a.line
                ));
            }
        }
    }
}

fn validate_flows(map: &AgentMap, errors: &mut Vec<String>) {
    let mut pipelines: BTreeMap<String, Vec<&Annotation>> = BTreeMap::new();
    for a in map.annotations_of("flow") {
        pipelines
            .entry(a.get("pipeline").unwrap_or("").to_string())
            .or_default()
            .push(a);
    }

    for (pipeline, steps) in pipelines {
        let mut numbered: Vec<(u32, &Annotation)> = Vec::new();
        for a in steps {
            match a.get("step").and_then(|s| s.parse::<u32>().ok()) {
                Some(n) => numbered.push((n, a)),
                None => errors.push(format!(
                    "{}:{}: `@sd flow` step must be an integer",
                    a.file, a.line
                )),
            }
        }
        numbered.sort_by_key(|(n, _)| *n);

        for (i, (n, a)) in numbered.iter().enumerate() {
            let expected = i as u32 + 1;
            if *n != expected {
                errors.push(format!(
                    "{}:{}: pipeline `{pipeline}` steps must be contiguous from 1 (found {n}, expected {expected})",
                    a.file, a.line
                ));
            }
        }

        let mut ids = HashSet::new();
        for (i, (_, a)) in numbered.iter().enumerate() {
            let id = a.get("id").unwrap_or("");
            if !ids.insert(id.to_string()) {
                errors.push(format!(
                    "{}:{}: duplicate flow id `{id}` in pipeline `{pipeline}`",
                    a.file, a.line
                ));
            }
            let is_last = i + 1 == numbered.len();
            match a.get("next") {
                None => {
                    if !is_last {
                        errors.push(format!(
                            "{}:{}: `@sd flow` `{id}` is not last but has no `next`",
                            a.file, a.line
                        ));
                    }
                }
                Some(next) => {
                    let following = numbered.get(i + 1).and_then(|(_, b)| b.get("id"));
                    if following != Some(next) {
                        errors.push(format!(
                            "{}:{}: `@sd flow` `{id}` next=`{next}` does not match the following step",
                            a.file, a.line
                        ));
                    }
                    if is_last {
                        errors.push(format!(
                            "{}:{}: `@sd flow` `{id}` is the last step but has `next`",
                            a.file, a.line
                        ));
                    }
                }
            }
        }
    }
}

fn validate_refs(map: &AgentMap, errors: &mut Vec<String>) {
    let mut seen = HashSet::new();
    for t in &map.tests {
        if !seen.insert(t.name.as_str()) {
            errors.push(format!(
                "{}:{}: duplicate test name `{}` (guard references must be unambiguous)",
                t.file, t.line, t.name
            ));
        }
    }
    let test_names: HashSet<&str> = map.tests.iter().map(|t| t.name.as_str()).collect();
    let verify_ids: HashSet<&str> = map
        .annotations_of("verify")
        .filter_map(|a| a.get("id"))
        .collect();

    for a in &map.annotations {
        match a.kind.as_str() {
            "invariant" => {
                if let Some(t) = a.get("test") {
                    if !test_names.contains(t) {
                        errors.push(format!(
                            "{}:{}: `@sd invariant` test `{t}` does not exist",
                            a.file, a.line
                        ));
                    }
                }
            }
            "verify" => {
                if let Some(t) = a.get("negative_test") {
                    if !test_names.contains(t) {
                        errors.push(format!(
                            "{}:{}: `@sd verify` negative_test `{t}` does not exist",
                            a.file, a.line
                        ));
                    }
                }
            }
            "flow" => {
                if let Some(v) = a.get("verify") {
                    if !verify_ids.contains(v) {
                        errors.push(format!(
                            "{}:{}: `@sd flow` verify `{v}` does not exist",
                            a.file, a.line
                        ));
                    }
                }
            }
            _ => {}
        }
    }
}

fn validate_tests(map: &AgentMap, errors: &mut Vec<String>) {
    for t in &map.tests {
        if t.ignored {
            errors.push(format!(
                "{}:{}: test `{}` is `#[ignore]`d — an ignored test cannot guard anything",
                t.file, t.line, t.name
            ));
        }
        if !t.has_assert && !t.should_panic {
            errors.push(format!(
                "{}:{}: test `{}` contains no assertion (false-green guard)",
                t.file, t.line, t.name
            ));
        }
    }
}

fn validate_verification_strength(map: &AgentMap, errors: &mut Vec<String>) {
    for a in map.annotations_of("verify") {
        let Some(fname) = a.get("fn") else {
            continue;
        };
        let defs: Vec<&FnDef> = map.functions.iter().filter(|f| f.name == fname).collect();
        if defs.is_empty() {
            errors.push(format!(
                "{}:{}: `@sd verify` fn `{fname}` does not exist in the sources",
                a.file, a.line
            ));
            continue;
        }
        for d in defs {
            if d.body.contains("=> true") {
                errors.push(format!(
                    "{}:{}: `{}` can return success for an unrecognised input (`=> true`) — false green",
                    d.file, d.line, fname
                ));
            }
        }
    }
}

fn validate_security(map: &AgentMap, errors: &mut Vec<String>) {
    for s in &map.sources {
        // This crate's own validator mentions the forbidden spellings in its
        // error strings; skip it so the check cannot trip over itself.
        if s.path.starts_with("crates/stickdoctor-agent/") {
            continue;
        }
        let code = mask_comments(&s.raw);
        if code.contains(r#"Command::new("diskpart")"#) {
            errors.push(format!(
                "{}: bare `Command::new(\"diskpart\")` — resolve diskpart.exe from System32",
                s.path
            ));
        }
        if code.contains(r#"Command::new("format.com")"#) {
            errors.push(format!(
                "{}: bare `Command::new(\"format.com\")` — resolve format.com from System32",
                s.path
            ));
        }
        if code.contains(r#"Command::new("chkdsk")"#) {
            errors.push(format!(
                "{}: bare `Command::new(\"chkdsk\")` — resolve chkdsk.exe from System32",
                s.path
            ));
        }
    }
    // The system-disk identity comes from STORAGE_DEVICE_NUMBER, which is
    // { DeviceType; DeviceNumber; PartitionNumber } — 12 bytes. A hand-rolled
    // two-field struct reads DeviceType and silently disables the check.
    for s in &map.sources {
        // This crate's validator mentions the forbidden struct names in its own
        // error string; skip it so the check cannot trip over itself.
        if s.path.starts_with("crates/stickdoctor-agent/") {
            continue;
        }
        let code = mask_comments(&s.raw);
        if code.contains("struct DevNum") || code.contains("struct StorageDeviceNumber") {
            errors.push(format!(
                "{}: hand-rolled STORAGE_DEVICE_NUMBER — use the windows-sys type (it has a leading DeviceType field)",
                s.path
            ));
        }
    }
    if let Some(fmt) = map
        .sources
        .iter()
        .find(|s| s.path.ends_with("stickdoctor-core/src/format.rs"))
    {
        let code = mask_comments(&fmt.raw);
        if !code.contains(r#"system32("diskpart.exe")"#) {
            errors.push(format!(
                "{}: run_diskpart must resolve diskpart.exe from System32",
                fmt.path
            ));
        }
        if !code.contains("Stdio::piped()") {
            errors.push(format!(
                "{}: run_diskpart must feed the script on stdin (no temp file)",
                fmt.path
            ));
        }
        if !code.contains("LOAD_LIBRARY_SEARCH_SYSTEM32") {
            errors.push(format!(
                "{}: format_with_fmifs must load fmifs.dll with LOAD_LIBRARY_SEARCH_SYSTEM32",
                fmt.path
            ));
        }
    }
    if let Some(tools) = map
        .sources
        .iter()
        .find(|s| s.path.ends_with("stickdoctor-core/src/tools.rs"))
    {
        let code = mask_comments(&tools.raw);
        if !code.contains(r#"system32("chkdsk.exe")"#) {
            errors.push(format!(
                "{}: run_chkdsk must resolve chkdsk.exe from System32",
                tools.path
            ));
        }
    }
}

fn is_test_path(path: &str) -> bool {
    path.split('/').any(|seg| seg == "tests")
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// A machine-readable JSON view of the map.
pub fn render_json(map: &AgentMap) -> String {
    let mut o = String::new();
    o.push_str("{\n");
    o.push_str(&format!("  \"schema\": {},\n", json_str(SCHEMA_VERSION)));
    o.push_str("  \"modules\": ");
    o.push_str(&json_str_array(
        &map.annotations_of("file")
            .filter_map(|a| a.get("module").map(str::to_string))
            .collect::<Vec<_>>(),
    ));
    o.push_str(",\n  \"invariants\": ");
    o.push_str(&json_str_array(
        &map.annotations_of("invariant")
            .filter_map(|a| a.get("id").map(str::to_string))
            .collect::<Vec<_>>(),
    ));
    o.push_str(",\n  \"verifications\": ");
    o.push_str(&json_str_array(
        &map.annotations_of("verify")
            .filter_map(|a| a.get("id").map(str::to_string))
            .collect::<Vec<_>>(),
    ));
    o.push_str(",\n  \"tests\": ");
    o.push_str(&json_str_array(
        &map.tests.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
    ));
    o.push_str("\n}\n");
    o
}

/// A human-readable breadcrumb map, also committed as `docs/AGENT_MAP.md`.
pub fn render_markdown(map: &AgentMap) -> String {
    let mut o = String::new();
    o.push_str("# StickDoctor agent breadcrumb map\n\n");
    o.push_str(&format!(
        "Generated from `@sd` annotations (schema `{}`).\n\
         Regenerate with `cargo run -p stickdoctor-cli -- agent-map --write`.\n\n",
        SCHEMA_VERSION
    ));

    o.push_str("## Modules\n\n");
    o.push_str("| Module | Role | Entry | Exit |\n|---|---|---|---|\n");
    for a in map.annotations_of("file") {
        o.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            a.get("module").unwrap_or(""),
            a.get("role").unwrap_or(""),
            a.get("entry").unwrap_or(""),
            a.get("exit").unwrap_or("")
        ));
    }
    o.push('\n');

    let mut pipelines: BTreeMap<String, Vec<&Annotation>> = BTreeMap::new();
    for a in map.annotations_of("flow") {
        pipelines
            .entry(a.get("pipeline").unwrap_or("").to_string())
            .or_default()
            .push(a);
    }
    for (pipeline, mut steps) in pipelines {
        steps.sort_by_key(|a| {
            a.get("step")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)
        });
        o.push_str(&format!("## Flow: `{pipeline}`\n\n"));
        o.push_str("| Step | Stage | Verifies |\n|---|---|---|\n");
        for a in steps {
            o.push_str(&format!(
                "| {} | {} (`{}`) | {} |\n",
                a.get("step").unwrap_or(""),
                a.get("label").unwrap_or(""),
                a.get("id").unwrap_or(""),
                a.get("verify").unwrap_or("")
            ));
        }
        o.push('\n');
    }

    o.push_str("## Invariants\n\n");
    o.push_str("| ID | Invariant | Guard test |\n|---|---|---|\n");
    for a in map.annotations_of("invariant") {
        o.push_str(&format!(
            "| {} | {} | `{}` |\n",
            a.get("id").unwrap_or(""),
            a.get("text").unwrap_or(""),
            a.get("test").unwrap_or("")
        ));
    }
    o.push('\n');

    o.push_str("## Verification\n\n");
    o.push_str("| ID | Function | Proves | Negative test |\n|---|---|---|---|\n");
    for a in map.annotations_of("verify") {
        o.push_str(&format!(
            "| {} | `{}` | {} | `{}` |\n",
            a.get("id").unwrap_or(""),
            a.get("fn").unwrap_or(""),
            a.get("proves").unwrap_or(""),
            a.get("negative_test").unwrap_or("")
        ));
    }
    o.push('\n');

    o.push_str("## Hazards\n\n");
    o.push_str("| Hazard | Guard |\n|---|---|\n");
    for a in map.annotations_of("hazard") {
        o.push_str(&format!(
            "| {} | {} |\n",
            a.get("text").unwrap_or(""),
            a.get("guard").unwrap_or("")
        ));
    }
    o.push('\n');

    o.push_str(&format!(
        "Counts: {} modules, {} annotations, {} tests.\n",
        map.files.len(),
        map.annotations.len(),
        map.tests.len()
    ));
    o
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn json_str_array(items: &[String]) -> String {
    let mut o = String::from("[\n");
    for (i, item) in items.iter().enumerate() {
        let comma = if i + 1 == items.len() { "" } else { "," };
        o.push_str(&format!("    {}{}\n", json_str(item), comma));
    }
    o.push(']');
    o
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_strings_comments_and_raw_strings() {
        let src = "let a = \"fn not_a_fn {\"; // fn also_not\n\
                   let b = r\"fn raw {\";\n\
                   let c = 'x';\n\
                   fn real() { assert!(true); }\n";
        let masked = mask_source(src);
        assert!(masked.contains("fn real"));
        assert!(!masked.contains("not_a_fn"));
        assert!(!masked.contains("also_not"));
        assert!(!masked.contains("raw {"));
    }

    #[test]
    fn extract_fn_name_finds_the_item() {
        assert_eq!(extract_fn_name("fn foo() {"), Some("foo".to_string()));
        assert_eq!(
            extract_fn_name("pub(crate) fn bar(x: u32) -> u32 {"),
            Some("bar".to_string())
        );
        assert_eq!(extract_fn_name("let x = 1;"), None);
        assert_eq!(extract_fn_name("type F = fn(u32) -> u32;"), None);
    }

    #[test]
    fn parses_key_value_pairs_with_quotes() {
        let pairs = parse_pairs("module=\"a b c\" role=\"x=y\" step=3").unwrap();
        assert_eq!(pairs[0], ("module".to_string(), "a b c".to_string()));
        assert_eq!(pairs[1], ("role".to_string(), "x=y".to_string()));
        assert_eq!(pairs[2], ("step".to_string(), "3".to_string()));
    }

    #[test]
    fn annotation_lines_are_detected() {
        let mut map = AgentMap::default();
        parse_annotation_line(
            "f.rs",
            1,
            "//! @sd file module=\"a.rs\" role=\"r\"",
            &mut map,
        )
        .unwrap();
        assert_eq!(map.annotations.len(), 1);
        assert_eq!(map.annotations[0].kind, "file");
        assert_eq!(map.annotations[0].get("module"), Some("a.rs"));
        // Prose that merely mentions @sd mid-line is not an annotation.
        parse_annotation_line("f.rs", 2, "// see @sd file for details", &mut map).unwrap();
        assert_eq!(map.annotations.len(), 1);
    }

    #[test]
    fn unknown_kinds_are_rejected() {
        let mut map = AgentMap::default();
        assert!(parse_annotation_line("f.rs", 1, "//! @sd flle module=\"a\"", &mut map).is_err());
    }

    #[test]
    fn flow_chains_must_be_contiguous() {
        let mut map = AgentMap::default();
        for (line, ann) in [
            (
                1,
                "//! @sd flow pipeline=p id=p.one step=1 label=a next=p.two",
            ),
            (2, "//! @sd flow pipeline=p id=p.two step=3 label=b"),
        ] {
            parse_annotation_line("f.rs", line, ann, &mut map).unwrap();
        }
        let errors = validate(&map);
        assert!(
            errors.iter().any(|e| e.contains("contiguous")),
            "expected a contiguity error, got {errors:?}"
        );
    }

    #[test]
    fn vacuous_tests_are_rejected() {
        let mut map = AgentMap::default();
        // A file with a #[test] fn that has no assertion.
        let src = "#[test]\nfn nothing() {\n    let _ = 1;\n}\n";
        collect_items("f.rs", &mask_source(src), &mut map);
        let errors = validate(&map);
        assert!(
            errors.iter().any(|e| e.contains("no assertion")),
            "expected a vacuous-test error, got {errors:?}"
        );
    }

    #[test]
    fn mask_comments_keeps_strings_but_drops_comments() {
        let src = "let a = \"code here\"; // comment Command::new(\"chkdsk\")\nlet b = 1;\n";
        let masked = mask_comments(src);
        // The comment is blanked.
        assert!(!masked.contains("comment"));
        // String contents survive.
        assert!(masked.contains("code here"));
    }
}
