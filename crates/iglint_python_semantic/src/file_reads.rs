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
//! Access is default deny: every vetted facility that takes a path or a
//! file descriptor ([`FILE_FACILITIES`]: `open` and `file` in every mode,
//! the path-taking attributes of `os` and `os.path`, the `system.*`
//! functions that name local files) and every vetted Java class that
//! makes a file handle from a string ([`JAVA_FILE_POLICY`]) is a possible
//! access of a workspace file, whatever the direction of the operation:
//! reads observe a fix, an append reveals the size (`tell`), writes,
//! renames and deletions change what a fix edits or move a workspace file
//! to a path that a later literal read observes. An access is fine only
//! when every path argument is a literal (or a constant concatenation or
//! `os.path.join` of literals) that [`outside_workspace`] proves to name
//! no file iglint edits. Anything else counts: a computed path, a
//! facility used as a value, a Java file handle reached through a member
//! ([`file_member`], measured). The attributes of `os`, `os.path` and
//! `sys` that take no path are reviewed one by one (`PATH_FREE`); a test
//! fails on a vetted attribute in neither list. See
//! `docs/internal/fix-safety.md`, "File reads", for why literal outside
//! paths stay fine (an alias needs an operation that is itself counted)
//! and why pre-existing symbolic links are outside the closed world.

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

/// How a file facility takes the paths it may touch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Facility {
    /// Path (or file descriptor) arguments, by position and keyword: the
    /// call is fine only when every one of them is present and a literal
    /// outside the workspace. The mode of `open`, the direction of the
    /// operation (read, write, append, rename, delete) and the other
    /// arguments do not matter.
    Paths(&'static [(usize, &'static str)]),
    /// A URL argument: an `http` or `https` literal is fine, a `file:`
    /// literal is a path.
    Url {
        position: usize,
        keyword: &'static str,
    },
    /// The name of a file the facility makes in a directory of its own
    /// choosing (the extension of `system.file.getTempFile`): fine only as
    /// a literal without a separator or `..`.
    Name {
        position: usize,
        keyword: &'static str,
    },
    /// The arguments name files in ways that are not modelled (a file
    /// dialog, a report's distribution actions, a JDBC URL, a sound clip
    /// that may be a path): every call counts.
    Always,
}

const fn url(position: usize, keyword: &'static str) -> Facility {
    Facility::Url { position, keyword }
}

