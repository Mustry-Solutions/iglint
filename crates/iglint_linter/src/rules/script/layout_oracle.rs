//! Differential test of the layout-only safe fixes (`SCR123`, `SCR124`)
//! against Ignition's Jython (review round 25, B1).
//!
//! A generated program of library-module code full of what Jython's lexer
//! reads in unusual ways (strings it drops, backslashes followed by
//! whitespace, stray backslashes, form feeds, carriage returns, comments
//! and suppression comments in and after them, continuation lines,
//! brackets, triple-quoted strings, blocks) is fixed with the safe fixes
//! only, and Jython runs it before and after
//! (`oracle/run-oracle.sh --run`, which records the output, the exception
//! with its line, and every global's value). A safe fix must never change
//! that record.
//!
//! Running a fresh batch (needs Docker and the 8.3.3 image):
//!
//! ```sh
//! IGLINT_LAYOUT_OUT=$HOME/.cache/layout cargo test -p iglint_linter --lib \
//!     layout_oracle::write_sources -- --ignored
//! oracle/run-oracle.sh --run $HOME/.cache/layout $HOME/.cache/layout/list.txt \
//!     $HOME/.cache/layout/result.txt
//! IGLINT_LAYOUT_OUT=$HOME/.cache/layout cargo test -p iglint_linter --lib \
//!     layout_oracle::compare_results -- --ignored --nocapture
//! ```
//!
//! `IGLINT_LAYOUT_SEED` (default 25) and `IGLINT_LAYOUT_COUNT` (default
//! 2,000) choose the batch. With `IGLINT_LAYOUT_RECORD=<file>`,
//! `compare_results` also writes a sample of the pairs whose text the
//! fixes changed, which `replayed_sample_is_unchanged` replays without
//! Docker: it regenerates each program, fixes it again and checks that
//! the fixed text is the one Jython ran (so a change of the fixes asks
//! for a new recording).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::FixMode;
use crate::rules::script::testing::{library, library_path};

/// The rules whose safe fixes are tested (`SCR101` decides which
/// suppressions are used).
const RULES: [(&str, &str); 3] = [("SCR101", ""), ("SCR123", ""), ("SCR124", "")];

/// The committed sample.
const SAMPLE: &str = include_str!("../../../tests/fixtures/layout-fix/sample.expected");

/// SplitMix64.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.next() % n as u64).unwrap_or(0)
    }

    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }

    fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

/// Trailing whitespace of one of the kinds the rules remove.
fn ws(r: &mut Rng) -> &'static str {
    r.pick(&[" ", "  ", "\t", " \t", "\x0c", " \x0c", "\t ", "   "])
}

/// A short expression over the names defined so far.
fn expr(r: &mut Rng, names: &[String]) -> String {
    let atom = |r: &mut Rng| -> String {
        match r.below(5) {
            0 | 1 if !names.is_empty() => names[r.below(names.len())].clone(),
            2 => format!("'{}'", r.pick(&["s", "a b", "#", "x#y", "\\\\", "q\\n"])),
            3 => format!("u'{}'", r.pick(&["u", "#c"])),
            _ => r.below(100).to_string(),
        }
    };
    let a = atom(r);
    if r.chance(30) {
        let b = atom(r);
        // Keep numbers with numbers so that most programs run on.
        if a.ends_with('\'') == b.ends_with('\'') {
            return format!("{a} + {b}");
        }
    }
    a
}

/// A suppression comment, used (on a line that reads an undefined name)
/// or not.
fn suppression(r: &mut Rng) -> &'static str {
    r.pick(&[
        "# iglint: ignore[SCR101]",
        "# iglint: ignore[SCR101, SCR102]",
        "# iglint: ignore",
        "#iglint: ignore[SCR101]",
        "# note",
        "# iglint: ignore[SCR101] reason",
    ])
}

/// The end of a line: maybe a comment, maybe trailing whitespace, then the
/// line break.
fn end(r: &mut Rng, out: &mut String) {
    if r.chance(25) {
        out.push_str(r.pick(&["  ", " ", "\t"]));
        out.push_str(suppression(r));
    }
    if r.chance(60) {
        out.push_str(ws(r));
    }
    out.push_str(if r.chance(8) { "\r\n" } else { "\n" });
}

