//! Reads of files that a fix may edit.
//!
//! Every fix edits files of the workspace: a script that reads one of them
//! (its bytes, or metadata that depends on them, such as the size or the
//! modification time) observes the edit. `import math` followed by
//! `open('.../m/code.py').read().startswith('import math\n')` prints
//! `False` after `SCR102` removes the unused import. So a fix is safe only
//! when no code in the workspace may read a workspace file, and
//! [`crate::vetted::unknown_code`] reports the first possible file read in
//! [`crate::vetted::UnknownCode::file_read`].
//!
//! A read is fine only when its path is a literal (or a constant
//! concatenation or `os.path.join` of literals) that [`outside_workspace`]
//! proves to name no file iglint edits. Anything else is a possible read
//! of a workspace file: a computed path, a file facility used as a value,
//! a Java file handle reached through a member ([`FILE_MEMBERS`], measured)
//! and every use of a vetted Java class that makes handles from a string
//! other than a direct constructor call with such a literal.
//!
//! What counts as a read, checked on Ignition 8.3's Jython and in the
//! image (see `docs/internal/fix-safety.md`, "File reads"):
//!
//! * the builtins `open` and `file` unless the mode is a literal without
//!   `r`, `+` or `U` (a write-only open observes nothing);
//! * metadata that a fix changes: `os.stat`, `os.lstat` (size, times,
//!   inode), `os.access` (a fix replaces the file, whose owner may change),
//!   `os.path.getsize`, `getmtime`, `getatime`, `getctime` and `samefile`
//!   (a fix replaces the inode). Not `os.path.exists`, `isfile`, `isdir`,
//!   `islink`, `lexists`, `os.listdir` or `os.walk`: fixes never create or
//!   delete a file, so names and existence do not change;
//! * Ignition: `system.file.readFileAsString`, `readFileAsBytes`,
//!   `system.iec61850.writeFile` (it sends a local file), and the legacy
//!   `system.net.httpGet`, `httpPost`, `httpPut` and `httpDelete`, which
//!   open any `java.net.URL`, `file:` included;
//! * the Java classes of [`JAVA_FILE_POLICY`].

use iglint_python::ast::{Expr, Operator};

/// The Java types whose values are file handles: a file, a path, a
/// channel or descriptor, a URL or URI, a zip file, and Ignition's project
/// and resource collection managers and resources (as the gateway loaded
/// them from the files a fix edits). `stubs-gen/gen_vetted.py` measures
/// the members that hand one out ([`file_member`]); its copy of this list
/// is part of the generated signature (`generated_tables_match_vetted_inputs`).
pub const FILE_HANDLE_TYPES: &[&str] = &[
    "java.io.File",
    "java.io.FileDescriptor",
    "java.io.FileInputStream",
    "java.io.FileReader",
    "java.io.RandomAccessFile",
    "java.nio.file.Path",
    "java.nio.file.FileSystem",
    "java.nio.file.FileStore",
    "java.nio.file.DirectoryStream",
    "java.nio.file.spi.FileSystemProvider",
    "java.nio.file.attribute.BasicFileAttributes",
    "java.nio.channels.FileChannel",
    "java.nio.channels.AsynchronousFileChannel",
    "java.util.zip.ZipFile",
    "java.net.URL",
    "java.net.URI",
    "java.net.URLConnection",
    "com.inductiveautomation.ignition.gateway.project.ProjectManager",
    "com.inductiveautomation.ignition.common.resourcecollection.Resource",
    "com.inductiveautomation.ignition.common.resourcecollection.ResourceCollection",
    "com.inductiveautomation.ignition.common.resourcecollection.Snapshot",
    "com.inductiveautomation.ignition.gateway.resourcecollection.ResourceCollectionManager",
    "com.inductiveautomation.ignition.gateway.resourcecollection.ResourceCollectionFileTree",
    "com.inductiveautomation.ignition.gateway.resourcecollection.ResourceDataFile",
    "com.inductiveautomation.ignition.gateway.resourcecollection.StreamingResource",
];