/// Every vetted facility that takes a path or a file descriptor, by
/// qualified name (`__builtin__.open`, `os.rename`, `os.path.exists`,
/// `system.file.writeFile`). Default deny: any of them may touch a
/// workspace file unless every path argument is a literal outside the
/// workspace, whatever the operation. Reads observe a fix; writes,
/// appends (`open(p, "a")` then `tell()` reveals the size), renames,
/// copies and deletions change the files a fix edits, or move a
/// workspace file to a path that a later literal read then observes
/// (`os.rename(<workspace file>, '/tmp/m.py')`). Every attribute of `os`,
/// `os.path` and `sys` that is vetted is either here or in `PATH_FREE`
/// (`every_vetted_os_attribute_is_classified`), and every `system.*`
/// function with a parameter that may name a local file is here or
/// reviewed (`every_system_file_parameter_is_classified`).
pub(crate) const FILE_FACILITIES: &[(&str, Facility)] = &[
    ("__builtin__.file", Facility::Paths(&[(0, "name")])),
    ("__builtin__.open", Facility::Paths(&[(0, "name")])),
    ("os.access", Facility::Paths(&[(0, "path")])),
    ("os.listdir", Facility::Paths(&[(0, "path")])),
    ("os.lstat", Facility::Paths(&[(0, "path")])),
    ("os.makedirs", Facility::Paths(&[(0, "name")])),
    ("os.mkdir", Facility::Paths(&[(0, "path")])),
    ("os.path.exists", Facility::Paths(&[(0, "path")])),
    ("os.path.getatime", Facility::Paths(&[(0, "filename")])),
    ("os.path.getctime", Facility::Paths(&[(0, "filename")])),
    ("os.path.getmtime", Facility::Paths(&[(0, "filename")])),
    ("os.path.getsize", Facility::Paths(&[(0, "filename")])),
    ("os.path.isdir", Facility::Paths(&[(0, "s")])),
    ("os.path.isfile", Facility::Paths(&[(0, "path")])),
    ("os.path.islink", Facility::Paths(&[(0, "path")])),
    ("os.path.ismount", Facility::Paths(&[(0, "path")])),
    ("os.path.lexists", Facility::Paths(&[(0, "path")])),
    ("os.path.realpath", Facility::Paths(&[(0, "filename")])),
    ("os.path.samefile", Facility::Paths(&[(0, "f1"), (1, "f2")])),
    (
        "os.path.sameopenfile",
        Facility::Paths(&[(0, "fp1"), (1, "fp2")]),
    ),
    ("os.path.walk", Facility::Paths(&[(0, "top")])),
    ("os.remove", Facility::Paths(&[(0, "path")])),
    ("os.removedirs", Facility::Paths(&[(0, "name")])),
    ("os.rename", Facility::Paths(&[(0, "old"), (1, "new")])),
    ("os.renames", Facility::Paths(&[(0, "old"), (1, "new")])),
    ("os.rmdir", Facility::Paths(&[(0, "path")])),
    ("os.stat", Facility::Paths(&[(0, "path")])),
    ("os.unlink", Facility::Paths(&[(0, "path")])),
    ("os.utime", Facility::Paths(&[(0, "path")])),
    ("os.walk", Facility::Paths(&[(0, "top")])),
    (
        "system.dataset.exportCSV",
        Facility::Paths(&[(0, "filename")]),
    ),
    (
        "system.dataset.exportExcel",
        Facility::Paths(&[(0, "filename")]),
    ),
    (
        "system.dataset.exportHTML",
        Facility::Paths(&[(0, "filename")]),
    ),
    ("system.db.addDatasource", Facility::Always),
    ("system.db.setDatasourceConnectURL", Facility::Always),
    (
        "system.file.fileExists",
        Facility::Paths(&[(0, "filepath")]),
    ),
    (
        "system.file.getTempFile",
        Facility::Name {
            position: 0,
            keyword: "extension",
        },
    ),
    ("system.file.openFile", Facility::Always),
    ("system.file.openFiles", Facility::Always),
    (
        "system.file.readFileAsBytes",
        Facility::Paths(&[(0, "filepath")]),
    ),
    (
        "system.file.readFileAsString",
        Facility::Paths(&[(0, "filepath")]),
    ),
    ("system.file.saveFile", Facility::Always),
    ("system.file.saveToFile", Facility::Always),
    ("system.file.saveToFileNonBlocking", Facility::Always),
    ("system.file.writeFile", Facility::Paths(&[(0, "filepath")])),
    (
        "system.iec61850.readFile",
        Facility::Paths(&[(2, "localFilePath")]),
    ),
    (
        "system.iec61850.writeFile",
        Facility::Paths(&[(1, "localFilePath")]),
    ),
    ("system.net.httpDelete", url(0, "url")),
    ("system.net.httpGet", url(0, "url")),
    ("system.net.httpPost", url(0, "url")),
    ("system.net.httpPut", url(0, "url")),
    ("system.net.openURL", url(0, "url")),
    (
        "system.print.printToImage",
        Facility::Paths(&[(1, "filename")]),
    ),
    ("system.report.executeAndDistribute", Facility::Always),
    ("system.tag.exportTags", Facility::Paths(&[(0, "filePath")])),
    ("system.util.playSoundClip", Facility::Always),
    (
        "system.vision.exportCSV",
        Facility::Paths(&[(0, "filename")]),
    ),
    (
        "system.vision.exportExcel",
        Facility::Paths(&[(0, "filename")]),
    ),
    (
        "system.vision.exportHTML",
        Facility::Paths(&[(0, "filename")]),
    ),
    ("system.vision.openFile", Facility::Always),
    ("system.vision.openFiles", Facility::Always),
    ("system.vision.openURL", url(0, "url")),
    (
        "system.vision.printToImage",
        Facility::Paths(&[(1, "filename")]),
    ),
    ("system.vision.saveFile", Facility::Always),
];

/// The vetted attributes of `os`, `os.path` and `sys` that take no path
/// and no file descriptor, reviewed one by one: constants, the
/// environment, the working directory (a path source: what is done with
/// the path is checked where it is used), error types, random bytes, the
/// console streams, and the pure string functions of `os.path`, which
/// compute a path without touching the file system (`abspath` and
/// `relpath` only read the working directory, `expanduser` and
/// `expandvars` the environment).
#[cfg(test)]
pub(crate) const PATH_FREE: &[&str] = &[
    "os.F_OK",
    "os.R_OK",
    "os.W_OK",
    "os.X_OK",
    "os.altsep",
    "os.curdir",
    "os.devnull",
    "os.environ",
    "os.error",
    "os.extsep",
    "os.getcwd",
    "os.getcwdu",
    "os.getenv",
    "os.linesep",
    "os.name",
    "os.pardir",
    "os.path",
    "os.path.abspath",
    "os.path.altsep",
    "os.path.basename",
    "os.path.commonprefix",
    "os.path.curdir",
    "os.path.defpath",
    "os.path.devnull",
    "os.path.dirname",
    "os.path.expanduser",
    "os.path.expandvars",
    "os.path.extsep",
    "os.path.isabs",
    "os.path.join",
    "os.path.normcase",
    "os.path.normpath",
    "os.path.pardir",
    "os.path.pathsep",
    "os.path.relpath",
    "os.path.samestat",
    "os.path.sep",
    "os.path.split",
    "os.path.splitdrive",
    "os.path.splitext",
    "os.path.supports_unicode_filenames",
    "os.pathsep",
    "os.sep",
    "os.strerror",
    "os.urandom",
    "sys.byteorder",
    "sys.exit",
    "sys.float_info",
    "sys.getdefaultencoding",
    "sys.getfilesystemencoding",
    "sys.getrecursionlimit",
    "sys.hexversion",
    "sys.maxint",
    "sys.maxsize",
    "sys.maxunicode",
    "sys.platform",
    "sys.stderr",
    "sys.stdout",
    "sys.version",
    "sys.version_info",
];

