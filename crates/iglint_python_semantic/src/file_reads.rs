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
/// channel or descriptor, a URL or URI, a zip file, Ignition's project
/// and resource collection managers and resources (as the gateway loaded
/// them from the files a fix edits), and the configuration objects that
/// carry the content of those files (a view's configuration, a tag's
/// event scripts). `stubs-gen/gen_vetted.py` measures
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
    // Configuration objects that hold resource content as the gateway
    // loaded it (review round 12): Perspective view, component, page,
    // session and project configurations, tag configurations, event
    // scripts and alarm definitions, gateway event scripts, named queries
    // and decoded gateway resources. Measured to add 20 member names
    // (`getEventScripts`, `findViewConfig`, `getPropertyConfig`,
    // `getQueryFromPath`, `getTagConfigsAsync`, ...).
    "com.inductiveautomation.perspective.common.config.ViewConfig",
    "com.inductiveautomation.perspective.common.config.ComponentConfig",
    "com.inductiveautomation.perspective.common.config.PageConfig",
    "com.inductiveautomation.perspective.common.config.PagesConfig",
    "com.inductiveautomation.perspective.common.config.SessionPropsConfig",
    "com.inductiveautomation.perspective.common.config.PropertyConfig",
    "com.inductiveautomation.perspective.common.config.PropertyConfigCollection",
    "com.inductiveautomation.perspective.common.config.BindingConfig",
    "com.inductiveautomation.perspective.common.config.ActionConfig",
    "com.inductiveautomation.perspective.common.config.EventConfig",
    "com.inductiveautomation.perspective.common.config.ScriptConfig",
    "com.inductiveautomation.perspective.common.config.CustomMethodConfig",
    "com.inductiveautomation.perspective.common.config.MessageHandlerConfig",
    "com.inductiveautomation.perspective.common.config.ExtensionFunctionConfig",
    "com.inductiveautomation.perspective.common.config.PropertyChangeScriptConfig",
    "com.inductiveautomation.perspective.common.config.SystemEventsConfig",
    "com.inductiveautomation.perspective.common.config.DocksConfig",
    "com.inductiveautomation.perspective.common.config.DockedViewConfig",
    "com.inductiveautomation.perspective.common.config.PerspectiveProjectProps",
    "com.inductiveautomation.perspective.gateway.api.PerspectiveProject",
    "com.inductiveautomation.perspective.gateway.api.PerspectiveProjectCache",
    "com.inductiveautomation.perspective.gateway.config.ProjectConfig",
    "com.inductiveautomation.perspective.gateway.model.ViewModel",
    "com.inductiveautomation.ignition.common.tags.config.TagConfiguration",
    "com.inductiveautomation.ignition.common.tags.config.TagConfigurationModel",
    "com.inductiveautomation.ignition.common.sqltags.model.scripts.TagEventScripts",
    "com.inductiveautomation.ignition.common.alarming.config.AlarmConfiguration",
    "com.inductiveautomation.ignition.common.alarming.config.AlarmDefinition",
    "com.inductiveautomation.ignition.common.script.builtin.ialabs.TagAlarmDefinition",
    "com.inductiveautomation.ignition.common.script.ScheduledScript",
    "com.inductiveautomation.ignition.common.script.TagChangeScript",
    "com.inductiveautomation.ignition.common.script.ScriptConfig$MessageHandlerScript",
    "com.inductiveautomation.ignition.common.script.ScriptConfig$ShutdownScript",
    "com.inductiveautomation.ignition.common.script.ScriptConfig$StartupScript",
    "com.inductiveautomation.ignition.common.script.ScriptConfig$TagChangeScriptEvent",
    "com.inductiveautomation.ignition.common.script.ScriptConfig$TimerScript",
    "com.inductiveautomation.ignition.common.db.namedquery.NamedQuery",
    "com.inductiveautomation.ignition.gateway.config.DecodedResource",
    "com.inductiveautomation.ignition.gateway.tags.scripting.CompiledTagScript",
    "com.inductiveautomation.ignition.gateway.tags.scripting.events.AbstractTagScript",
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
    // The legacy Vision exports (unnamed parameters): the file name first.
    ("system.db.exportCSV", Facility::Paths(&[(0, "filename")])),
    ("system.db.exportExcel", Facility::Paths(&[(0, "filename")])),
    ("system.db.exportHTML", Facility::Paths(&[(0, "filename")])),
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
];