/// How a file facility takes the path it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Facility {
    /// A path argument (position, keyword), and the mode argument of
    /// `open`: a literal mode without `r`, `+` or `U` writes only.
    Path {
        position: usize,
        keyword: &'static str,
        mode: Option<(usize, &'static str)>,
    },
    /// Two path arguments (`os.path.samefile`).
    TwoPaths,
    /// A URL argument: an `http` or `https` literal is fine, a `file:`
    /// literal is a path.
    Url {
        position: usize,
        keyword: &'static str,
    },
}

const fn path(position: usize, keyword: &'static str) -> Facility {
    Facility::Path {
        position,
        keyword,
        mode: None,
    }
}

/// The Python and Ignition facilities that read files or their metadata,
/// by qualified name (`__builtin__.open`, `os.path.getsize`).
pub(crate) const FILE_FACILITIES: &[(&str, Facility)] = &[
    (
        "__builtin__.file",
        Facility::Path {
            position: 0,
            keyword: "name",
            mode: Some((1, "mode")),
        },
    ),
    (
        "__builtin__.open",
        Facility::Path {
            position: 0,
            keyword: "name",
            mode: Some((1, "mode")),
        },
    ),
    ("os.access", path(0, "path")),
    ("os.lstat", path(0, "path")),
    ("os.path.getatime", path(0, "filename")),
    ("os.path.getctime", path(0, "filename")),
    ("os.path.getmtime", path(0, "filename")),
    ("os.path.getsize", path(0, "filename")),
    ("os.path.samefile", Facility::TwoPaths),
    ("os.stat", path(0, "path")),
    ("system.file.readFileAsBytes", path(0, "filepath")),
    ("system.file.readFileAsString", path(0, "filepath")),
    ("system.iec61850.writeFile", path(1, "localFilePath")),
    (
        "system.net.httpDelete",
        Facility::Url {
            position: 0,
            keyword: "url",
        },
    ),
    (
        "system.net.httpGet",
        Facility::Url {
            position: 0,
            keyword: "url",
        },
    ),
    (
        "system.net.httpPost",
        Facility::Url {
            position: 0,
            keyword: "url",
        },
    ),
    (
        "system.net.httpPut",
        Facility::Url {
            position: 0,
            keyword: "url",
        },
    ),
];

/// What a vetted Java class that makes or takes file handles does with
/// its constructor arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JavaPolicy {
    /// Its instances read a file named by the string arguments: a direct
    /// constructor call whose path arguments are literals outside the
    /// workspace is fine; any other use (a computed path, the class used
    /// as a value) may read a workspace file. `all` when every argument is
    /// part of the path (`File(parent, child)`), else only the first.
    Reader { all: bool },
    /// A URL or URI: fine only from an `http`, `https` or outside `file:`
    /// literal.
    Url,
    /// Writes the file it names, or reads only from a handle or stream the
    /// script already holds (whose source is checked): plain.
    Plain,
}

/// The policy of every vetted Java class that is a file handle type or
/// has a public constructor taking one (`JAVA_FILE_CLASSES`, measured;
/// `every_measured_file_class_has_a_policy` checks this table against it).
pub(crate) const JAVA_FILE_POLICY: &[(&str, JavaPolicy)] = &[
    ("java.io.File", JavaPolicy::Reader { all: true }),
    ("java.io.FileInputStream", JavaPolicy::Reader { all: false }),
    // Writers: they truncate or append and never read the file.
    ("java.io.FileOutputStream", JavaPolicy::Plain),
    ("java.io.FileReader", JavaPolicy::Reader { all: false }),
    ("java.io.FileWriter", JavaPolicy::Plain),
    ("java.io.PrintWriter", JavaPolicy::Plain),
    // Made from a `URL` by `openConnection` (a measured file member).
    ("java.net.HttpURLConnection", JavaPolicy::Plain),
    ("java.net.URI", JavaPolicy::Url),
    ("java.net.URL", JavaPolicy::Url),
    ("java.util.Formatter", JavaPolicy::Plain),
    // `Scanner(String)` scans the string; its file constructors take a
    // `File` or `Path`, whose source is checked.
    ("java.util.Scanner", JavaPolicy::Plain),
];

