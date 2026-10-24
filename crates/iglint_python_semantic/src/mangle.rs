//! Private name mangling, as Jython's code generator does it.
//!
//! Measured on Ignition 8.3.3's Jython (`oracle/scope/`): the compiler
//! rewrites an identifier that starts with two underscores and does not end
//! with two (`__x`, `___x`, `__x_`, not `__x__` or `__`) to
//! `_<class>__x`, where `<class>` is the class name without its leading
//! underscores, in these places only:
//!
//! * every name a class body reads, stores or deletes (assignments,
//!   augmented assignments, `del`, `for`, `with` and `except` targets,
//!   tuple unpacking, list comprehension variables, import aliases, `def`
//!   and `class` names), with the class's own name;
//! * every attribute name (`obj.__x`) and the code object name of every
//!   `def` and `class` in any scope nested in a class (methods, lambdas,
//!   generator expressions, functions nested in methods), with the name
//!   of the innermost enclosing class.
//!
//! Names read or stored in functions, lambdas and generator expressions
//! are never mangled (CPython differs here). The symbol table pass does
//! not mangle either: `global __x` in a class body declares `__x`, and the
//! class's `__x = 1` then stores `_C__x` in the class namespace. A class
//! whose name is only underscores makes the code generator fail
//! (`StringIndexOutOfBoundsException`) as soon as it has to mangle a name.

use std::sync::{Mutex, OnceLock};

use rustc_hash::FxHashSet;

/// Whether Jython mangles `name` inside a class.
pub fn is_private(name: &str) -> bool {
    name.starts_with("__") && !name.ends_with("__")
}

/// Whether mangling a private name in class `class` crashes Jython's code
/// generator (the class name has no character other than `_`).
pub fn mangling_crashes(class: &str) -> bool {
    class.bytes().all(|b| b == b'_')
}

/// `name` as Jython spells it inside class `class`: `_<class>__x` for a
/// private name, `name` otherwise (also when the class name is only
/// underscores, where Jython fails instead, see [`mangling_crashes`]).
///
/// Mangled spellings are interned for the life of the process, so the
/// model can keep borrowing names: one allocation per distinct spelling,
/// and only private names in classes have one.
pub fn mangle<'a>(class: &str, name: &'a str) -> &'a str {
    if !is_private(name) || mangling_crashes(class) {
        return name;
    }
    let spelled = format!("_{}{}", class.trim_start_matches('_'), name);
    intern(spelled)
}

fn intern(s: String) -> &'static str {
    static NAMES: OnceLock<Mutex<FxHashSet<&'static str>>> = OnceLock::new();
    let mut names = NAMES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(found) = names.get(s.as_str()) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.into_boxed_str());
    names.insert(leaked);
    leaked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mangles_like_jython() {
        assert_eq!(mangle("C", "__x"), "_C__x");
        assert_eq!(mangle("_C", "__x"), "_C__x");
        assert_eq!(mangle("__F", "___w"), "_F___w");
        assert_eq!(mangle("D_", "__x_"), "_D___x_");
        assert_eq!(mangle("C", "__x__"), "__x__");
        assert_eq!(mangle("C", "__"), "__");
        assert_eq!(mangle("C", "_x"), "_x");
        assert_eq!(mangle("___", "__x"), "__x");
        assert!(mangling_crashes("___"));
        assert!(std::ptr::eq(mangle("C", "__x"), mangle("C", "__x")));
    }
}