/// What a `system.*` function's outputs (its return value, the files it
/// writes, what it hands to a callback) carry, for the functions whose
/// outputs may carry content that the gateway loaded from workspace
/// resources: such a function is a possible read of the files a fix
/// edits, whatever its parameters. Reviewed for every function and object
/// of `crates/iglint_stubs` (all scopes); the others take no part
/// (`SYSTEM_VALUE_ONLY`, or `vetted::SYSTEM_RUNS_CODE`), and
/// `every_system_function_output_is_classified` fails on a function in
/// none or more than one of the lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    /// Tag or project configuration, resource listings with content, or
    /// Vision windows and components (which hold their window's
    /// definition): every use counts, called or as a value.
    Content,
    /// Tag reads: the value of each path, and a path that names a
    /// configuration property (`Tag.eventScripts`, `Tag.expression`)
    /// returns that configuration (verified on 8.3.3:
    /// `readBlocking(['[default]T.eventScripts'])`, `getTagValue`,
    /// `getAttribute`). Fine only as a direct call whose path argument
    /// (the first, or `tagPaths=` / `tagPath=`) can only name values
    /// (`vetted::value_write_paths`: no property, `Value`, `Quality`,
    /// `Timestamp`); computed paths count. Values that carry
    /// configuration because an expression, binding or reference tag reads
    /// a configuration property are counted at that reference.
    TagValues,
    /// A network client: the gateway that runs the script, or another one
    /// that shares the workspace's files, serves their content over HTTP
    /// (the 8.3 REST API's project and resource exports, WebDev resources),
    /// and no host name can be proven not to reach one (`localhost`, an
    /// alias, a proxy, the gateway's own DNS name). Every use counts; there
    /// is no list of trusted hosts, since declaring one harmless is the
    /// declaration `assume-scripts-do-not-read-workspace-files` makes.
    Network,
}

/// The `system.*` functions and objects whose outputs may carry workspace
/// content ([`Output`]), sorted by name.
pub(crate) const SYSTEM_OUTPUTS: &[(&str, Output)] = &[
    ("system.device.getInstance", Output::Content),
    ("system.file.getChooser", Output::Content),
    ("system.gui.desktop", Output::Content),
    ("system.gui.find", Output::Content),
    ("system.gui.findWindow", Output::Content),
    ("system.gui.getOpenedWindows", Output::Content),
    ("system.gui.getParentWindow", Output::Content),
    ("system.gui.getSibling", Output::Content),
    ("system.gui.getWindow", Output::Content),
    ("system.gui.openDesktop", Output::Content),
    ("system.gui.openWindow", Output::Content),
    ("system.gui.swapWindow", Output::Content),
    ("system.nav.desktop", Output::Content),
    ("system.nav.goBack", Output::Content),
    ("system.nav.goForward", Output::Content),
    ("system.nav.goHome", Output::Content),
    ("system.nav.openWindow", Output::Content),
    ("system.nav.openWindowImpl", Output::Content),
    ("system.nav.openWindowInstance", Output::Content),
    ("system.nav.swapTo", Output::Content),
    ("system.nav.swapWindow", Output::Content),
    ("system.net.httpClient", Output::Network),
    ("system.net.httpDelete", Output::Network),
    ("system.net.httpGet", Output::Network),
    ("system.net.httpPost", Output::Network),
    ("system.net.httpPut", Output::Network),
    ("system.perspective.getProjectInfo", Output::Content),
    ("system.security._tempCloseAll", Output::Content),
    ("system.system.queryAuditLog", Output::Content),
    ("system.tag.browse", Output::Content),
    ("system.tag.browseTags", Output::Content),
    ("system.tag.browseTagsSimple", Output::Content),
    ("system.tag.exportTags", Output::Content),
    ("system.tag.getAlarmStates", Output::Content),
    ("system.tag.getAttribute", Output::Content),
    ("system.tag.getConfiguration", Output::Content),
    ("system.tag.getTagValue", Output::TagValues),
    ("system.tag.getTagValues", Output::TagValues),
    ("system.tag.query", Output::Content),
    ("system.tag.read", Output::TagValues),
    ("system.tag.readAll", Output::TagValues),
    ("system.tag.readAsync", Output::TagValues),
    ("system.tag.readBlocking", Output::TagValues),
    ("system.util.initialize", Output::Content),
    ("system.util.queryAuditLog", Output::Content),
    ("system.vision.desktop", Output::Content),
    ("system.vision.findWindow", Output::Content),
    ("system.vision.getOpenedWindows", Output::Content),
    ("system.vision.getParentWindow", Output::Content),
    ("system.vision.getSibling", Output::Content),
    ("system.vision.getWindow", Output::Content),
    ("system.vision.goBack", Output::Content),
    ("system.vision.goForward", Output::Content),
    ("system.vision.goHome", Output::Content),
    ("system.vision.openDesktop", Output::Content),
    ("system.vision.openWindow", Output::Content),
    ("system.vision.openWindowInstance", Output::Content),
    ("system.vision.swapTo", Output::Content),
    ("system.vision.swapWindow", Output::Content),
];

