//! Parses Python files and prints their syntax errors with Jython's
//! positions (the same way iglint will report them).
//!
//! usage: check FILE...

fn main() {
    for path in std::env::args().skip(1) {
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                eprintln!("{path}: {error}");
                continue;
            }
        };
        let parsed = iglint_python::parse_module(&source);
        let locator = iglint_python::Locator::new(&source);
        for error in &parsed.errors {
            let location = error.jython_location(&locator);
            println!(
                "{path}:{}:{}: {:?}: {}",
                location.line, location.column, error.kind, error.message
            );
        }
        for dropped in &parsed.dropped {
            let location = locator.location(dropped.range.start());
            println!(
                "{path}:{}:{}: warning: {}",
                location.line,
                location.column,
                dropped.reason.message()
            );
        }
    }
}