/// The policy of the vetted Java class `qualified`, if it handles files.
pub(crate) fn java_policy(qualified: &str) -> Option<JavaPolicy> {
    JAVA_FILE_POLICY
        .iter()
        .find(|(c, _)| *c == qualified)
        .map(|(_, p)| *p)
}

/// The facility named `qualified`, if it reads files.
pub(crate) fn facility(qualified: &str) -> Option<Facility> {
    FILE_FACILITIES
        .iter()
        .find(|(q, _)| *q == qualified)
        .map(|(_, f)| *f)
}

/// Whether `name`, named as a member of any value, may hand out a file
/// handle (`JAVA_FILE_MEMBERS`, measured over every type a script can hold
/// in the gateway: `getParentFile`, `toPath`, `getDataDir`,
/// `getProjectManager`, ...).
pub fn file_member(name: &str) -> bool {
    crate::generated::JAVA_FILE_MEMBERS
        .binary_search(&name)
        .is_ok()
}

/// The text of `e` when it is a string literal, a concatenation of string
/// literals, or `os.path.join` of string literals (`join` resolved by the
/// caller), with `os.path.join`'s rule that an absolute component starts
/// over.
pub(crate) fn literal_text(e: &Expr, is_join: &dyn Fn(&Expr) -> bool) -> Option<String> {
    match e {
        Expr::Str(s) => Some(s.s.to_latin1().into_owned()),
        Expr::BinOp(b) if b.op == Operator::Add => {
            let mut left = literal_text(&b.left, is_join)?;
            left.push_str(&literal_text(&b.right, is_join)?);
            Some(left)
        }
        Expr::Call(c)
            if is_join(&c.func)
                && c.keywords.is_empty()
                && c.starargs.is_none()
                && c.kwargs.is_none()
                && !c.args.is_empty() =>
        {
            let mut out = String::new();
            for a in &c.args {
                let part = literal_text(a, is_join)?;
                if part.starts_with('/') || out.is_empty() {
                    out = part;
                } else {
                    if !out.ends_with('/') {
                        out.push('/');
                    }
                    out.push_str(&part);
                }
            }
            Some(out)
        }
        _ => None,
    }
}

/// Whether the literal path `text` provably names no file that iglint may
/// edit, wherever the workspace is deployed.
///
/// Files iglint edits are resources: they live in a project
/// (`.../projects/<name>/<module>/<type>/...`, or a project directory
/// linted on its own) or in a gateway resource collection
/// (`.../config/resources/<collection>/<module>/<type>/...`), always below
/// a module directory: `ignition` or a module id, which for every module
/// of 8.3 is a dotted name (`com.inductiveautomation.perspective`), and the
/// project and resource manifests `project.json` and `resource.json`. A
/// path is possibly in the workspace when it is relative (the gateway's
/// working directory is its installation directory), starts with `~`,
/// has a `..` component (a symbolic link makes `..` leave a directory), a
/// percent escape, or a component that is (case-insensitively) `projects`,
/// `config`, `resources` or `ignition`, contains `inductiveautomation`, is
/// a directory name containing a dot after its first character (a module
/// id), or is a manifest. Everything else, `/tmp/report.csv`,
/// `C:\exports\x.csv`, is outside.
pub fn outside_workspace(text: &str) -> bool {
    let absolute = text.starts_with('/')
        || text.starts_with("\\\\")
        || (text.len() >= 3
            && text.as_bytes()[0].is_ascii_alphabetic()
            && text.as_bytes()[1] == b':'
            && matches!(text.as_bytes()[2], b'/' | b'\\'));
    if !absolute || text.contains('%') || text.contains('\0') {
        return false;
    }
    let lower = text.to_ascii_lowercase();
    let parts: Vec<&str> = lower.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let last = parts.len().saturating_sub(1);
    for (i, part) in parts.iter().enumerate() {
        if *part == ".." {
            return false;
        }
        // Windows ignores trailing dots and spaces.
        let part = part.trim_end_matches([' ', '.']);
        if part.starts_with('~')
            || matches!(part, "projects" | "config" | "resources" | "ignition")
            || part.contains("inductiveautomation")
            || (i == last && matches!(part, "project.json" | "resource.json"))
            || (i < last && part.len() > 1 && part[1..].contains('.'))
        {
            return false;
        }
    }
    true
}