/// `system.*` parameters whose names look like local files but are not,
/// reviewed against the 8.3 documentation: names and URLs that the
/// browser or a remote device resolves, OPC servers' folders and
/// endpoints, a report's output format, tag paths.
#[cfg(test)]
pub(crate) const SYSTEM_NOT_LOCAL_FILES: &[&str] = &[
    // Folders of an OPC server's address space.
    "system.opc.browse",
    "system.opc.browseSimple",
    // The URLs of an OPC UA server.
    "system.opcua.addConnection",
    // A remote device's file system.
    "system.iec61850.listFiles",
    // The name under which the browser saves the data it is sent.
    "system.perspective.download",
    // The browser navigates.
    "system.perspective.navigate",
    // Prints a message; the destination is the browser console, the
    // gateway log or both.
    "system.perspective.print",
    // The output format (`pdf`, `csv`, ...): the report comes back as bytes.
    "system.report.executeReport",
    // Tag paths.
    "system.tag.move",
];

/// What a vetted Java class that makes or takes file handles does with
/// its constructor arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JavaPolicy {
    /// Its instances open, create or truncate the file named by its
    /// arguments, for reading or writing alike: a direct constructor call
    /// whose path arguments are literals outside the workspace is fine;
    /// any other use (a computed path, the class used as a value) may
    /// touch a workspace file. `all` when every argument is part of the
    /// path (`File(parent, child)`), else only the first (the others are
    /// an append flag or a charset).
    Path { all: bool },
    /// The first argument may be a file name or something else (a
    /// `Writer`, an `OutputStream`, a `Locale`): fine without arguments,
    /// with a literal outside the workspace, or with a direct constructor
    /// call of a vetted Java class that has no file policy
    /// (`PrintWriter(StringWriter())`).
    MaybePath,
    /// A URL or URI: fine only from an `http`, `https` or outside `file:`
    /// literal.
    Url,
    /// Takes no path, only a handle or stream the script already holds,
    /// whose source is checked where it is made: plain.
    Plain,
}

/// The policy of every vetted Java class that is a file handle type or
/// has a public constructor taking one (`JAVA_FILE_CLASSES`, measured;
/// `every_measured_file_class_has_a_policy` checks this table against it).
pub(crate) const JAVA_FILE_POLICY: &[(&str, JavaPolicy)] = &[
    ("java.io.File", JavaPolicy::Path { all: true }),
    ("java.io.FileInputStream", JavaPolicy::Path { all: false }),
    // Writers count too: appending reveals nothing by itself, but a
    // script that writes, truncates or creates a workspace file changes
    // what a fix edits.
    ("java.io.FileOutputStream", JavaPolicy::Path { all: false }),
    ("java.io.FileReader", JavaPolicy::Path { all: false }),
    ("java.io.FileWriter", JavaPolicy::Path { all: false }),
    // `PrintWriter(String fileName)` and `Formatter(String fileName)`
    // open the file; their other constructors take a writer, a stream or
    // a locale.
    ("java.io.PrintWriter", JavaPolicy::MaybePath),
    // Made from a `URL` by `openConnection` (a measured file member).
    ("java.net.HttpURLConnection", JavaPolicy::Plain),
    ("java.net.URI", JavaPolicy::Url),
    ("java.net.URL", JavaPolicy::Url),
    ("java.util.Formatter", JavaPolicy::MaybePath),
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

/// Whether the literal `text` is a bare file name (no separator, no
/// `..`, no NUL) for [`Facility::Name`].
pub(crate) fn bare_name(text: &str) -> bool {
    !text.contains(['/', '\\', '\0', ':']) && !text.contains("..")
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
    fn bare_names() {
        for n in ["csv", "x.csv", ""] {
            assert!(bare_name(n), "{n}");
        }
        for n in ["../x", "a/b", "a\\b", "..", "C:x"] {
            assert!(!bare_name(n), "{n}");
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