/// One generated program.
pub(crate) fn program(seed: u64, index: u64) -> String {
    let mut r = Rng(seed.wrapping_mul(1_000_003).wrapping_add(index));
    let mut names: Vec<String> = Vec::new();
    let mut out = String::new();
    let lines = 3 + r.below(10);
    for i in 0..lines {
        let name = format!("v{i}");
        let e = expr(&mut r, &names);
        match r.below(20) {
            // A string Jython drops: a backslash and whitespace end the line
            // inside a single-quoted string (the whitespace is escaped, the
            // line break ends the string and goes with it).
            0..=3 => {
                let q = r.pick(&["'", "\""]);
                let _ = write!(
                    out,
                    "{name} = {q}{}\\",
                    r.pick(&["x", "x  ", "", "a#b", "\\\\"])
                );
                out.push_str(ws(&mut r));
                if r.chance(30) {
                    out.push_str(suppression(&mut r));
                    if r.chance(50) {
                        out.push_str(ws(&mut r));
                    }
                }
                out.push('\n');
                // What follows is code while the string is dropped, and the
                // rest of the string once the backslash continues it; a
                // quote in a comment closes the continued string.
                match r.below(6) {
                    0 => {
                        let _ = write!(out, "{e} #{q} + 'z'");
                    }
                    1 | 2 => {
                        let _ = write!(out, "{e} #{q}");
                    }
                    3 => {
                        let _ = write!(out, "y #{q}");
                    }
                    4 => {
                        let _ = write!(out, "{e}{q}");
                    }
                    _ => {
                        let _ = write!(out, "{e}");
                    }
                }
            }
            // A string Jython drops without a backslash.
            4 => {
                let _ = write!(out, "{name} = '{}", r.pick(&["ab", "a b", ""]));
                out.push_str(ws(&mut r));
                out.push('\n');
                let _ = write!(out, "{e}");
            }
            // A stray backslash before whitespace (dropped with the next
            // character), before a comment, or in brackets.
            5 | 6 => {
                let bracket = r.chance(40);
                let _ = write!(out, "{name} = {}{e} \\", if bracket { "[" } else { "" });
                out.push_str(ws(&mut r));
                if r.chance(30) {
                    out.push_str(if r.chance(50) {
                        "# c"
                    } else {
                        suppression(&mut r)
                    });
                }
                out.push('\n');
                let next = expr(&mut r, &names);
                if bracket {
                    let _ = write!(out, "    , {next}]");
                } else {
                    out.push_str(&next);
                }
            }
            // `print` with a trailing comma and a stray backslash.
            7 => {
                let _ = write!(out, "print {e}, \\");
                out.push_str(ws(&mut r));
                out.push('\n');
                let _ = write!(out, "{}", r.below(10));
            }
            // A real continuation line.
            8 => {
                let _ = write!(out, "{name} = {e} + \\\n    {}", r.below(10));
            }
            // Brackets over several lines.
            9 => {
                let _ = write!(out, "{name} = [{e},");
                end(&mut r, &mut out);
                let _ = write!(out, "    {}]", r.below(10));
            }
            // Whitespace inside a triple-quoted string (data).
            10 => {
                let _ = write!(out, "{name} = '''a");
                out.push_str(ws(&mut r));
                out.push_str(r.pick(&["\n", "\\\n", "\r\n"]));
                out.push_str("b'''");
            }
            // A comment line, a blank line.
            11 => {
                out.push_str(r.pick(&["# comment", "", "#", "    # indented"]));
            }
            // A block.
            12 => {
                let _ = write!(out, "if {e}:");
                end(&mut r, &mut out);
                let _ = write!(out, "    {name} = {}", expr(&mut r, &names));
                end(&mut r, &mut out);
                if r.chance(40) {
                    out.push_str(ws(&mut r));
                    out.push('\n');
                }
                out.push_str("else:");
                end(&mut r, &mut out);
                let _ = write!(out, "    {name} = 0");
            }
            // A lone carriage return or form feed (dropped), or `!`.
            13 => {
                let _ = write!(
                    out,
                    "{name} = {e}{}",
                    r.pick(&[" \r", "\r ", " \x0c ", " !x"])
                );
            }
            // A name that is not defined, under a suppression or not.
            14 => {
                let _ = write!(out, "{name} = undefined_{i}");
            }
            // A backslash followed by whitespace inside a terminated string.
            15 => {
                let _ = write!(out, "{name} = 'a\\ b' + '\\\t'");
            }
            _ => {
                let _ = write!(out, "{name} = {e}");
            }
        }
        end(&mut r, &mut out);
        names.push(name);
    }
    if r.chance(30) {
        // No line break at the end.
        while out.ends_with(['\n', '\r']) {
            out.pop();
        }
        out.push_str(ws(&mut r));
    }
    out
}

/// The program fixed with the safe fixes of [`RULES`].
fn fixed(source: &str) -> String {
    let f = library("m", source);
    let result = f.run(
        &RULES,
        FixMode::DiffOnly {
            unsafe_fixes: false,
        },
    );
    let path = library_path(&f, "m");
    result
        .fixed
        .iter()
        .find(|s| s.path == path)
        .map_or_else(|| source.to_owned(), |s| s.fixed.clone())
}

/// FNV-1a of a text, to name it in the sample.
fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn out_dir() -> PathBuf {
    PathBuf::from(std::env::var("IGLINT_LAYOUT_OUT").expect("set IGLINT_LAYOUT_OUT"))
}

