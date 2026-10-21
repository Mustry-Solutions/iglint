//! The `.cases` file format shared with `oracle/jython_ast_oracle.py`.
//!
//! A case file holds many small sources. Each starts with a line `### <name>`
//! and runs until the next such line. A name may carry flags: `[nonl]` drops
//! the final newline, `[crlf]` and `[cr]` rewrite `\n` line ends.

/// One source from a case file.
#[derive(Clone, Debug)]
pub struct Case {
    pub name: String,
    pub source: Vec<u8>,
}

/// Splits a case file into its cases.
pub fn split_cases(data: &[u8]) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut current: Option<Case> = None;
    for line in data.split_inclusive(|&byte| byte == b'\n') {
        if let Some(rest) = line.strip_prefix(b"### ") {
            if let Some(case) = current.take() {
                cases.push(case);
            }
            let name = String::from_utf8_lossy(rest)
                .trim_end_matches(['\r', '\n'])
                .to_owned();
            current = Some(Case {
                name,
                source: Vec::new(),
            });
        } else if let Some(case) = current.as_mut() {
            case.source.extend_from_slice(line);
        }
    }
    cases.extend(current);
    for case in &mut cases {
        apply_flags(case);
    }
    cases
}

fn apply_flags(case: &mut Case) {
    if case.name.contains("[nonl]") && case.source.ends_with(b"\n") {
        case.source.pop();
    }
    let replacement: &[u8] = if case.name.contains("[crlf]") {
        b"\r\n"
    } else if case.name.contains("[cr]") {
        b"\r"
    } else {
        return;
    };
    let mut out = Vec::with_capacity(case.source.len() + 16);
    for &byte in &case.source {
        if byte == b'\n' {
            out.extend_from_slice(replacement);
        } else {
            out.push(byte);
        }
    }
    case.source = out;
}

/// Escapes a line the way the oracle does: backslash, `\r` and `\n`.
pub fn one_line(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

/// Decodes source bytes the way the oracle hands them to Jython: a UTF-8
/// BOM is dropped; sources with an encoding declaration are decoded with the
/// declared charset and parsed as files, others are UTF-8 and parsed as
/// unicode. Returns `None` for bytes the oracle cannot decode either.
pub fn decode_source(data: &[u8]) -> Option<(String, iglint_python::SourceKind)> {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    // The declaration is ASCII, so a lossy view is enough to find it.
    let lossy = String::from_utf8_lossy(data);
    match iglint_python::encoding_declaration(&lossy) {
        Some((name, _)) => {
            let text = match iglint_python::SourceEncoding::from_name(name) {
                Some(iglint_python::SourceEncoding::Latin1) => {
                    data.iter().map(|&byte| char::from(byte)).collect()
                }
                Some(iglint_python::SourceEncoding::Cp1252) => decode_cp1252(data),
                _ => String::from_utf8(data.to_vec()).ok()?,
            };
            Some((text, iglint_python::SourceKind::File))
        }
        None => Some((
            String::from_utf8(data.to_vec()).ok()?,
            iglint_python::SourceKind::Unicode,
        )),
    }
}

fn decode_cp1252(data: &[u8]) -> String {
    const HIGH: [u32; 32] = [
        0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160,
        0x2039, 0x0152, 0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013,
        0x2014, 0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
    ];
    data.iter()
        .map(|&byte| match byte {
            0x80..=0x9F => char::from_u32(HIGH[usize::from(byte - 0x80)]).unwrap_or('?'),
            _ => char::from(byte),
        })
        .collect()
}

/// What iglint makes of a source, in the oracle's record format (without
/// the `### name` line).
pub fn ast_record(data: &[u8]) -> String {
    let Some((source, kind)) = decode_source(data) else {
        return "error 0 0\nsource is not valid UTF-8\n".to_owned();
    };
    let options = iglint_python::ParseOptions {
        source_kind: kind,
        ..iglint_python::ParseOptions::default()
    };
    let parsed = iglint_python::parse_module_with_options(&source, &options);
    match parsed.errors.first() {
        Some(error) => {
            let locator = iglint_python::Locator::new(&source);
            let location = error.jython_location(&locator);
            format!(
                "error {} {}\n{}\n",
                location.line,
                location.column,
                one_line(&error.message)
            )
        }
        None => format!(
            "ok\n{}\n{}\n",
            one_line(&iglint_python::dump::dump(&parsed.module)),
            one_line(&iglint_python::dump::dump_with_attributes(
                &parsed.module,
                &source
            ))
        ),
    }
}