/// Whether the literal URL `text` reads no workspace file: `http`,
/// `https` and `ftp` URLs are fine, a `file:` URL is a path, anything else
/// (`jar:file:`, no scheme, a relative URL) may be.
pub fn url_outside_workspace(text: &str) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    if ["http://", "https://", "ftp://"]
        .iter()
        .any(|s| lower.starts_with(s))
    {
        return true;
    }
    match lower.strip_prefix("file:") {
        Some(rest) => {
            let rest = rest.strip_prefix("//localhost").unwrap_or(rest);
            let rest = match rest.strip_prefix("//") {
                // `file://host/...` names another machine's path, or with
                // an empty host a local one.
                Some(r) => r.find('/').map_or("", |i| &r[i..]),
                None => rest,
            };
            outside_workspace(&text.trim()[text.trim().len() - rest.len()..])
        }
        None => false,
    }
}

/// Whether a literal mode of `open` only writes (`'w'`, `'wb'`, `'a'`).
pub(crate) fn write_only_mode(mode: &str) -> bool {
    !mode.is_empty() && !mode.contains(['r', '+', 'U'])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_paths() {
        for p in [
            "/tmp/report.csv",
            "/var/exports/x.csv",
            "C:\\exports\\x.csv",
            "C:/exports/x.csv",
            "\\\\server\\share\\x.csv",
            "/home/u/.cache/x",
            "/etc/hosts",
        ] {
            assert!(outside_workspace(p), "{p}");
        }
        for p in [
            "code.py",
            "data/projects/p/x",
            "./x",
            "~/x",
            "/w/projects/p/ignition/script-python/m/code.py",
            "/usr/local/bin/ignition/data/x",
            "/opt/Ignition/x",
            "/data/config/resources/core/x",
            "/srv/p/com.inductiveautomation.perspective/views/v/view.json",
            "/srv/p/project.json",
            "/srv/p/x/resource.json",
            "/srv/my.module.id/x",
            "/tmp/../srv/x",
            "/tmp/%2e%2e/x",
            "C:\\Program Files\\Inductive Automation\\Ignition\\data\\x",
            "/srv/PROJECTS/x",
            "/srv/projects./x",
        ] {
            assert!(!outside_workspace(p), "{p}");
        }
    }

    #[test]
    fn urls() {
        for u in [
            "http://h/x",
            "HTTPS://h/projects/x",
            "file:///tmp/x",
            "file:/tmp/x",
        ] {
            assert!(url_outside_workspace(u), "{u}");
        }
        for u in [
            "file:///w/projects/p/x",
            "file:x",
            "jar:file:/tmp/x.jar!/a",
            "/tmp/x",
            "x",
            "file://localhost/w/projects/x",
        ] {
            assert!(!url_outside_workspace(u), "{u}");
        }
    }

    #[test]
    fn modes() {
        for m in ["w", "wb", "a", "ab"] {
            assert!(write_only_mode(m), "{m}");
        }
        for m in ["", "r", "rb", "w+", "a+", "rU", "U"] {
            assert!(!write_only_mode(m), "{m}");
        }
    }

    /// Every vetted Java class measured to make or take file handles has a
    /// policy, and every policy names such a class.
    #[test]
    fn every_measured_file_class_has_a_policy() {
        let measured = crate::generated::JAVA_FILE_CLASSES;
        for c in measured {
            assert!(java_policy(c).is_some(), "{c} has no file policy");
        }
        for (c, _) in JAVA_FILE_POLICY {
            assert!(measured.contains(c), "{c} is not a measured file class");
        }
        assert!(
            crate::generated::JAVA_FILE_MEMBERS
                .windows(2)
                .all(|w| w[0] < w[1])
        );
        assert!(FILE_FACILITIES.windows(2).all(|w| w[0].0 < w[1].0));
        for name in ["getParentFile", "toPath", "getDataDir", "getProjectManager"] {
            assert!(file_member(name), "{name}");
        }
    }
}
