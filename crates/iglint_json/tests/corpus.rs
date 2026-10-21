//! Checks against a private Ignition project. Run with
//! `IGLINT_PRIVATE_CORPUS=/path cargo test -p iglint_json --release -- --ignored --nocapture`.
//! Prints aggregate numbers only.

mod common;

use std::path::{Path, PathBuf};
use std::time::Instant;

use common::{J, from_value, render};
use iglint_json::{EscapeStyle, JsonEditor, Value, ValueKind, parse};
use iglint_text::apply_edits;

fn json_files(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if path.is_dir() {
            json_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "json") {
            out.push(path);
        }
    }
}

fn corpus() -> Option<Vec<PathBuf>> {
    let root = std::env::var_os("IGLINT_PRIVATE_CORPUS")?;
    let mut files = Vec::new();
    json_files(Path::new(&root), &mut files);
    files.sort();
    Some(files)
}

#[test]
#[ignore = "needs IGLINT_PRIVATE_CORPUS"]
fn corpus_parses_and_edits_cleanly() {
    let Some(files) = corpus() else {
        eprintln!("IGLINT_PRIVATE_CORPUS not set");
        return;
    };
    let texts: Vec<String> = files
        .iter()
        .map(|f| std::fs::read_to_string(f).unwrap())
        .collect();
    let total_bytes: usize = texts.iter().map(String::len).sum();

    let start = Instant::now();
    let mut failures = 0;
    for _ in 0..5 {
        for t in &texts {
            if parse(t).is_err() {
                failures += 1;
            }
        }
    }
    let elapsed = start.elapsed() / 5;
    assert_eq!(failures, 0, "files that failed to parse");

    let mut exact_style = 0;
    let mut sorted_objects = 0;
    let mut objects = 0;
    let mut duplicate_groups = 0;
    let mut removal_checks = 0;
    let mut removal_layout_exact = 0;
    for t in &texts {
        let v = parse(t).unwrap();
        let j = from_value(&v);
        let style = iglint_json::detect_escape_style(&v);
        if render(&j, true, style) == *t || render(&j, true, style) == t.trim_end() {
            exact_style += 1;
        }
        v.walk(|_, value| {
            if let Some(o) = value.as_object() {
                objects += 1;
                if o.is_sorted_like_ignition() {
                    sorted_objects += 1;
                }
                duplicate_groups += o.duplicate_keys().len();
            }
            true
        });
        removal_checks += check_removals(t, &v, &j, &mut removal_layout_exact);
    }
    eprintln!(
        "json corpus: {} files, {:.1} MiB, parse {:.1} ms ({:.0} MiB/s)",
        texts.len(),
        total_bytes as f64 / 1048576.0,
        elapsed.as_secs_f64() * 1000.0,
        total_bytes as f64 / 1048576.0 / elapsed.as_secs_f64()
    );
    eprintln!(
        "files byte-identical to Gson pretty style: {exact_style}/{}",
        texts.len()
    );
    eprintln!(
        "objects with sorted keys: {sorted_objects}/{objects}; duplicate key groups: {duplicate_groups}"
    );
    eprintln!(
        "member removals checked: {removal_checks}, layout identical to a fresh Gson write: {removal_layout_exact}"
    );
}

/// Removes the first member of every object (up to a budget) and checks the
/// result parses to the expected tree.
fn check_removals(text: &str, v: &Value<'_>, j: &J, layout_exact: &mut usize) -> usize {
    let editor = JsonEditor::new(text, v);
    let original_exact = render(j, true, editor.escape_style()) == text;
    let mut checked = 0;
    let mut stack: Vec<(Vec<usize>, &Value<'_>)> = vec![(Vec::new(), v)];
    while let Some((path, value)) = stack.pop() {
        if checked >= 200 {
            break;
        }
        let ValueKind::Object(o) = &value.kind else {
            if let ValueKind::Array(a) = &value.kind {
                for (i, e) in a.iter().enumerate() {
                    let mut p = path.clone();
                    p.push(i);
                    stack.push((p, e));
                }
            }
            continue;
        };
        for (i, m) in o.iter().enumerate() {
            let mut p = path.clone();
            p.push(i);
            stack.push((p, &m.value));
        }
        if o.is_empty() {
            continue;
        }
        let index = o.len() / 2;
        let edit = editor.remove_member(value, index).unwrap();
        let out = apply_edits(text, &[edit]).unwrap();
        let mut expected = j.clone();
        let mut cur = &mut expected;
        for &k in &path {
            cur = match cur {
                J::Arr(items) => &mut items[k],
                J::Obj(members) => &mut members[k].1,
                _ => unreachable!(),
            };
        }
        let J::Obj(members) = cur else { unreachable!() };
        members.remove(index);
        let reparsed = parse(&out).expect("edited file parses");
        assert_eq!(from_value(&reparsed), expected);
        if original_exact && out == render(&expected, true, editor.escape_style()) {
            *layout_exact += 1;
        } else if original_exact {
            panic!("layout differs after removal");
        }
        checked += 1;
    }
    let _ = EscapeStyle::Ignition;
    checked
}