/// The output of the vetted `system.*` function or object `qualified`,
/// when it may carry workspace content.
pub(crate) fn system_output(qualified: &str) -> Option<Output> {
    SYSTEM_OUTPUTS
        .binary_search_by(|(q, _)| (*q).cmp(qualified))
        .ok()
        .map(|i| SYSTEM_OUTPUTS[i].1)
}

/// The `system.*` functions and objects whose outputs carry no workspace
/// content, reviewed per package: run-time values that the running
/// configuration produces (a safe fix keeps behaviour, so it keeps them),
/// settings values, names (no fix creates, renames or deletes a resource)
/// and data the script passes in. Sorted.
#[cfg(test)]
pub(crate) const SYSTEM_VALUE_ONLY: &[&str] = &[
    // `system.alarm`: alarm events, states and rosters: run-time values (an
    // alarm property bound to a configuration property is counted at the
    // binding); pipeline names.
    "system.alarm.acknowledge",
    "system.alarm.cancel",
    "system.alarm.createRoster",
    "system.alarm.getRosters",
    "system.alarm.getShelvedPaths",
    "system.alarm.listPipelines",
    "system.alarm.processPathCondition",
    "system.alarm.processStringEnumCondition",
    "system.alarm.queryJournal",
    "system.alarm.queryStatus",
    "system.alarm.shelve",
    "system.alarm.unshelve",
    // `system.alert`: legacy alert states and history: run-time values.
    "system.alert.acknowledgeAlert",
    "system.alert.queryAlertHistory",
    "system.alert.queryAlertStatus",
    "system.alert.toDataset",
    // `system.bacnet`: device reads and writes.
    "system.bacnet.readRaw",
    "system.bacnet.readRawMultiple",
    "system.bacnet.synchronizeTime",
    "system.bacnet.synchronizeTimeUtc",
    "system.bacnet.writeRaw",
    "system.bacnet.writeRawMultiple",
    "system.bacnet.writeWithPriority",
    // `system.dataset`: datasets the script passes in; the file exports are
    // file facilities.
    "system.dataset.addColumn",
    "system.dataset.addRow",
    "system.dataset.addRows",
    "system.dataset.appendDataset",
    "system.dataset.clearDataset",
    "system.dataset.dataSetToCSV",
    "system.dataset.dataSetToExcel",
    "system.dataset.dataSetToExcelBytes",
    "system.dataset.dataSetToExcelStreaming",
    "system.dataset.dataSetToHTML",
    "system.dataset.dataSetToHTMLStreaming",
    "system.dataset.deleteRow",
    "system.dataset.deleteRows",
    "system.dataset.exportCSV",
    "system.dataset.exportExcel",
    "system.dataset.exportHTML",
    "system.dataset.filterColumns",
    "system.dataset.formatDates",
    "system.dataset.fromCSV",
    "system.dataset.fromCSVJava",
    "system.dataset.getColumnHeaders",
    "system.dataset.insertColumn",
    "system.dataset.insertRow",
    "system.dataset.removeRow",
    "system.dataset.setValue",
    "system.dataset.sort",
    "system.dataset.toCSV",
    "system.dataset.toCSVJava",
    "system.dataset.toCSVJavaStreaming",
    "system.dataset.toDataSet",
    "system.dataset.toDataset",
    "system.dataset.toExcel",
    "system.dataset.toJSONObject",
    "system.dataset.toPyDataSet",
    "system.dataset.updateRow",
    // `system.date`: dates.
    "system.date.addDays",
    "system.date.addHours",
    "system.date.addMillis",
    "system.date.addMinutes",
    "system.date.addMonths",
    "system.date.addSeconds",
    "system.date.addWeeks",
    "system.date.addYears",
    "system.date.daysBetween",
    "system.date.format",
    "system.date.fromMillis",
    "system.date.getAMorPM",
    "system.date.getDate",
    "system.date.getDayOfMonth",
    "system.date.getDayOfWeek",
    "system.date.getDayOfYear",
    "system.date.getHour12",
    "system.date.getHour24",
    "system.date.getMillis",
    "system.date.getMinute",
    "system.date.getMonth",
    "system.date.getQuarter",
    "system.date.getSecond",
    "system.date.getTimezone",
    "system.date.getTimezoneOffset",
    "system.date.getTimezoneRawOffset",
    "system.date.getYear",
    "system.date.hoursBetween",
    "system.date.isAfter",
    "system.date.isBefore",
    "system.date.isBetween",
    "system.date.isDaylightTime",
    "system.date.midnight",
    "system.date.millisBetween",
    "system.date.minutesBetween",
    "system.date.monthsBetween",
    "system.date.now",
    "system.date.parse",
    "system.date.secondsBetween",
    "system.date.setTime",
    "system.date.toMillis",
    "system.date.weeksBetween",
    "system.date.yearsBetween",
    // `system.db`: database results, connection status and transactions
    // (named query results, not their text); the database holds no resource
    // content the gateway put there on its own. The legacy exports write
    // the dataset they are given (file facilities).
    "system.db._callSProc",
    "system.db.addDatasource",
    "system.db.beginNamedQueryTransaction",
    "system.db.beginTransaction",
    "system.db.clearAllNamedQueryCaches",
    "system.db.clearNamedQueryCache",
    "system.db.clearQueryCache",
    "system.db.closeTransaction",
    "system.db.commitTransaction",
    "system.db.createSProcCall",
    "system.db.dataSetToCSV",
    "system.db.dateFormat",
    "system.db.execQuery",
    "system.db.execSProcCall",
    "system.db.execScalar",
    "system.db.execUpdate",
    "system.db.execUpdateAsync",
    "system.db.exportCSV",
    "system.db.exportExcel",
    "system.db.exportHTML",
    "system.db.getConnection",
    "system.db.getConnectionInfo",
    "system.db.getConnections",
    "system.db.refresh",
    "system.db.removeDatasource",
    "system.db.rollbackTransaction",
    "system.db.runNamedQuery",
    "system.db.runPrepQuery",
    "system.db.runPrepStmt",
    "system.db.runPrepStmtGetKey",
    "system.db.runPrepUpdate",
    "system.db.runQuery",
    "system.db.runSFNamedQuery",
    "system.db.runSFPrepUpdate",
    "system.db.runSFUpdateQuery",
    "system.db.runScalarPrepQuery",
    "system.db.runScalarQuery",
    "system.db.runUpdateQuery",
    "system.db.runUpdateQueryGetKey",
    "system.db.setDatasourceConnectURL",
    "system.db.setDatasourceEnabled",
    "system.db.setDatasourceMaxConnections",
    "system.db.toDataSet",
    "system.db.toPyDataSet",
    // `system.device`: device settings and status.
    "system.device.addDevice",
    "system.device.addDeviceInternal",
    "system.device.getDeviceHostname",
    "system.device.listDevices",
    "system.device.refreshBrowse",
    "system.device.removeDevice",
    "system.device.restart",
    "system.device.setDeviceEnabled",
    "system.device.setDeviceHostname",
    // `system.dnp`: device operations.
    "system.dnp.demandPoll",
    "system.dnp.directOperateAnalog",
    "system.dnp.directOperateBinary",
    "system.dnp.freezeAnalogs",
    "system.dnp.freezeAtTimeAnalogs",
    "system.dnp.freezeAtTimeCounters",
    "system.dnp.freezeClearAnalogs",
    "system.dnp.freezeClearCounters",
    "system.dnp.freezeCounters",
    "system.dnp.selectOperateAnalog",
    "system.dnp.selectOperateBinary",
    "system.dnp.synchronizeTime",
    // `system.dnp3`: device operations.
    "system.dnp3.directOperateAnalog",
    "system.dnp3.directOperateBinary",
    "system.dnp3.freezeAnalogs",
    "system.dnp3.freezeAnalogsAtTime",
    "system.dnp3.freezeCounters",
    "system.dnp3.freezeCountersAtTime",
    "system.dnp3.selectOperateAnalog",
    "system.dnp3.selectOperateBinary",
    // `system.eam`: agent status and history.
    "system.eam.getGroups",
    "system.eam.isController",
    "system.eam.queryAgentHistory",
    "system.eam.queryAgentStatus",
    // `system.eventstream`: event stream names and diagnostics (event
    // streams are resources iglint does not read: unknown code).
    "system.eventstream.getDiagnostics",
    "system.eventstream.listEventStreams",
    "system.eventstream.publishEvent",
    // `system.file`: the file facilities (their outputs come from the paths
    // they take, checked as file facilities) and file dialogs (file
    // facilities on every call).
    "system.file.fileExists",
    "system.file.getTempFile",
    "system.file.openFile",
    "system.file.openFiles",
    "system.file.readFileAsBytes",
    "system.file.readFileAsString",
    "system.file.saveFile",
    "system.file.saveToFile",
    "system.file.saveToFileNonBlocking",
    "system.file.writeFile",
    // `system.groups`: removes transaction groups.
    "system.groups.removeGroups",
    // `system.gui`: Vision dialogs, colors and component geometry; window
    // names.
    "system.gui.chooseColor",
    "system.gui.closeDesktop",
    "system.gui.closeParentWindow",
    "system.gui.closeWindow",
    "system.gui.color",
    "system.gui.confirm",
    "system.gui.convertPointToScreen",
    "system.gui.createPopupContext",
    "system.gui.createPopupMenu",
    "system.gui.errorBox",
    "system.gui.getCurrentDesktop",
    "system.gui.getDesktopHandles",
    "system.gui.getOpenedWindowNames",
    "system.gui.getQuality",
    "system.gui.getScreenIndex",
    "system.gui.getScreens",
    "system.gui.getWindowNames",
    "system.gui.inputBox",
    "system.gui.isTouchscreenMode",
    "system.gui.isTouchscreenModeEnabled",
    "system.gui.messageBox",
    "system.gui.moveComponent",
    "system.gui.openDiagnostics",
    "system.gui.passwordBox",
    "system.gui.reshapeComponent",
    "system.gui.resizeComponent",
    "system.gui.setScreenIndex",
    "system.gui.setTouchscreenMode",
    "system.gui.setTouchscreenModeEnabled",
    "system.gui.showNumericKeypad",
    "system.gui.showTouchscreenKeyboard",
    "system.gui.transform",
    "system.gui.warningBox",
    // `system.historian`: history points, annotations and metadata the
    // historian stored.
    "system.historian.applyAnnotationChanges",
    "system.historian.applyHistoricalSourceChanges",
    "system.historian.browse",
    "system.historian.createAnnotationPoint",
    "system.historian.createDataPoint",
    "system.historian.createMetadataPoint",
    "system.historian.deleteAnnotations",
    "system.historian.getDefaultTagProviderName",
    "system.historian.getSystemName",
    "system.historian.queryAggregatedPoints",
    "system.historian.queryAnnotations",
    "system.historian.queryAnnotationsInternal",
    "system.historian.queryDataPoints",
    "system.historian.queryMetadata",
    "system.historian.queryMetadataInternal",
    "system.historian.queryRawPoints",
    "system.historian.remapHistoricalSource",
    "system.historian.storeAnnotations",
    "system.historian.storeDataPoints",
    "system.historian.storeMetadata",
    "system.historian.updateRegisteredNodePath",
    // `system.iec61850`: device operations; the local file paths are file
    // facilities.
    "system.iec61850.cancel",
    "system.iec61850.getControlParams",
    "system.iec61850.listFiles",
    "system.iec61850.operate",
    "system.iec61850.readFile",
    "system.iec61850.select",
    "system.iec61850.writeFile",
    // `system.kafka`: messages from a broker: what a script or a connector
    // published.
    "system.kafka.listConnectorInfo",
    "system.kafka.listTopicPartitions",
    "system.kafka.listTopics",
    "system.kafka.pollPartition",
    "system.kafka.pollTopic",
    "system.kafka.seekLatest",
    "system.kafka.sendRecord",
    "system.kafka.sendRecordAsync",
    // `system.math`: numbers.
    "system.math.geometricMean",
    "system.math.kurtosis",
    "system.math.max",
    "system.math.mean",
    "system.math.meanDifference",
    "system.math.median",
    "system.math.min",
    "system.math.mode",
    "system.math.normalize",
    "system.math.percentile",
    "system.math.populationVariance",
    "system.math.product",
    "system.math.skewness",
    "system.math.standardDeviation",
    "system.math.sum",
    "system.math.sumDifference",
    "system.math.sumLog",
    "system.math.sumSquares",
    "system.math.variance",
    // `system.nav`: the current window name.
    "system.nav.centerWindow",
    "system.nav.closeParentWindow",
    "system.nav.closeWindow",
    "system.nav.getCurrentWindow",
    // `system.net`: host addresses and remote server names, e-mail sending,
    // and openURL (the browser opens the URL; checked as a file facility).
    "system.net.getExternalIpAddress",
    "system.net.getHostName",
    "system.net.getIpAddress",
    "system.net.getRemoteServers",
    "system.net.openURL",
    "system.net.sendAuthEmail",
    "system.net.sendEmail",
    // `system.opc`: OPC values, browse results and server state; the
    // Ignition OPC UA server exposes tags as value nodes only (verified on
    // 8.3.3 with exposed tag providers: no property nodes, `T.eventScripts`
    // is `Bad_NodeIdUnknown`).
    "system.opc.browse",
    "system.opc.browseServer",
    "system.opc.browseSimple",
    "system.opc.getServerState",
    "system.opc.getServers",
    "system.opc.isServerEnabled",
    "system.opc.readValue",
    "system.opc.readValues",
    "system.opc.setServerEnabled",
    "system.opc.writeValue",
    "system.opc.writeValues",
    // `system.opcua`: connections and method calls.
    "system.opcua.addConnection",
    "system.opcua.callMethod",
    "system.opcua.invalidateMethodArgumentCache",
    "system.opcua.removeConnection",
    // `system.perspective`: session actions and session information (page
    // and view paths, which no fix changes).
    "system.perspective.alterDock",
    "system.perspective.alterLogging",
    "system.perspective.authenticationChallenge",
    "system.perspective.closeDock",
    "system.perspective.closePage",
    "system.perspective.closePopup",
    "system.perspective.closeSession",
    "system.perspective.debug",
    "system.perspective.download",
    "system.perspective.getSessionInfo",
    "system.perspective.isAuthorized",
    "system.perspective.login",
    "system.perspective.logout",
    "system.perspective.navigate",
    "system.perspective.navigateBack",
    "system.perspective.navigateForward",
    "system.perspective.openDock",
    "system.perspective.openPopup",
    "system.perspective.print",
    "system.perspective.refresh",
    "system.perspective.sendMessage",
    "system.perspective.setTheme",
    "system.perspective.toggleDock",
    "system.perspective.togglePopup",
    "system.perspective.vibrateDevice",
    "system.perspective.workstation.exit",
    "system.perspective.workstation.toKiosk",
    "system.perspective.workstation.toWindowed",
    // `system.print`: printer names and print jobs of components.
    "system.print.createImage",
    "system.print.createPrintJob",
    "system.print.getDefaultPrinterName",
    "system.print.getPrinterNames",
    "system.print.printToImage",
    // `system.project`: project names; requestScan runs code.
    "system.project.getProjectName",
    "system.project.getProjectNames",
    // `system.report`: report names; a report runs as a resource iglint does
    // not read (unknown code).
    "system.report.executeAndDistribute",
    "system.report.executeReport",
    "system.report.getReportNamesAsDataset",
    "system.report.getReportNamesAsList",
    // `system.roster`: roster users.
    "system.roster.addUsers",
    "system.roster.createRoster",
    "system.roster.deleteRoster",
    "system.roster.getRoster",
    "system.roster.getRosterNames",
    "system.roster.getRosters",
    "system.roster.getUsers",
    "system.roster.removeUsers",
    // `system.secrets`: secret values.
    "system.secrets.decrypt",
    "system.secrets.encrypt",
    "system.secrets.getProviders",
    "system.secrets.getSecrets",
    "system.secrets.readSecretValue",
    // `system.security`: user names, roles and screen locks.
    "system.security._tempReOpen",
    "system.security.exit",
    "system.security.getRoles",
    "system.security.getUserRoles",
    "system.security.getUsername",
    "system.security.isScreenLocked",
    "system.security.lockScreen",
    "system.security.login",
    "system.security.logout",
    "system.security.switchUser",
    "system.security.unlockScreen",
    "system.security.validateUser",
    // `system.serial`: serial port data.
    "system.serial.closeSerialPort",
    "system.serial.configureSerialPort",
    "system.serial.openSerialPort",
    "system.serial.port",
    "system.serial.readBytes",
    "system.serial.readBytesAsString",
    "system.serial.readLine",
    "system.serial.readUntil",
    "system.serial.sendBreak",
    "system.serial.write",
    "system.serial.writeBytes",
    // `system.sfc`: chart instances and variables (charts are resources
    // iglint does not read: unknown code).
    "system.sfc.cancelChart",
    "system.sfc.getRunningCharts",
    "system.sfc.getVariables",
    "system.sfc.pauseChart",
    "system.sfc.redundantCheckpoint",
    "system.sfc.resumeChart",
    "system.sfc.setVariable",
    "system.sfc.setVariables",
    "system.sfc.startChart",
    // `system.system`: gateway information, logging, messages and
    // translations (handlers are workspace scripts, analysed).
    "system.system._getSessionInfo",
    "system.system.audit",
    "system.system.beep",
    "system.system.exit",
    "system.system.getAvailableLocales",
    "system.system.getAvailableTerms",
    "system.system.getClientId",
    "system.system.getConnectTimeout",
    "system.system.getConnectionMode",
    "system.system.getEdition",
    "system.system.getGatewayAddress",
    "system.system.getGatewayStatus",
    "system.system.getInactivitySeconds",
    "system.system.getLocale",
    "system.system.getLogger",
    "system.system.getModules",
    "system.system.getProjectName",
    "system.system.getProperty",
    "system.system.getReadTimeout",
    "system.system.getSessionInfo",
    "system.system.getSystemFlags",
    "system.system.getVersion",
    "system.system.invokeAsynchronous",
    "system.system.invokeLater",
    "system.system.jsonDecode",
    "system.system.jsonEncode",
    "system.system.logger",
    "system.system.modifyTranslation",
    "system.system.parseTranslateArguments",
    "system.system.playSoundClip",
    "system.system.retarget",
    "system.system.sendMessage",
    "system.system.sendMessageInternal",
    "system.system.sendRequest",
    "system.system.sendRequestAsync",
    "system.system.setConnectTimeout",
    "system.system.setConnectionMode",
    "system.system.setLocale",
    "system.system.setLoggingLevel",
    "system.system.setReadTimeout",
    "system.system.threadDump",
    "system.system.translate",
    // `system.tag`: tag history, annotations and tag management that returns
    // qualities; `exists` (no fix creates or deletes a tag).
    "system.tag.browseHistoricalTags",
    "system.tag.deleteAnnotations",
    "system.tag.deleteTags",
    "system.tag.exists",
    "system.tag.isOverlaysEnabled",
    "system.tag.queryAnnotations",
    "system.tag.queryTagCalculations",
    "system.tag.queryTagDensity",
    "system.tag.queryTagHistory",
    "system.tag.removeManagedTag",
    "system.tag.removeTag",
    "system.tag.removeTags",
    "system.tag.rename",
    "system.tag.requestGroupExecution",
    "system.tag.restartProvider",
    "system.tag.scan",
    "system.tag.setOverlaysEnabled",
    "system.tag.storeAnnotations",
    "system.tag.storeTagHistory",
    "system.tag.updateManagedTagValue",
    "system.tag.write",
    "system.tag.writeAll",
    "system.tag.writeAllSynchronous",
    "system.tag.writeAsync",
    "system.tag.writeBlocking",
    "system.tag.writeSynchronous",
    "system.tag.writeToTag",
    "system.tag.writeToTagSynchronous",
    "system.tag.writeToTags",
    // `system.user`: users, schedules and holidays: settings values.
    "system.user.addCompositeSchedule",
    "system.user.addHoliday",
    "system.user.addRole",
    "system.user.addSchedule",
    "system.user.addUser",
    "system.user.createScheduleAdjustment",
    "system.user.editHoliday",
    "system.user.editRole",
    "system.user.editSchedule",
    "system.user.editUser",
    "system.user.getHoliday",
    "system.user.getHolidayNames",
    "system.user.getHolidays",
    "system.user.getNewUser",
    "system.user.getRoles",
    "system.user.getSchedule",
    "system.user.getScheduleNames",
    "system.user.getScheduledUsers",
    "system.user.getSchedules",
    "system.user.getUser",
    "system.user.getUserSources",
    "system.user.getUsers",
    "system.user.isUserScheduled",
    "system.user.removeHoliday",
    "system.user.removeRole",
    "system.user.removeSchedule",
    "system.user.removeUser",
    // `system.util`: gateway information, logging, messages to handlers
    // (workspace scripts, analysed), translations, globals the scripts
    // stored, thread dumps (Java frames and line numbers, which safe fixes
    // keep).
    "system.util._getSessionInfo",
    "system.util.audit",
    "system.util.beep",
    "system.util.exit",
    "system.util.getAvailableLocales",
    "system.util.getAvailableTerms",
    "system.util.getClientId",
    "system.util.getConnectTimeout",
    "system.util.getConnectionMode",
    "system.util.getEdition",
    "system.util.getGatewayAddress",
    "system.util.getGatewayStatus",
    "system.util.getGlobals",
    "system.util.getInactivitySeconds",
    "system.util.getLocale",
    "system.util.getLogger",
    "system.util.getModules",
    "system.util.getProjectName",
    "system.util.getProperty",
    "system.util.getReadTimeout",
    "system.util.getSessionInfo",
    "system.util.getSystemFlags",
    "system.util.getVersion",
    "system.util.globals",
    "system.util.invokeAsynchronous",
    "system.util.invokeLater",
    "system.util.jsonDecode",
    "system.util.jsonEncode",
    "system.util.logger",
    "system.util.modifyTranslation",
    "system.util.parseTranslateArguments",
    "system.util.playSoundClip",
    "system.util.retarget",
    "system.util.sendMessage",
    "system.util.sendMessageInternal",
    "system.util.sendRequest",
    "system.util.sendRequestAsync",
    "system.util.setConnectTimeout",
    "system.util.setConnectionMode",
    "system.util.setLocale",
    "system.util.setLoggingLevel",
    "system.util.setReadTimeout",
    "system.util.shutdown",
    "system.util.threadDump",
    "system.util.translate",
    // `system.vision`: Vision dialogs, settings, window names and component
    // images.
    "system.vision.beep",
    "system.vision.centerWindow",
    "system.vision.close",
    "system.vision.closeDesktop",
    "system.vision.closeParentWindow",
    "system.vision.closeWindow",
    "system.vision.color",
    "system.vision.createImage",
    "system.vision.createPopupMenu",
    "system.vision.createPrintJob",
    "system.vision.exit",
    "system.vision.exportCSV",
    "system.vision.exportExcel",
    "system.vision.exportHTML",
    "system.vision.getAvailableLocales",
    "system.vision.getAvailableTerms",
    "system.vision.getClientId",
    "system.vision.getConnectTimeout",
    "system.vision.getConnectionMode",
    "system.vision.getCurrentDesktop",
    "system.vision.getCurrentWindow",
    "system.vision.getDesktopHandles",
    "system.vision.getEdition",
    "system.vision.getExternalIpAddress",
    "system.vision.getGatewayAddress",
    "system.vision.getHandle",
    "system.vision.getInactivitySeconds",
    "system.vision.getKeyboardLayouts",
    "system.vision.getLocale",
    "system.vision.getOpenedWindowNames",
    "system.vision.getReadTimeout",
    "system.vision.getRoles",
    "system.vision.getScreenIndex",
    "system.vision.getScreens",
    "system.vision.getSystemFlags",
    "system.vision.getUsername",
    "system.vision.getWindowNames",
    "system.vision.invokeLater",
    "system.vision.isOverlaysEnabled",
    "system.vision.isScreenLocked",
    "system.vision.isTouchScreenMode",
    "system.vision.lockScreen",
    "system.vision.logout",
    "system.vision.openFile",
    "system.vision.openFiles",
    "system.vision.openURL",
    "system.vision.playSoundClip",
    "system.vision.printToImage",
    "system.vision.refreshBinding",
    "system.vision.retarget",
    "system.vision.saveFile",
    "system.vision.setConnectTimeout",
    "system.vision.setConnectionMode",
    "system.vision.setLocale",
    "system.vision.setOverlaysEnabled",
    "system.vision.setReadTimeout",
    "system.vision.setScreenIndex",
    "system.vision.setTouchScreenMode",
    "system.vision.showColorInput",
    "system.vision.showConfirm",
    "system.vision.showDiagnostics",
    "system.vision.showError",
    "system.vision.showInput",
    "system.vision.showMessage",
    "system.vision.showNumericKeypad",
    "system.vision.showPasswordInput",
    "system.vision.showTouchscreenKeyboard",
    "system.vision.showWarning",
    "system.vision.switchUser",
    "system.vision.transform",
    "system.vision.unlockScreen",
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
    /// A `java.net.URL`, which opens connections: fine only from an outside
    /// `file:` literal. A network URL may fetch workspace content from a
    /// gateway ([`Output::Network`]).
    Url,
    /// A `java.net.URI`, which reads nothing by itself (`toURL` is a
    /// measured file member): fine from an `http`, `https`, `ftp` or
    /// outside `file:` literal.
    Uri,
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
    ("java.net.URI", JavaPolicy::Uri),
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

/// Whether the literal URL `text` names a local file outside the
/// workspace: a `file:` URL that [`url_outside_workspace`] accepts. Any
/// network URL counts as a possible read ([`Output::Network`]).
pub fn file_url_outside_workspace(text: &str) -> bool {
    text.trim().to_ascii_lowercase().starts_with("file:") && url_outside_workspace(text)
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