/// Writes every program and, where the fixes changed it, its fixed text
/// (`N.before.py`, `N.after.py`) and the list for the oracle.
#[test]
#[ignore = "writes the sources of a differential run against Jython"]
fn write_sources() {
    let dir = out_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let seed = env_u64("IGLINT_LAYOUT_SEED", 25);
    let count = env_u64("IGLINT_LAYOUT_COUNT", 2000);
    let mut list = String::new();
    let mut changed = 0;
    for i in 0..count {
        let before = program(seed, i);
        let after = fixed(&before);
        if after == before {
            continue;
        }
        changed += 1;
        for (kind, text) in [("before", &before), ("after", &after)] {
            let name = format!("{seed}-{i}.{kind}.py");
            std::fs::write(dir.join(&name), text).unwrap();
            let _ = writeln!(list, "{name}");
        }
    }
    std::fs::write(dir.join("list.txt"), list).unwrap();
    println!("{count} programs, {changed} changed by the safe fixes");
}

/// The records of an oracle result file, by source name.
fn records(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("### ") {
            out.push((name.to_owned(), String::new()));
        } else if let Some((_, record)) = out.last_mut() {
            record.push_str(line);
            record.push('\n');
        }
    }
    out
}

/// Compares Jython's record of every program before and after its fixes;
/// with `IGLINT_LAYOUT_RECORD`, writes a sample for the replay test.
#[test]
#[ignore = "needs the result of oracle/run-oracle.sh --run on write_sources"]
fn compare_results() {
    let dir = out_dir();
    let result = std::fs::read_to_string(dir.join("result.txt")).unwrap();
    let records = records(&result);
    let by_name: std::collections::HashMap<&str, &str> = records
        .iter()
        .map(|(n, r)| (n.as_str(), r.as_str()))
        .collect();
    let mut pairs = 0;
    let mut differences = Vec::new();
    let mut sample = String::new();
    let mut kinds = std::collections::BTreeMap::<&str, usize>::new();
    for (name, record) in &records {
        let Some(stem) = name.strip_suffix(".before.py") else {
            continue;
        };
        let after = by_name
            .get(format!("{stem}.after.py").as_str())
            .unwrap_or_else(|| panic!("no record for {stem}.after.py"));
        pairs += 1;
        let kind = record.lines().next().unwrap_or_default();
        *kinds
            .entry(kind.split(' ').next().unwrap_or_default())
            .or_default() += 1;
        if record != after {
            differences.push(format!("{stem}:\n{record}--- after:\n{after}"));
            continue;
        }
        // Every sixth pair.
        let (seed, index) = stem.split_once('-').unwrap();
        let (seed, index): (u64, u64) = (seed.parse().unwrap(), index.parse().unwrap());
        let before_text = std::fs::read_to_string(dir.join(format!("{stem}.before.py"))).unwrap();
        let after_text = std::fs::read_to_string(dir.join(format!("{stem}.after.py"))).unwrap();
        if index % 6 == 0 {
            let _ = writeln!(
                sample,
                "### {seed} {index} {:016x} {:016x}",
                hash(&before_text),
                hash(&after_text)
            );
            sample.push_str(record);
        }
    }
    println!("{pairs} pairs compared, by outcome before the fixes: {kinds:?}");
    if let Ok(path) = std::env::var("IGLINT_LAYOUT_RECORD") {
        std::fs::write(Path::new(&path), sample).unwrap();
    }
    assert!(
        differences.is_empty(),
        "{} of {pairs} safe fixes change what Jython does:\n{}",
        differences.len(),
        differences.join("\n")
    );
}

/// Replays the committed sample: every program still gets the fixed text
/// that Jython was shown to run like the original.
#[test]
fn replayed_sample_is_unchanged() {
    let mut cases = 0;
    let mut outcomes = std::collections::BTreeSet::new();
    for line in SAMPLE.lines() {
        let Some(head) = line.strip_prefix("### ") else {
            if !line.starts_with("out ") && !line.starts_with("g ") {
                outcomes.insert(line.split(' ').next().unwrap_or_default().to_owned());
            }
            continue;
        };
        let fields: Vec<&str> = head.split(' ').collect();
        let [seed, index, before, after] = fields[..] else {
            panic!("bad sample line {line}");
        };
        let source = program(seed.parse().unwrap(), index.parse().unwrap());
        assert_eq!(
            format!("{:016x}", hash(&source)),
            before,
            "the generator changed; record the sample again"
        );
        let fixed = fixed(&source);
        assert_eq!(
            format!("{:016x}", hash(&fixed)),
            after,
            "the safe fixes of program {seed}-{index} changed since Jython ran them; \
             record the sample again (see the module documentation):\n{source:?}\n{fixed:?}"
        );
        cases += 1;
    }
    assert!(cases >= 100, "{cases}");
    // The sample covers programs that run, raise and do not compile.
    for outcome in ["ok", "raised"] {
        assert!(outcomes.contains(outcome), "{outcomes:?}");
    }
}

/// The generator produces the shapes of review round 25, B1.
#[test]
fn programs_cover_dropped_strings() {
    let dropped = (0..200)
        .filter(|i| {
            let p = program(25, *i);
            iglint_python::parse_module(&p)
                .dropped
                .iter()
                .any(|d| d.reason == iglint_python::DropReason::UnterminatedString)
        })
        .count();
    assert!(dropped > 50, "{dropped}");
}
