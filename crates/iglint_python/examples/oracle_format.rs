//! Prints what iglint makes of Python sources in the format of the Jython
//! oracle (`oracle/run-oracle.sh`), so that the two outputs can be diffed.
//!
//! usage: oracle_format [--tokens] ROOT LIST OUT

use std::fmt::Write as _;
use std::path::Path;

use iglint_python::{AnchorKind, Locator, Token, TokenKind};

#[path = "../tests/support/cases.rs"]
mod cases;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let tokens = if args.first().map(String::as_str) == Some("--tokens") {
        args.remove(0);
        true
    } else {
        false
    };
    let [root, list, out] = <[String; 3]>::try_from(args).unwrap_or_else(|_| {
        eprintln!("usage: oracle_format [--tokens] ROOT LIST OUT");
        std::process::exit(2);
    });
    let list = std::fs::read_to_string(list).expect("read list");
    let mut output = String::new();
    for path in list.lines().filter(|line| !line.is_empty()) {
        let data = std::fs::read(Path::new(&root).join(path)).expect("read input");
        let items: Vec<(String, Vec<u8>)> = if path.ends_with(".cases") {
            cases::split_cases(&data)
                .into_iter()
                .map(|case| (case.name, case.source))
                .collect()
        } else {
            vec![(path.to_owned(), data)]
        };
        for (name, data) in items {
            let _ = writeln!(output, "### {}", cases::one_line(&name));
            let source = String::from_utf8_lossy(&data);
            let source = source.strip_prefix('\u{feff}').unwrap_or(&source);
            if tokens {
                write_tokens(&mut output, source);
            } else {
                output.push_str(&cases::ast_record(&data));
            }
        }
    }
    std::fs::write(out, output).expect("write output");
}

fn write_tokens(output: &mut String, source: &str) {
    let locator = Locator::new(source);
    let (tokens, error) = iglint_python::tokenize_until_error(source);
    let rendered: Vec<String> = tokens
        .iter()
        .map(|token| render_token(&locator, token))
        .collect();
    let _ = writeln!(output, "{}", rendered.join(" "));
    if let Some(error) = error {
        // The lexer's own position, even where building Jython's
        // SyntaxError would fail later.
        let location = locator.anchor_location(error.anchor());
        let _ = writeln!(
            output,
            "error {} {} {}",
            location.line,
            location.column,
            cases::one_line(&error.message)
        );
    }
}

fn render_token(locator: &Locator<'_>, token: &Token) -> String {
    let location = locator.anchor_location(token.anchor);
    let column = match token.anchor.kind() {
        // Tokens keep Jython's internal marker for multi-line strings.
        AnchorKind::MultilineString(_) => -2,
        _ => location.column,
    };
    let name = if token.kind == TokenKind::EndOfFile {
        "EOF"
    } else {
        token.kind.jython_name()
    };
    format!("{name}@{}:{}", location.line, column)
}
