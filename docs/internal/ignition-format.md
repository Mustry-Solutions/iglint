# Ignition 8.3 file format, as iglint models it

Status: verified against Ignition 8.3.3 (`inductiveautomation/ignition:8.3.3`).
Audience: iglint contributors. This is the reference for `iglint_project`
(discovery and resource model), for the script wrappers in DESIGN.md and for
rule design.

How facts were established, in order of authority:

* **[jar]**: read from the 8.3.3 gateway, designer and module jars, or computed
  by calling Ignition's own classes (for example `ExtensionFunctionDescriptor.buildMethod`,
  `TagScriptManager.buildSignature`, `ProjectFileUtil.exportToZip`) under the
  bundled JRE and Jython. These are exact.
* **[gw]**: observed on a running throwaway 8.3.3 gateway: we placed resources
  on disk, restarted, and logged what the scripts saw.
* **[schema]**: the JSON schemas and component descriptors shipped inside the
  Perspective module (`perspective-common.jar`: `schemas/*.json`,
  `ia.components.json`, `descriptors/*.json`; `perspective-designer.jar`:
  `dom_events.json`).
* **[corpus]**: structure seen in a private production corpus (about 300
  Perspective views, 220 script modules, 1,300 tag definition files). Only
  aggregate observations are used.
* **[docs]**: public documentation, not independently verified. Marked as such.

All examples are invented. The fixtures in
`crates/iglint_project/tests/fixtures/basic/` contain every structure described
here, formatted the way Ignition writes it. Everything in them that can be
loaded without a Designer session was loaded on a real 8.3.3 gateway: the
projects started, the gateway event scripts ran, and the tags and UDTs were read back.

---

## 1. Resource collections and the repository layout

Ignition 8.3 stores almost everything as **resources** on disk. A resource is
a directory with a manifest (`resource.json`, or `unary-resource.json` for
tag folders) and zero or more data files. Resources live in **resource
collections**. There are two kinds of collection:

| Kind | Gateway path | Manifest |
|---|---|---|
| Project | `data/projects/<project>/` | `project.json` |
| Gateway config collection ("config mode") | `data/config/resources/<collection>/` | `config-mode.json` |

Typical repository layout (for example, a Docker Compose setup that bind-mounts
both trees):

```text
projects/<project>/...                       -> data/projects/<project>/
services/<gateway>/config/resources/<coll>/  -> data/config/resources/<coll>/
```

Inside either kind of collection, the first path segment is the **module id**
that owns the resource type and the second is the **type id**:
`<moduleId>/<typeId>/<resource path...>/`. The core platform uses module id
`ignition`.

### 1.1 Resource paths

* The resource path is the directory path below `<moduleId>/<typeId>/`, with
  `/` separators. Path components are case-sensitive.
* **Singleton** resources live directly in the type directory (for example
  `ignition/startup/resource.json`). The resource path is empty.
* **Folders** are plain directories without a manifest (projects) or with a
  `unary-resource.json` (tag definitions). A directory that has a manifest
  and also contains child resource directories is both a resource and a folder
  (for example a script package that has its own `code.py`; see 4.2).
* A directory without a manifest is only a folder: the gateway does not
  load data files in it, even a `view.json` below `views/` or a `code.py`
  below `ignition/script-python/` (the resource does not exist, references
  to it fail). Files outside `<moduleId>/<typeId>/` directories, and files
  of a resource that its type does not read (a `helper.py` next to a
  library `code.py`), are ignored the same way. iglint lints such files
  anyway (a manifest-less directory as the resource its main file implies,
  other files standalone), keeps them out of inheritance and reference
  resolution, and reports them with `REF009`.
* Hidden `.resources/` directories at collection roots are the gateway's
  content-addressed cache. Ignore them.

### 1.2 `resource.json`

Written by Gson with 2-space indentation, keys in this order, **no trailing
newline** [jar][corpus]:

```json
{
  "scope": "G",
  "description": "",
  "documentation": "",
  "version": 1,
  "restricted": false,
  "overridable": true,
  "files": [
    "view.json",
    "thumbnail.png"
  ],
  "attributes": {
    "lastModification": {
      "actor": "designer",
      "timestamp": "2026-03-04T14:01:00Z"
    },
    "lastModificationSignature": "<64 hex chars>"
  }
}
```

| Key | Meaning |
|---|---|
| `scope` | Application scope code: `G` gateway (1), `D` designer (2), `C` client (4), combinations such as `DG`, `CG`, `CD`, `A` all (7), empty or `N` none [jar `ApplicationScope.toCode/parseScope`]. Perspective resources and gateway event scripts are `G`; script modules and named queries are `A`. |
| `description` | Optional. Present on gateway config resources. |
| `documentation` | Optional free text (named queries store their description here). |
| `version` | Resource format version. `1` for almost everything; named queries are `2` [jar]. |
| `restricted` | Protected resource. |
| `overridable` | `false` forbids a child project (or child config collection) from overriding this resource. |
| `files` | Data files that belong to the resource, in the order the writer chose. A file that exists on disk but is not listed is **ignored** by the gateway [gw: tag `tags.json` not listed in `unary-resource.json` was not loaded]. A listed file that is missing is an error. |
| `attributes` | Type-specific JSON object. Common keys: `lastModification` (`actor`, ISO-8601 UTC `timestamp`), `lastModificationSignature` (SHA-256 hex written by the Designer/gateway), `uuid` and `enabled` (gateway config resources), plus the per-type keys documented below. |

Manifests written by hand (or by tools) often lack `lastModification*`; the
gateway accepts that.

### 1.3 `project.json`

```json
{
  "title": "Fixture HMI",
  "description": "Operator screens",
  "parent": "fixture-base",
  "enabled": true,
  "inheritable": false
}
```

Key order differs between writers (the export path writes `title, description,
enabled, inheritable, parent` [jar]); treat it as unordered. `parent` is the
parent project name or `""`. `inheritable: true` marks a project that other
projects may inherit from.

### 1.4 Project inheritance [gw]

* A project sees the union of its own resources and those of its ancestors.
* Resources are matched by `(moduleId, typeId, resource path)`. A child
  resource **replaces** the parent's resource of the same path entirely. No
  merge happens inside a resource: overriding `core/format` replaces the whole
  module. Functions that exist only in the parent's version disappear.
* `overridable: false` on the parent resource forbids the override.
* Folders merge: a child can add `hmi/motors` next to the inherited `core/*`.
* Gateway event scripts run per **runnable** (non-inheritable) project, using
  the effective (merged) resources. The fixtures show a child that inherits
  views, style classes, named queries and script modules from its parent.

### 1.5 Gateway config collections [corpus][gw]

Each directory below `config/resources/` is a collection with a
`config-mode.json` that has the same shape as `project.json`. They form an
inheritance chain:

```text
system  <-  external  <-  core  <-  <deployment mode, e.g. production>  <-  local
```

* `core`: shared configuration (tag providers, tags, databases, ...).
* Deployment-mode collections (user-named, for example `development`,
  `production`) override `core` resources by path. Only the active mode applies.
* `local`: instance-specific (not synchronised to a redundant peer), not
  inheritable.
* `external`: externally managed.

A resource in a mode collection with the same path as one in `core` replaces
it (the fixtures override `database-connection/Production`).

Gateway config resources usually have one data file `config.json` and
attributes `uuid`, `enabled`, `lastModification*`. Selected types:

| Path | Content |
|---|---|
| `ignition/tag-provider/<provider>/config.json` | `{"profile": {"type": "STANDARD" or "MANAGED" or "REMOTE", "allowBackfill", "enableTagReferenceStore"}, "settings": {...}}` |
| `ignition/tag-group/<provider>/<group>/config.json` | `{"config": {"mode": "Direct", "rate": 1000, "opcDataMode": "Subscribed", ...}}` |
| `ignition/tag-definition/<provider>/...` | Tags, see section 9. |
| `ignition/tag-type-definition/<provider>/...` | UDT definitions, see section 9. |
| `ignition/database-connection/<name>/config.json` | `connectURL`, `driver`, `translator`, pool settings, `validationQuery`, `password` (secret: `{"type": "Embedded", "data": {...ciphertext...}}` or `{"type": "Referenced", "data": {"providerName", "secretName"}}`). |
| `ignition/database-driver`, `database-translator`, `user-source`, `identity-provider`, `security-levels`, `security-zone`, `api-token`, `secret-provider`, `opc-connection`, `store-and-forward-engine`, `alarm-journal`, `email-profile`, `schedule`, `holiday`, `roster-config`, `translations`, `images`, `system-properties`, `gateway-network-*`, ... | Platform settings. iglint only needs names (for reference checks). |
| `com.inductiveautomation.opcua/device/<name>` | OPC UA driver devices. |
| `com.inductiveautomation.historian/historian-provider/<name>` | Historian providers (names used by `historyProvider`). |
| `com.inductiveautomation.perspective/themes`, `icons`, `fonts` | Gateway-wide Perspective assets (theme names, icon libraries). |
| `com.inductiveautomation.alarm-notification/alarm-notification-profile` | Notification profiles. |
| `com.inductiveautomation.eventstream/event-streams` | Event streams (8.3 module). |

The complete list of resource types registered by 8.3.3 is in Appendix A.

---

## 2. Project resource types

All paths are relative to the project directory. "Header" is the exact first
line Ignition generates for the script file or embedded script. See section 5
for how headers are applied.

| Path | Files | Attributes | Notes |
|---|---|---|---|
| `ignition/script-python/<pkg>/<mod>/` | `code.py` | `hintScope` (int) | Project library. Section 4. |
| `ignition/named-query/<folder>/<name>/` | `query.sql` | query settings | Section 8. Manifest `version: 2`. |
| `ignition/startup/` | `onStartup.py` | `enabled` | Gateway startup. Header `def onStartup():` |
| `ignition/shutdown/` | `onShutdown.py` | `enabled` | Header `def onShutdown():` |
| `ignition/update/` | `onUpdate.py` | `enabled` | Project-update event. Header `def onUpdate(actor, resources):` |
| `ignition/timer/<name>/` | `handleTimerEvent.py` | `delay` (ms), `fixedDelay` (bool: fixed delay vs fixed rate), `sharedThread` (bool), `enabled` | Header `def handleTimerEvent():` |
| `ignition/scheduled/<name>/` | `handleScheduleEvent.py` | `cronExpression`, `enabled` | Header `def handleScheduleEvent():` |
| `ignition/tag-change/<name>/` | `onTagChange.py` | `paths` (list of tag paths), `changeTypes` (subset of `ValueChange`, `QualityChange`, `TimestampChange`), `enabled` | Header `def onTagChange(initialChange, newValue, previousValue, event, executionCount):` |
| `ignition/message/<name>/` | `handleMessage.py` | `threadType` (`Shared` or `Dedicated`; clients also `EDT`), `enabled`, optional `permission` (zone/role list) | Gateway message handler. Header `def handleMessage(payload):` |
| `ignition/global-props/` | `data.bin` | | Project properties (default database, default tag provider, ...). Singleton. |
| `ignition/designer-properties/` | `data.bin` | | Designer settings. |
| `ignition/script-app-library/` | | | Deprecated since 8.3.0 (legacy `app.` scripts). |
| `com.inductiveautomation.perspective/views/<path>/` | `view.json`, optional `thumbnail.png` | `title` (optional), `lastModification*` | Section 6. |
| `com.inductiveautomation.perspective/style-classes/<path>/` | `style.json` (legacy: `data.bin`) | | Section 7.1. |
| `com.inductiveautomation.perspective/stylesheet/` | `stylesheet.css` | | Project CSS, singleton. |
| `com.inductiveautomation.perspective/page-config/` | `config.json` | | Section 7.2. |
| `com.inductiveautomation.perspective/session-props/` | `props.json` | | Section 7.3. |
| `com.inductiveautomation.perspective/general-properties/` | `data.bin` (JSON text) | | Update mode, locale, timezone, idp, timeouts. |
| `com.inductiveautomation.perspective/inactivity-properties`, `session-permissions`, `offline-mode-settings`, `tag-drop-settings`, `symbol-state-settings` | | | Singletons, settings only. |
| `com.inductiveautomation.perspective/startup/` | `onStartup.py` | `enabled` | Session startup. Header `def onStartup(session):` |
| `com.inductiveautomation.perspective/shutdown/` | `onShutdown.py` | `enabled` | Header `def onShutdown(session):` |
| `com.inductiveautomation.perspective/page-startup/` | `onPageStartup.py` | `enabled` | Header `def onPageStartup(page):` |
| `com.inductiveautomation.perspective/message/<name>/` | `handleMessage.py` | `threadType`, `enabled` | Session message handler. Header `def handleMessage(session, payload):` |
| `com.inductiveautomation.perspective/key-event/<name>/` | `onKeyEvent.py` | `eventMode` (`KEYDOWN`/`KEYUP`), `eventBound` (`KEY`/`CODE`), `eventBoundKey`, `eventBoundCode`, `regexPattern`, `regexWindow`, `alt/control/shift/metaEventModifier`, `capturePhaseEventOption`, `preventDefaultEventOption`, `stopPropagationEventOption`, `enabled` | Header `def onKeyEvent(page, event):` |
| `com.inductiveautomation.perspective/barcode/` | `onBarcodeDataReceived.py` | `enabled` | Header `def onBarcodeDataReceived(session, data, context):` |
| `com.inductiveautomation.perspective/bluetooth/` | `onBluetoothReceived.py` | `enabled` | Header `def onBluetoothReceived(session, data):` |
| `com.inductiveautomation.perspective/nfc-scan/` | `onNdefDataReceived.py` | `enabled` | Header `def onNdefDataReceived(session, data, context):` |
| `com.inductiveautomation.perspective/accelerometer/` | `onAccelerometerDataReceived.py` | `enabled` | Header `def onAccelerometerDataReceived(session, data, context):` |
| `com.inductiveautomation.perspective/auth-challenge/` | `onAuthChallengeCompleted.py` | `enabled` | Header `def onAuthChallengeCompleted(session, payload, result):` |
| `com.inductiveautomation.perspective/form-submission-handler/<name>/` | `handleSubmission.py` | `enabled` | Header `def handleSubmission(session, name, data, formContext, sessionContext, retry):` |
| `com.inductiveautomation.perspective/session-scripts/`, `gateway-scripts/` | `data.bin` | | Pre-8.3 aggregate formats, migrated into the per-event resources above. Recognise, do not lint. |
| `com.inductiveautomation.webdev/resources/<path>/` | `config.json` + method files | | Section 10. |
| `com.inductiveautomation.reporting/reports/<path>/` | report document and data config | | Identify only. Script data source header `def updateData(data, sample):` [jar]. |
| `com.inductiveautomation.alarm-notification/alarm-pipelines/<name>/` | pipeline definition | | Identify only. Script block function `handleAlarm` with an `event` parameter [jar, exact header not computed]; calculated roster function `calculateRoster` [jar]. |
| `com.inductiveautomation.sfc/charts/<path>/` | `sfc.xml` | | Identify only. XML with chart scripts (`onStartScript`, `onStopScript`, `onCancelScript`, `onAbortScript`, step scripts) [jar]. |
| `com.inductiveautomation.vision/windows/<path>/` | `window.bin` or `window.xml`, `thumbnail.png`, optional `permissions.json` | | Vision window. Identify only. |
| `com.inductiveautomation.vision/templates/<path>/` | `template.bin`/`template.xml`, `droptarget.*` | | Vision template. Identify only. |
| `com.inductiveautomation.vision/client-event-scripts`, `startup`, `shutdown`, `timer`, `tag-change`, `message`, `keystroke`, `menubar`, `shutdown-intercept`, `client-tags`, `*-properties`, `custom-palettes` | | | Vision client resources. Client event headers follow the gateway ones (`def onStartup():`, `def handleTimerEvent():`, `def handleMessage(payload):`, `def onTagChange(initialChange, newValue, previousValue, event, executionCount):`), plus `def onKeystroke():` and `def isShutdownPrevented(event):` [jar]. |
| `com.inductiveautomation.sqlbridge/transaction-groups/<path>/` | | | Identify only. |

Perspective session events and form handlers run **on the gateway** (scope
`G`) even though they are tied to a session.

---

## 3. Script scopes and implicit names

### 3.1 Where each script runs

| Script | Runs in | Python scope |
|---|---|---|
| Project library modules (`code.py`) | every scope that imports them | module |
| Gateway events (startup, shutdown, update, timer, scheduled, tag change, message) | gateway | function |
| Perspective component/view event scripts, transforms, change scripts, message handlers, custom methods, extension functions | gateway (Perspective runs all Python on the gateway, in the session) | function |
| Perspective session events, page startup, key events, form handlers | gateway | function |
| Tag event scripts | gateway (tag provider) | function |
| WebDev handlers | gateway | function |
| Alarm pipeline scripts, report scripts, SFC scripts | gateway | function |
| Vision client scripts | Vision client (Java client JVM) | function |

### 3.2 Names available without import [gw]

Observed in gateway event scripts on 8.3.3 (`globals()` of the generated
function):

```text
__builtins__, system, fpmi, sys, <the event function itself>,
<every top-level script-python package or module of the project>
```

* `system` is the scripting API. `fpmi` is a legacy alias of `system`.
* Every **top-level** name under `ignition/script-python/` (for example `core`,
  `hmi`) is injected as a global. Nested names are reached through attribute
  access (`core.format.withUnit`) or `import core.format`.
* `project` and `shared` are **not** defined in 8.3 (they were the 7.x
  prefixes). `ModuleLibrary` still carries the constants `project` and
  `shared` as `LEGACY_*_MODULE`, but neither name resolves at runtime: `project.alpha` and `shared.alpha`
  raise `NameError` [gw].
* `app` is not defined (`script-app-library` is deprecated).
* Library modules see the same globals: a module can call another top-level
  package without importing it [gw].
* Parameters of the generated function are locals (see section 5).
* Library modules have `__name__` but no `__file__`; see section 5.4 for
  the exact globals of modules and event scripts.

Inside Perspective component scripts, `self` is the component (the view for
view-level events); `event`, `value`, `payload`, etc. are the header
parameters. Session and page events have no `self`; they receive `session`
or `page` as parameters.

---

## 4. Project script library (`ignition/script-python`)

### 4.1 Layout

```text
ignition/script-python/
  core/                 <- folder (no manifest): package "core"
    format/
      code.py           <- module core.format
      resource.json
    tags/
      code.py
      resource.json
```

`resource.json` for a module:

```json
{
  "scope": "A",
  "version": 1,
  "restricted": false,
  "overridable": true,
  "files": [
    "code.py"
  ],
  "attributes": {
    "hintScope": 2
  }
}
```

`hintScope` only controls Designer code hints: `0` None, `1` Gateway, `2`
Designer (default), `7` All [jar `ModuleLibrary$ScriptHintScope`]. It does
**not** restrict where the module runs.

Name rules: every path component must match `[_a-zA-Z]\w*`
(`ModuleLibrary.NAME_PATTERN`) [jar]. An empty module's `code.py` is a single
`\n` (`EMPTY_SCRIPT_RESOURCE`).

### 4.2 Mapping to Python names [gw]

* Directory path = dotted module name: `core/format/code.py` is `core.format`.
* A folder **without** `code.py` becomes a package object (`<app package core>`)
  whose attributes are its children. `import core.format` and `core.format.f()`
  both work.
* A directory **with** its own `code.py` becomes a module. **Its children are
  not reachable**: with `legacy/code.py` and `legacy/helpers/code.py`,
  `legacy.helpers` raises `AttributeError` and `import legacy.helpers` raises
  `ImportError: No module named helpers` [gw]. This is a real-world trap and a
  lint rule.
* Modules are loaded lazily and cached per project in `sys.modules`
  (`alpha`, `gamma`, `gamma.delta` observed).
* A child project's module of the same path replaces the parent's (section 1.4).
* Third-party pure-Python packages may also live in `user-lib/pylib` on the
  gateway; they are importable but not project resources.

---

## 5. Script files and generated headers

### 5.1 Files that contain a full `def` (gateway events, session events, WebDev)

8.3 writes the complete function: header line, then the body indented with
tabs, **no trailing newline** [jar `buildMethod` output]:

```python
def handleTimerEvent():
	system.util.getLogger("demo").info("tick")
```

On load Ignition calls `ExtensionFunctionDescriptor.extractUserScript(file,
name)` [jar]:

* If the first line starts with exactly `def <name>(`, **the whole first line
  is dropped**, whatever it contains. The rest is the body.
* Otherwise the whole file is treated as the body (legacy, headerless style).

At run time the body is re-wrapped with the **canonical** header from the
descriptor and compiled (`buildMethod` = canonical header + `\n` + body, no
re-indentation) [jar]. Consequences, all verified on the gateway:

* Parameters written in the file are cosmetic. `def handleMessage(payload=None,
  extra=5):` runs as `def handleMessage(payload):` and `extra` is undefined;
  `def handleTimerEvent(bogus=1):` loses `bogus` [gw].
* A one-line function `def handleTimerEvent(): doWork()` loses its body,
  because the whole first line is dropped.
* A multi-line header (parameters wrapped onto the next line) leaves
  `  a):` in the body, which is a syntax error.
* A header-less legacy file fails at run time (observed `NameError` in a
  message handler written as top-level code) [gw].
* Extra top-level code after the body (helper `def`s at column 0) is compiled
  as module code around the function.

iglint therefore parses the file **as written** (that is what the user edits),
and additionally checks that line 1 is exactly the canonical header (rule
material).

### 5.2 Canonical headers [jar]

Gateway project events (`ignition/...`):

```python
def onStartup():
def onShutdown():
def onUpdate(actor, resources):
def handleTimerEvent():
def handleScheduleEvent():
def onTagChange(initialChange, newValue, previousValue, event, executionCount):
def handleMessage(payload):
```

Perspective session and gateway events (`com.inductiveautomation.perspective/...`):

```python
def onStartup(session):
def onShutdown(session):
def onPageStartup(page):
def handleMessage(session, payload):
def onKeyEvent(page, event):
def onBarcodeDataReceived(session, data, context):
def onBluetoothReceived(session, data):
def onNdefDataReceived(session, data, context):
def onAccelerometerDataReceived(session, data, context):
def onAuthChallengeCompleted(session, payload, result):
def handleSubmission(session, name, data, formContext, sessionContext, retry):
```

Vision client events: `def onStartup():`, `def onShutdown():`,
`def handleTimerEvent():`, `def handleMessage(payload):`,
`def onTagChange(initialChange, newValue, previousValue, event, executionCount):`,
`def onKeystroke():`, `def isShutdownPrevented(event):`.

WebDev (`do<Method>.py`): `def doGet(request, session):` and the same for
`doPost`, `doPut`, `doDelete`, `doHead`, `doOptions`, `doTrace`, `doPatch`
[jar `PythonResource`, `buildDef` with params `request, session`].

Event Streams (module 8.3): `def onEventsReceived(events, state):`,
`def onFilter(event, state):`, `def onTransform(event, state):`,
`def onError(event, state):` [jar].

Reporting script data source: `def updateData(data, sample):` [jar].

### 5.3 Embedded scripts (Perspective, tags): body only

Perspective and tag event scripts store **only the body** in a JSON string,
indented with one tab per level (`"\tif x:\n\t\ty()"`). Ignition compiles them
inside these headers:

| Location | Header | Source |
|---|---|---|
| Event script action (`events.*.*` with `type: "script"`) | `def runAction(self, event):` | [jar `ScriptAction`] |
| Script transform | `def transform(self, value, quality, timestamp):` | [jar `ScriptTransform`] |
| Property change script (`propConfig.<path>.onChange`) | `def valueChanged(self, previousValue, currentValue, origin, missedEvents):` | [jar `PropertyChangeScript`] |
| Component message handler | `def onMessageReceived(self, payload):` | [jar `MessageHandlerImpl`] |
| Custom method `{name, params: [a, b]}` | `def name(self,a,b):`; with no params `def name(self,):` | [jar `CustomMethodConfig.getSignature` + `"def %s:"`] |
| Component extension function | `def <name>(self, <arguments>):` from the component descriptor, for example `def filterAlarm(self, alarmEvent):` | [jar + `ia.components.json`] |
| Tag event `valueChanged` | `def valueChanged(tag,tagPath,previousValue,currentValue,initialChange,missedEvents):` | [jar `TagScriptManager.buildSignature`] |
| Tag event `qualityChanged` | `def qualityChanged(tag,tagPath,previousValue,currentValue,initialChange,missedEvents):` | [jar] |
| Tag event `qualifiedValueChanged` | `def qualifiedValueChanged(tag,tagPath,previousValue,currentValue,initialChange,changed,missedEvents):` | [jar] |
| Tag event `alarmActive` | `def alarmActive(tag,tagPath,alarmName,alarmEvent,alarmPath,missedEvents):` | [jar] |
| Tag event `alarmCleared` | `def alarmCleared(tag,tagPath,alarmName,alarmEvent,alarmPath,missedEvents):` | [jar] |
| Tag event `alarmAcked` | `def alarmAcked(tag,tagPath,alarmName,alarmEvent,alarmPath,ackedBy,missedEvents):` | [jar] |

Tag headers have no spaces after commas and end with `\n`; the body follows
directly. Custom method headers have no space after `self,` and no space
between parameters. The exact header text matters only for byte-accurate
column mapping of the first body line (it does not), so iglint may use these
strings verbatim.

`origin` values seen in change scripts: `Binding`, `BindingWriteback`,
`Script`, `Delegate`, `Browser`, `Project` [docs].

### 5.4 How the source is compiled [jar][gw]

Perspective builds `"def " + signature + ":\n" + body`
(`ScriptFunctionHelper`) and calls `ScriptManager.compileFunction(String)`,
which calls `Py.compile_flags(source, filename, mode,
CompilerFlags.getCompilerFlags().combine(0x100))`; `0x100` is
`PyCF_SOURCE_IS_UTF8` [jar]. Perspective's compiled-script cache uses the
same call. Library modules and gateway event scripts are compiled from Java
strings too. Consequences, each observed on an 8.3.3 gateway in a library
module, a gateway message handler and a `compileFunction` call [gw]:

* **Encoding declarations.** A PEP 263 declaration (`# -*- coding: utf-8
  -*-`, `# coding=latin-1`, any charset name) on line 1 or 2 of the
  **compiled** source fails with `SyntaxError: encoding declaration in
  Unicode string` (reported at line 0); line 3 or later is an ordinary
  comment. In Perspective scripts and event files line 1 is the generated
  header, so the first body line is line 2 (an error) and the second body
  line is line 3 (fine). A file whose line 1 is a coding comment followed by
  the `def` loses its header (section 5.1) and the comment lands on line 2:
  same error. iglint parses every script with
  `SourceKind::Unicode` (`iglint_python`), which matches.
* **Byte order mark.** A UTF-8 BOM is **not** skipped: a library module
  starting with one fails with `SyntaxError: no viable alternative at
  character '\ufeff'` (line 1); BOM plus coding declaration gives the
  encoding error. In an event file the BOM hides the `def` header, so the
  whole file becomes the body and the BOM sits on line 2 (`no viable
  alternative at character`). `iglint_python` skips a leading BOM (as
  CPython and Jython do for files read from disk); `SCR001` reports it
  separately for scripts whose source starts with one. Plain `compile()` of
  a byte string accepts BOMs and declarations; only the unicode path
  Ignition uses rejects them.
* **Invalid UTF-8.** Resource bytes are decoded as UTF-8 with malformed
  input replaced, never rejected: `ModuleLibrary.deserializeScript` turns
  a library `code.py` containing the Latin-1 byte `0xE9` into
  `u'caf\ufffd'` (one U+FFFD per maximal invalid sequence, as Java's
  decoders do), and the module compiles and runs with the replacement
  character in its strings [jar]. A coding declaration does not help (see
  above). iglint reads files the same way (offsets kept, see
  `iglint_text::decode_utf8`) and reports the sequences with `VIEW007`.
* **No compiler flags.** `print` is a statement, `1/2 == 0`, `'x'` is a
  `str` (code object `co_flags` without future flags) in library modules,
  event scripts and WebDev handlers alike.
* **`from __future__`.** Works as the first statement of a library module
  (after a docstring too). Anywhere else, including any Perspective or event
  script (the body of a generated function), it fails with `from __future__
  imports must occur at the beginning of the file`.
* **Globals of a library module** at import time: `__builtins__`,
  `__name__`, `fpmi`, `sys`, `system` and every top-level library package
  (including the module's own). `__name__` is the last component of the
  module's path (`c` for `a/b/c`, measured with Ignition's own
  `ScriptManager.addProjectScriptModules` [oracle]). There is no
  `__file__`; `__doc__` exists only when the module has a docstring
  (otherwise the builtin `__doc__` is found); `__package__` appears once an
  `import` statement has run (see "Names that statements create" below).
* **Globals of an event script** (message handler, WebDev handler):
  `__builtins__`, `fpmi`, `sys`, `system`, the function's own name and the
  top-level library packages; no `__name__`, `__file__` or `__doc__`
  (`__name__` and `__doc__` still resolve to the builtins' values).

Jython runtime facts that rules rely on (checked with IA's Jython 2.7.3):

* Java exceptions (`java.lang.NullPointerException`, SQL exceptions from
  `system.db.*`) are caught by a bare `except:` and by `except Throwable`,
  **not** by `except Exception` or `except BaseException`.
* `raise "text"` raises `TypeError: exceptions must be old-style classes or
  derived from BaseException, not str`.
* `%` formatting: a dict on the right satisfies any number of positional
  placeholders (`"%s %s" % {"a": 1}` formats the dict twice), `"abc" % []`
  raises `not all arguments converted` (CPython returns `"abc"`),
  `%(key)s` with a tuple raises `format requires a mapping`.
* `str.format`: auto numbering (`{}`) is supported; mixing it with manual
  numbering raises `ValueError`; `{-1}` crashes with a Java
  `ArrayIndexOutOfBoundsException`.
* The compiler rejects some code its parser accepts (`return` outside a
  function, `continue` in a loop's `else`, duplicate parameters, `return`
  with a value in a generator, `import *` and unqualified `exec` in
  functions with nested free variables, `del` of a closure variable,
  assignments to `__debug__`, misplaced `from __future__`), and crashes on
  some assignment targets its parser accepts (`-a = 1`, `for f() in x`):
  see `crates/iglint_python_semantic/tests/oracle/compile.cases`, recorded
  with `oracle/run-oracle.sh --compile`.

#### Names that statements create [oracle]

Measured with `oracle/run-oracle.sh --implicit`
(`crates/iglint_python_semantic/tests/oracle/implicit_names.cases` and
`.expected`, checked by `tests/implicit_names_oracle.rs`): each source runs
as a project library module loaded by Ignition's `ScriptManager`, as the
body of an event function compiled with `ScriptManager.compileFunction`,
and as top-level code after such a function, and the namespace is read
afterwards.

| Statement | Library module | Event script (body or top-level code) |
|---|---|---|
| `import a`, `import a.b`, `import a as b`, `from a import b [as c]`, `from a import *`, `from __future__ import x` (module, function or class body, once it runs) | binds `__package__ = None` in the module's globals, unless it is already set to something else | nothing (`from __future__` does not compile there) |
| the same, failing (`ImportError` caught), Java and project library imports included | binds `__package__` too (it is computed before the lookup) | nothing |
| `from . import x`, `from .m import x`, `from .. import x` | `ValueError` (`Attempted relative import in non-package`), binds nothing | `ImportError`; may leave `__warningregistry__` (an `ImportWarning` about `python.path` directories) |
| an import in a function that never runs | nothing | nothing |
| `exec "import m"` (unqualified) | binds `__package__` | nothing |
| `exec "import m" in d`, `__import__('m')` | nothing (`d` gets only `__builtins__`) | nothing |
| `__import__('m', globals())` | binds `__package__` | nothing |
| any import after the code set `__name__` to a dotted name or `__package__` to a string | `__package__` is the parent name (or stays), and a `RuntimeWarning` (`Parent module ... not found`) leaves `__warningregistry__` | the same, for top-level code |
| a module docstring | `__doc__` | nothing |
| every other statement (comprehensions, generator expressions, `for`, `while`, `try`, `with`, `class`, `def`, `lambda`, `yield`, `assert`, `del`, `print`) | nothing | nothing |

Event scripts run without `__name__`, and Jython's import stores
`__package__` only in globals that have one, so their imports bind nothing
unless the code binds `__name__` itself. The semantic model binds
`__package__` for every absolute import of a library module
(`BindingKind::ImportPackage`), and for relative ones too when the code
binds `__name__`. A plain Jython module imported from a file (not how
Ignition loads project code) has `__file__`, `__doc__` and `__package__`
from the start.

#### Size limits [oracle]

Measured with `oracle/run-oracle.sh --compile` (full `compile()` of a
unicode source, the call above), recorded in
`crates/iglint_python_semantic/tests/limits_oracle.rs`. All of them fail
in `compile()`, so the script never loads; the errors are Java exceptions,
not `SyntaxError`s. They are the same with the default 1 MiB thread stack
and with `-Xss16m`.

* **First two lines: 100,000 characters.** While it looks for an
  encoding declaration, Jython reads the first two lines of the source
  (with `"\n\n"` appended) through a `BufferedReader` marked with a
  read-ahead limit of 100,000 (`MARK_LIMIT`, checked by reflection) and
  then resets it. Counted in UTF-16 code units (an astral character counts
  2), the source fails with `java.io.IOException: Mark invalid` exactly
  when the terminator of line 2 starts at index 100,000 or later. `\n`,
  `\r` and `\r\n` end a line; a `\r\n` after line 1 counts 2. Examples:
  a single line of up to 99,998 characters compiles (99,999 fails); a
  99,995-character comment followed by `x=1` fails. In Perspective and
  event scripts line 1 is the generated header, so the header plus the
  first body line are limited: with `def runAction(self, event):` a first
  body line of 99,971 characters (tab included) compiles, 99,972 fails.
  Lines from 3 on have no limit: 1,000,000-character string literals
  (`str` and `unicode`, any characters and escapes), identifiers, a
  1,000,000-character comment and 100,000-digit numbers compile. There is
  no 65,535-byte limit on a string constant. `SCR002` reports this limit
  exactly.
* **Method too large.** The module body, each function body and each
  class body compile to one Java method of at most 65,535 bytes of
  bytecode (`java.lang.RuntimeException: Module or method too large in
  <file>`). Each statement updates the frame's line number (at least 6
  bytes from line 6 on, also for statements sharing a line: 11,000 `pass`
  on one line fail, 9,000 compile; `global` generates nothing). Largest
  bodies that still compile: 9,379 lines of `pass`, 4,689 of `x`, 3,282 of
  `x = 1` in a function (3,126 in a module), 2,746 distinct `xN = N`,
  1,728 empty `def`s, a 9,352-term `1+1+...`, a call with 5,965 arguments.
  The size depends on Jython's code generator, so `SCR002` reports only
  what cannot compile whatever the statements are: more than 10,922
  statement lines in one body (all bodies of 10,921 to 10,924 lines
  tested fail). Lines count, not statements, which keeps the bound below
  the real size.
* **Dead code generates nothing.** Jython's code generator stops
  compiling a suite after a statement that ends it: `return`, `raise`,
  `continue`, and an `if` with an `else` (also at the end of an `elif`
  chain) whose every branch ends that way. `break`, loops, `try` (even
  when the body and every handler return), `with`, `while 1:` and
  `assert False` do not end a suite, and `if 1: return` without `else`
  does not either (no constant folding). The cut applies to function and
  class bodies and to the suites of compound statements, also at module
  level, but not to the module's own statement list: `raise X` followed by
  10,921 lines of `pass` at module level fails, inside a function it
  compiles. Nothing after the cut counts: not its lines, not the bodies of
  `def`s in it (a dead `def g():` with 10,924 lines compiles), not its
  string constants (40,000 distinct literals after `return` compile).
  `SCR002` counts only what is generated. Errors of the code generator
  are not raised for dead code either: `'break' outside loop`,
  `'continue' not properly in loop`, `'return'` and `'yield'` outside
  function (in a class body), `default 'except:' must be last`, `can not
  delete variable ... referenced in nested scope`, `import * is not
  allowed in function ...`, and the compiler crashes on invalid targets
  (`-a = 1`). Errors of the symbol table pass still are: `'return' with
  argument inside generator`, `unqualified exec is not allowed ...`,
  duplicate arguments, `name 'x' is local and global`, `can not assign to
  __debug__` and misplaced `from __future__` imports. The cases are in
  `crates/iglint_python_semantic/tests/oracle/compile.cases`.
* **Class too large.** A module compiles to one Java class with at most
  65,535 constant pool entries (`org.python.objectweb.asm.
  ClassTooLargeException: Class too large`). A distinct string constant
  takes two entries: 32,593 distinct string literals compile, 32,594 fail.
  Distinct integers outside the 16-bit range take one (about 64,740 fit).
  `SCR002` reports more than 32,767 distinct string literals (`str` and
  `unicode` literals of the same text count once).

---

## 6. Perspective `view.json`

### 6.1 Encoding

The Designer writes `view.json` with Gson [corpus]:

* 2-space indent, **keys sorted alphabetically at every level**, no trailing
  newline.
* HTML-safe escaping: `'` `=` `<` `>` `&` are written as `\u0027` `\u003d`
  `\u003c` `\u003e` `\u0026`. Scripts and expressions are therefore full of
  escapes and the decoded-offset map in `iglint_json` is mandatory.
* Hand-edited files (or files from other tools) may use raw characters,
  other key orders and a trailing newline. Both must parse.

Nesting depth [gw]: Gson (2.8.9, and IA's 2.10.1 fork that Perspective
uses) parses arrays and objects of any depth;
`ViewConfig.fromProjectResource` with Perspective's shared Gson read views
with `custom` and `props` values nested 300,000 levels deep. Using such a
view fails, measured on 8.3.3 on threads with the default 1 MiB stack:

* components nested in `children` are built recursively: the loader
  overflows the stack (`StackOverflowError`) at about 2,100 nested
  components (about 1,200 on a cold JVM);
* a property tree (`PropertyTree`, what a view's `custom` and `props`
  become) copies values recursively: it overflows at about 1,470 nested
  objects or 3,260 nested arrays (about 490 and 1,000 on a cold JVM);
* in a browser session, a view whose `custom` held an object nested 400
  levels deep did not render (256 rendered), without a gateway log entry.

The thresholds vary with JIT state and stack contents, so there is no
exact limit. iglint parses any depth without recursion but analyses files
nested at most 8,192 levels (`iglint_project::MAX_JSON_DEPTH`), far above
every failure observed; deeper files are reported by `VIEW001`.

### 6.2 Top level

```json
{
  "custom": {},
  "events": {"system": {"onStartup": {...}, "onShutdown": {...}}},
  "params": {"line": "Line1"},
  "permissions": {"securityLevels": [...], "type": "AllOf"},
  "propConfig": {"params.line": {...}, "custom.x": {...}},
  "props": {"defaultSize": {"height": 720, "width": 1280}},
  "root": {...}
}
```

| Key | Meaning |
|---|---|
| `params` | View parameters and their default values. The direction lives in `propConfig["params.<name>"].paramDirection`. |
| `custom` | View custom properties. |
| `props` | View props: `defaultSize {width, height}`, `dropConfig {udts[], dataTypes[]}` (each `{type, param, action: "bind" or "path"}`), `loading {mode: "blocking" or "non-blocking"}`, `inputBehavior: "merge" or "replace"` [schema `view-props-schema.json`]. |
| `propConfig` | Property configuration for view-level paths (`params.*`, `custom.*`, `props.*`). |
| `events` | View events. Only the `system` group (`onStartup`, `onShutdown`) [corpus]. `self` is the view. |
| `permissions` | Optional view access permissions (security levels). |
| `root` | Root component. |

Typical key sets: `custom, params, propConfig, props, root` (most views) and
`custom, params, props, root` [corpus].

### 6.3 Components

```json
{
  "type": "ia.display.label",
  "version": 0,
  "meta": {"name": "Title", "visible": true, "tooltip": {...}, "domId": "", "tabIndex": 0, "contextMenu": {...}, "hasDelegate": true},
  "position": {...},
  "props": {...},
  "custom": {...},
  "propConfig": {...},
  "events": {...},
  "scripts": {"customMethods": [...], "messageHandlers": [...], "extensionFunctions": null},
  "children": [...]
}
```

* `type`: component type id. The 70 built-in ids are in `ia.components.json`
  (for example `ia.container.flex`, `ia.container.coord`, `ia.container.breakpt`,
  `ia.container.column`, `ia.container.split`, `ia.container.tab`,
  `ia.container.drawing`, `ia.display.label`, `ia.display.view`,
  `ia.display.flex-repeater`, `ia.display.table`, `ia.input.button`,
  `ia.shapes.svg`, ...). Additional component libraries ship their own
  `*.components.json` (`perspective-timeseries`, `perspective-map`,
  `perspective-googlemap`, `perspective-amcharts`, `pdf-viewer`, `barcode`).
  Third-party module components use their own prefixes and are unknown to us.
* `version`: optional component config version; version 1 migrated
  `meta.rotate` into `position.rotate` [jar].
* `meta` keys [schema `meta-schema.json`]: `name` (required, unique among
  siblings), `visible`, `tooltip`, `contextMenu`, `domId`, `tabIndex`; plus
  `hasDelegate` written by the Designer [corpus].
* `props`: component properties; schema per type in `ia.components.json`.
  `props.style` is a style object; `props.style.classes` is a
  space-separated list of style class paths (section 7.1).
* `custom`: component custom properties.
* `children`: only on containers.

### 6.4 `position` by parent container

The child's `position` depends on the **parent** type
(`childPositionSchema` in `ia.components.json`):

| Parent | Child `position` keys |
|---|---|
| `ia.container.coord` | `x`, `y`, `width`, `height`, `rotate {anchor, angle}` |
| `ia.container.flex` | `grow`, `shrink`, `basis`, `align`, `display` |
| `ia.container.breakpt` | `size` (`small` or `large`) |
| `ia.container.column` | `height`, `breakpoints [{width, columns, ...}]` |
| `ia.container.tab` | `tabIndex` |
| `ia.container.split` | `position` (observed) |
| `ia.container.drawing`, `ia.shapes.group` | none |

The root component normally has no `position` or only a few keys [corpus].

### 6.5 `propConfig`

Keys are property paths relative to the owner: `props.text`,
`props.style.classes`, `props.series[0].data`, `custom.x`, `meta.visible`,
`position.basis`, and at view level `params.y`. Values:

```json
"custom.running": {
  "binding": {...},
  "onChange": {"enabled": true, "script": "\t..."},
  "persistent": true,
  "paramDirection": "input",
  "access": "PRIVATE"
}
```

| Key | Values |
|---|---|
| `binding` | Section 6.6. |
| `onChange` | `{enabled, script}`; the script is a body (header in 5.3). |
| `persistent` | `true`: the value is saved in the view and sent to the client. Written mostly for `custom.*` and `params.*`. |
| `paramDirection` | Only on `params.*`: `input`, `output`, `inout` [jar `ParamType`]. |
| `access` | `PRIVATE` (not sent to the browser) or `SYSTEM` [jar]. Absent means public. |

### 6.6 Bindings

```json
"binding": {
  "type": "tag",
  "config": {...},
  "transforms": [...],
  "enabled": false,
  "overlayOptOut": true,
  "previewEnabled": false
}
```

Binding types registered by the gateway [jar `BindingRegistryImpl`]: `tag`,
`query`, `expr`, `expr-struct`, `property`, `tag-history`, `http`. Others
become `BrokenBinding` at run time. Relative frequency in the corpus: property
45%, expr 31%, tag 21%, expr-struct 2%, tag-history and query under 1%.

| Type | `config` |
|---|---|
| `property` | `{path, bidirectional?}`. Paths (section 6.10). |
| `expr` | `{expression}`: Ignition expression source. |
| `expr-struct` | `{struct: {key: expression or nested object}, waitOnAll: true}`. The binding value is an object with the same keys; script transforms see `value.<key>`. |
| `tag` | `{tagPath, mode, references?, bidirectional?, fallbackDelay}`. `mode`: `direct` (literal path), `indirect` (path with `{ref}` placeholders filled from `references: {ref: "<property reference or literal>"}`; numeric keys such as `{0}` are common), `expression` (`tagPath` is an expression that evaluates to a path). `fallbackDelay` (seconds, Designer writes `2.5`) is not in the shipped schema but is always written. |
| `query` | `{queryPath, returnFormat: auto or json or dataset or scalar, parameters: {name: expression}, polling: {enabled, rate}, designerUseLimit, bypassCache}`. Parameter values are **expressions**. |
| `tag-history` | `{tags: [{path, alias?, aggregate?}] or an expression, dateRange: {startDate, endDate} or {mostRecent, mostRecentUnits: sec or min or hour or day or week or month}, returnFormat: wide or tall or calculations, returnSize: {type: raw or fixed (numRows) or interval (delay, delayUnits) ...}, aggregate, valueFormat: dataset or document, polling, ignoreBadQuality, preventInterpolation, avoidScanClassValidation, enableValueCache, calculations}` |
| `http` | `{request: {url, method, header: [{key, value}], body, auth: {type, value}}, enableCookies, connectTimeout, socketTimeout, polling}` |

Every string property inside a binding config that is described as an
expression (query parameters, `expr`, `expr-struct` leaves, tag `expression`
mode path, polling `rate` when a string, http `url`) is Ignition expression
source with `{...}` property references.

### 6.7 Transforms

`transforms` is an ordered list. Registered types [jar]: `script`, `map`,
`expression`, `format`.

| Type | Shape |
|---|---|
| `script` | `{type, code}`; body only, header `def transform(self, value, quality, timestamp):`; the function must `return`. |
| `expression` | `{type, expression}`; `{value}` refers to the incoming value. |
| `map` | `{type, inputType, outputType, mappings: [{input, output}], fallback}`. `inputType`: `scalar` (Designer default; the schema calls it `primitive`), `range` (input `{min, max, exclusiveMin?, exclusiveMax?}`), `expression`. `outputType`: `scalar`/`primitive`, `color`, `style-list` (space-separated style class names), `inline-style` (style object), `expression`. |
| `format` | `{type, formatType, formatValue, locale?, timezone?}`; `formatType` `numeric` (pattern) or `datetime`; `formatValue` also accepts `currency`, `integer`, `number`, `percent`, `full`, `long`, `medium`, `short`. |

### 6.8 Events and actions

```json
"events": {
  "component": {"onActionPerformed": {"type": "script", "scope": "G", "config": {"script": "\t..."}}},
  "dom": {"onClick": [ {...action...}, {...action...} ]},
  "system": {"onStartup": {...}}
}
```

* Event groups: `dom` (browser events, list in `dom_events.json`: mouse,
  keyboard, pointer, touch, focus, ... for example `onClick`, `onDoubleClick`,
  `onMouseEnter`, `onKeyDown`, `onWheel`), `component` (component-specific,
  from the descriptor `events`, for example `onActionPerformed`,
  `onRowClick`, `onEditCellCommit`, `onFileReceived`), `system` (`onStartup`,
  `onShutdown`), and `model` (reserved) [jar `EventConfig`].
* The value is one action object or a **list** of actions (run in order).
* Action keys: `type`, `scope`, `config`, optional `enabled`, `permissions`
  (`{securityLevels, type}`), `preventDefault` [jar `ActionConfig`].
* `scope` is `G` (runs on the gateway) or `C` (runs in the browser) [jar
  `ActionScope`]. Script and auth-challenge actions are `G`; the others are `C`.

Action types [jar, client JS]:

| `type` | Scope | `config` |
|---|---|---|
| `script` | G | `{script}` (header `def runAction(self, event):`) |
| `auth-challenge` | G | `{idp, forceAuth, timeout, payload, framing}` |
| `nav` | C | one of `{page, newTab?}` (page URL), `{url, newTab?}` (external URL), `{view, params?}` (view path), or browser back/forward |
| `popup` | C | `{type: open or close or toggle, id, viewPath, viewParams, title, modal, showCloseIcon, draggable, resizable, overlayDismiss, viewportBound, position {...}, positionType}`. `viewParams` are literal values. |
| `dock` | C | `{type: open or close or toggle, id, viewParams}` |
| `alter-dock` | C | `{view, viewParams, display, modal, content, size, autoBreakpoint, anchor, handle, handleIcon}` |
| `login`, `logout` | C | `{}` (login may carry `forceAuth`) |
| `theme` | C | `{name}` |
| `refresh`, `debug`, `request-print`, `console-log` | C | small or empty |
| `fullscreen` | C | `{action, type}` |
| `alter-logging` | C | `{remoteLoggingEnabled, logLevel}` |
| `workstation/mode` | C | `{mode}` |
| `native/barcode`, `native/bluetooth`, `native/ndef`, `native/accelerometer` | C | mobile app data sources |

### 6.9 Component scripts

```json
"scripts": {
  "customMethods": [{"name": "describe", "params": ["prefix", "suffix"], "script": "\t..."}],
  "messageHandlers": [{"messageType": "motor-refresh", "pageScope": true, "sessionScope": false, "viewScope": false, "script": "\t..."}],
  "extensionFunctions": null
}
```

* `customMethods`: callable as `self.<name>(...)` (and on the component from
  other scripts). Header in 5.3.
* `messageHandlers`: receive `system.perspective.sendMessage(messageType,
  payload, scope=...)` where scope is `session`, `page` or `view`; the
  booleans say which scopes this handler listens to.
* `extensionFunctions`: `null` or `[{name, enabled, script}]`. Only components
  that declare extension functions in their descriptor accept them:
  `ia.display.alarmstatustable` (`filterAlarm(alarmEvent)`,
  `filterShelvedAlarm(shelvedAlarmEvent)`), `ia.display.alarmjournaltable`
  (`filterAlarm(alarmEvent)`), `ia.display.tag-browse-tree`
  (`filterBrowseNode(node)`). The descriptor also carries the default body
  (`\treturn True`).

### 6.10 References inside a view

Property binding `path` values and `{...}` references in expressions use the
same syntax:

| Prefix | Target |
|---|---|
| `view.params.x`, `view.custom.x`, `view.props.x` | The owning view. |
| `this.props.x`, `this.custom.x`, `this.meta.x`, `this.position.x` | The component itself. |
| `parent.props.x` | Parent component. |
| `/root/A/B.props.x` | Absolute component path from the view root (names joined by `/`). |
| `./Child.props.x` | Child of this component. |
| `../Sibling.props.x` | Sibling (parent's child). |
| `.../X/Y.props.x`, `..../X.props.x` | Each extra dot goes one level further up. |
| `session.props.x`, `session.custom.x` | Session props (section 7.3). |
| `page.props.x` | Page props. |

In expressions, `{[provider]path/to/tag}` and `{[.]relative}` are tag
references (in tag expressions), and `tag("...")` reads tags. Script access
mirrors this: `self.view.params.x`, `self.getSibling("Name")`,
`self.getChild("Name")`, `self.parent`, `self.session.custom.x`,
`self.page`.

### 6.11 Embedded views and view paths

View paths are resource paths below `com.inductiveautomation.perspective/views/`
without leading slash (for example `Popups/MotorDetail`). Places that contain
one [schema]:

| Component or API | Location |
|---|---|
| `ia.display.view` | `props.path`, params in `props.params` |
| `ia.display.flex-repeater` | `props.path`, one param object per entry in `props.instances[]` |
| `ia.display.viewcanvas` | `props.instances[].viewPath` / `viewParams` |
| `ia.display.carousel` | `props.views[].viewPath` / `viewParams` |
| `ia.display.accordion` | `props.items[].body.viewPath`, `props.items[].header.content.viewPath` |
| `ia.display.dashboard` | `props.widgets[].viewPath`, `props.availableWidgets[].viewPath` |
| `ia.container.tab` | `props.tabs` are labels; content is `children` with `position.tabIndex` |
| `ia.display.table` | `props.rows.subview.viewPath`, `props.columns[].viewPath` |
| `popup` / `dock` / `alter-dock` actions | `config.viewPath` / `config.view` |
| `nav` action | `config.view` (view) or `config.page` (page URL, section 7.2) |
| page config | `pages[url].viewPath`, dock `viewPath` |
| Scripts | `system.perspective.navigate(page=..., url=..., view=..., params=...)`, `openPopup(id, view, params=..., ...)`, `togglePopup(id, view, ...)`, `closePopup(id)`, `openDock(id, ...)`, `closeDock`, `toggleDock`, `alterDock(id, config)` |

`ia.display.icon` and `ia.shapes.svg` also have `props.path`, but it is an icon
or SVG path, not a view path. Values may be bound; only literal strings can be
resolved statically.

---

## 7. Other Perspective project resources

### 7.1 Style classes (`style-classes/<path>/style.json`)

```json
{
  "base": {
    "style": {"color": "#1B5E20", "fontWeight": "bold"},
    "animation": {"duration": "2s", "delay": "0s", "direction": "alternate", "iterationCount": "infinite", "timingFunction": "ease", "fillMode": "both"}
  },
  "variants": [
    {"pseudo": "hover", "style": {...}},
    {"mediaQuery": "max-width: 600px", "style": {...}}
  ]
}
```

`pseudo` values: `active`, `checked`, `default`, `disabled`, `empty`,
`enabled`, `first-child`, `fullscreen`, `focus`, `hover`, `in-range`,
`invalid`, `last-child`, `link`, `only-child`, `out-of-range`, ... [schema
`style-class-schema.json`]. Legacy style classes may store the same JSON in
`data.bin`. A class is referenced by its path (`Brand/Primary`) in
`props.style.classes` (space-separated) and in `style-list` map outputs.
Classes are compiled to CSS class `psc-<path>` (slashes escaped), which the
project `stylesheet.css` can target.

### 7.2 Page configuration (`page-config/config.json`)

```json
{
  "pages": {
    "/": {"title": "Overview", "viewPath": "Main/Overview"},
    "/motors/:line": {"title": "Motors", "viewPath": "Main/Overview", "docks": {"right": [ {...dock...} ]}}
  },
  "sharedDocks": {
    "cornerPriority": "top-bottom",
    "top": [ {"id": "header", "viewPath": "Docks/Header", "viewParams": {}, "show": "visible", "size": 56, "anchor": "fixed", "content": "auto", "handle": "hide", "iconUrl": "", "modal": false, "resizable": false, "autoBreakpoint": 480} ]
  }
}
```

* Page keys are URLs starting with `/`. `:name` segments become view params of
  the page's view.
* Dock sides: `top`, `left`, `right`, `bottom`. Dock fields: `id`,
  `viewPath`, `viewParams`, `show` (`visible`, `onDemand`, `hidden`), `size`,
  `anchor`, `content`, `handle`, `iconUrl`, `modal`, `resizable`,
  `autoBreakpoint`.

### 7.3 Session properties (`session-props/props.json`)

```json
{
  "custom": {"selectedLine": "Line1"},
  "propConfig": {"custom.selectedLine": {"persistent": true}},
  "props": {"theme": "light", "locale": "en-US"}
}
```

Same `propConfig` model as views (bindings, transforms, change scripts on
`custom.*`). Built-in `props` keys [schema `session-props.json`]: `id`, `host`,
`theme`, `locale`, `timeZoneId`, `lastActivity`, `auth` (user, roles,
securityLevels, ...), `gateway`, `device`, `bluetooth`, `geolocation`,
`appBar`, `pipes`, `symbols`, `googleMapsApiKey`, `offline`.

### 7.4 Session and page events

See the table in section 2 and headers in 5.2. In the fixtures:
`com.inductiveautomation.perspective/{startup,shutdown,page-startup,barcode}/`,
`message/<name>/`, `key-event/<name>/`, `form-submission-handler/<name>/`.

---

## 8. Named queries (`ignition/named-query/<path>/`)

`query.sql` holds the SQL text. `resource.json` (manifest `version: 2`)
[jar `NamedQuery.toResource`]:

```json
{
  "scope": "A",
  "documentation": "Machines on one production line",
  "version": 2,
  "restricted": false,
  "overridable": true,
  "files": [
    "query.sql"
  ],
  "attributes": {
    "cacheAmount": 1,
    "cacheEnabled": false,
    "database": "Production",
    "useMaxReturnSize": false,
    "fallbackEnabled": false,
    "autoBatchEnabled": false,
    "permissions": [
      {}
    ],
    "maxReturnSize": 100,
    "cacheUnit": "SEC",
    "type": "Query",
    "parameters": [
      {
        "type": "Parameter",
        "identifier": "line",
        "sqlType": 7
      }
    ],
    "enabled": true
  }
}
```

* `type`: `Query`, `ScalarQuery`, `UpdateQuery`.
* `database`: connection name; `""` means the project default database.
* `parameters[].type`: `Parameter` (bound with `:name` in the SQL),
  `QueryString` (textual substitution with `{name}`, a SQL injection risk),
  `Database` (selects the connection).
* `sqlType` is the Ignition `DataType` integer: Int1 0, Int2 1, Int4 2, Int8 3,
  Float4 4, Float8 5, Boolean 6, String 7, DateTime 8, DataSet 9, Text 10,
  Int4Array 11, Int8Array 12, Float8Array 13, BooleanArray 14, StringArray 15,
  DateTimeArray 16, Int1Array 17, Int2Array 18, Float4Array 19, ByteArray 20,
  Document 29 [jar].
* Optional: `fallbackValue`, `syntaxProvider`, `cacheUnit` (`MS`, `SEC`, `MIN`,
  `HOUR`, `DAY`, `WEEK`, `MONTH`, `YEAR`).
* `--` comments are removed before parameter parsing [jar `QueryParser`].
* Callers: query bindings (`queryPath`, `parameters`) and
  `system.db.runNamedQuery(path, params)` (in Perspective and gateway scope the
  project is implicit).

---

## 9. Tags (`ignition/tag-definition`, `ignition/tag-type-definition`)

### 9.1 On-disk layout [jar `TagConfigResource`][gw]

```text
ignition/tag-definition/<provider>/unary-resource.json        <- provider root folder
ignition/tag-definition/<provider>/<Folder>/unary-resource.json
ignition/tag-definition/<provider>/<Folder>/tags.json         <- non-complex tags in this folder
ignition/tag-definition/<provider>/<Folder>/udts.json         <- complex tags (UDT instances) in this folder
ignition/tag-type-definition/<provider>/unary-resource.json   <- the _types_ root
ignition/tag-type-definition/<provider>/<TypeFolder>/udts.json <- UDT definitions
```

* Every tag folder is a directory with a `unary-resource.json`. Its
  `attributes.config` holds the folder's own tag properties (often `{}`).
* The data files **must be listed** in `files` (`["tags.json"]`,
  `["udts.json"]`, both, or `[]`); unlisted files are silently ignored [gw].
* Each data file is a JSON array of tag objects. Members of UDT instances and
  UDT types are nested in `tags`. Folder objects inside these arrays also
  occur (observed in `udts.json`).
* UDT definition path = type folder path + name; `typeId` references use that
  path relative to `_types_` (for example `Motors/Motor`). A UdtType with
  `typeId` inherits from (extends) that parent type.
* Manifest scope is `G`.

### 9.2 Tag object

Common keys (JSON names of `WellKnownTagProps`, with defaults) [jar]:

| Key | Notes |
|---|---|
| `name`, `tagType` | `tagType`: `AtomicTag`, `Folder`, `UdtInstance`, `UdtType` (also `Provider`, `Node`, `Property`, `TagModel`, `Unknown` internally). |
| `typeId` | UDT instances: the UDT path. UDT types: parent type. |
| `enabled` | default `true` |
| `valueSource` | `memory` (default), `opc`, `expr`, `db` (query tag), `reference`, `derived` |
| `dataType` | `Int1`...`Int8`, `Float4`, `Float8`, `Boolean`, `String`, `DateTime`, `Text`, `*Array`, `DataSet`, `Document` (default `Int4`) |
| `value` | memory tag value |
| `opcServer`, `opcItemPath` | `opc` source |
| `expression` | `expr` source; Ignition expression with `{[.]Sibling}` / `{[~]FromProvider}` / `{[provider]Path}` tag references |
| `datasource`, `query`, `queryType` | `db` source |
| `sourceTagPath` | `reference` and `derived` sources |
| `deriveExpressionGetter`, `deriveExpressionSetter` | `derived` source; `{source}` and `{value}` placeholders |
| `executionMode` (`EventDriven`, `FixedRate`, `TagGroupRate`), `executionRate`, `tagGroup` (default `Default`) | |
| `valuePersistence` (`None`, `Database`, `Configuration`, `Inherited`), `persistValue` | |
| `engUnit`, `engLow`, `engHigh`, `engLimitMode`, `formatString`, `deadband`, `deadbandMode`, `scaleMode` (`Off`, `Linear`, `SquareRoot`, `ExponentialFilter`, `BitInversion`), `rawLow`, `rawHigh`, `scaledLow`, `scaledHigh`, `clampMode`, `scaleFactor` | numeric |
| `tooltip`, `documentation`, `typeColor` | metadata |
| `accessRights` (`Read_Only`, `Read_Write`, `Custom`), `readOnly`, `readPermissions`, `writePermissions`, `permissionModel` | security |
| `historyEnabled`, `historyProvider`, `historySampleRate`, `historySampleRateUnits`, `historyMaxAge`, `historyMaxAgeUnits`, `historyTimeDeadband`, `historicalDeadband`, `sampleMode` (`OnChange`, `Periodic`, `TagGroup`) | history |
| `parameters` | UDT types and instances: `{Name: {"dataType": "String", "value": "..."}}` (bare values are accepted on input). Instances inherit parameters they do not override. |
| `eventScripts` | `[{"eventid": "valueChanged", "script": "\t...", "enabled": false?}]`; event ids: `valueChanged`, `qualityChanged`, `qualifiedValueChanged`, `alarmActive`, `alarmCleared`, `alarmAcked` (headers in 5.3). |
| `alarms` | list of alarm objects (9.3) |
| `alarmEvalEnabled` | |
| `tags` | children (folders, UDT members) |

Bound properties [jar round-trip through `TagGson`][gw]: any property value
may be an object instead of a literal:

```json
"opcItemPath": {"bindType": "parameter", "binding": "ns=1;s=[{DeviceName}]{BaseAddress}.Speed"}
"engHigh": {"bindType": "parameter", "binding": "{MaxSpeed}"}
```

`parameter` bindings substitute `{ParamName}` (plus built-ins such as
`{InstanceName}`, `{PathToParentFolder}`) as text; `{MaxSpeed}` alone yields
the typed parameter value. The deserializer also accepts `bindType:
"Expression"` with `binding` holding an expression; the corpus only uses
`parameter`.

### 9.3 Alarms

```json
{"name": "Overspeed", "mode": "AboveValue", "priority": "High",
 "setpointA": {"bindType": "Tag", "value": "[.]../Setpoints/MaxSpeed"},
 "label": {"bindType": "Expression", "value": "'Overspeed on ' + {itemName}"},
 "displayPath": "{InstanceName}/Speed", "activePipeline": "Notify"}
```

* `mode`: `Equality`, `Inequality`, `AboveValue`, `BelowValue`,
  `BetweenValues`, `OutsideValues`, `OutOfEngRange`, `BadQuality`,
  `AnyChange`, `Bit`, `OnCondition`, `WhenTrue`, `WhenFalse` [jar].
* `priority`: `Diagnostic`, `Low`, `Medium`, `High`, `Critical`.
* Other properties: `enabled`, `label`, `displayPath`, `setpointA`,
  `setpointB`, `deadband`, `deadbandMode`, `timeOnDelaySeconds`,
  `timeOffDelaySeconds`, `ackMode`, `notes`, `ackNotesReqd`,
  `shelvingAllowed`, `notifyInitialEvent`, `activePipeline`, `clearPipeline`,
  `ackPipeline` (pipeline names in the `defaultPipelineProject`).
* Alarm property bindings use `{"bindType": "Tag" | "Expression" |
  "UDTParameter", "value": ...}` (note `value`, not `binding`) [jar
  `BoundAlarmProperty.BindType`]. A `parameter` bind type is rejected here.

---

## 10. WebDev (`com.inductiveautomation.webdev/resources/<path>/`)

The resource kind is in `config.json` `resource-type` [jar]:

| `resource-type` | Files |
|---|---|
| `python-resource` | `config.json` + `doGet.py`, `doPost.py`, `doPut.py`, `doDelete.py`, `doHead.py`, `doOptions.py`, `doTrace.py`, `doPatch.py` (all eight are written) |
| `text-resource` | text content and content type |
| `file-resource` | `file.bin` and `content-type` |
| `mounted-folder-resource` | `folder-path` |

Python resource `config.json`, one object per method:

```json
{
  "resource-type": "python-resource",
  "doGet": {"enabled": true, "max-retry-attempts": 3, "require-auth": false, "require-https": false, "required-roles": "", "user-source": ""},
  "doPost": {...}
}
```

Method files start with `def doGet(request, session):` (5.2). The URL is
`/system/webdev/<project>/<resource path>`. `request` is a dict-like object
(`params`, `headers`, `data`, `postData`, `servletRequest`, ...); the return
value is a dict with one of `json`, `html`, `file`, `bytes`, `response`.

---

## 11. Facts that matter for iglint

1. **Two script storage styles.** Gateway/session/WebDev files contain a full
   `def`; Perspective and tag scripts contain a tab-indented body in a JSON
   string. Both are compiled inside a canonical header that iglint must
   reproduce (5.2, 5.3).
2. **The header line in `.py` event files is replaced**, not executed (5.1).
3. **Top-level script packages are globals**; `project.` and `shared.` do not
   exist in 8.3 (3.2).
4. **A module with its own `code.py` hides its sub-packages** (4.2).
5. **Child resources replace parent resources whole** (1.4).
6. **`files` in manifests is authoritative** (1.2, 9.1).
7. **Designer JSON is Gson HTML-escaped and key-sorted** (6.1).

---

## 12. Lint opportunities

Grouped by structure. Each is a candidate rule; "error" means it fails at run
time, the rest are quality findings.

### Project and resources

* `resource.json` lists a file that does not exist; a data file exists that is
  not listed (it is ignored by the gateway).
* Malformed `resource.json` / `project.json` (unknown scope code, missing keys,
  wrong types, unknown attribute key such as a misspelled `threadType`).
* `parent` names a project that does not exist, or is not `inheritable`.
* Child overrides a parent resource marked `overridable: false`.
* Child resource identical to the parent's (pointless override), or overriding a
  library module while dropping functions callers still use.
* Resource or folder names that are invalid for their type (script paths not
  matching `[_a-zA-Z]\w*`).

### Script library

* Module with `code.py` that also has child modules (children unreachable).
* Reference to `project.x` or `shared.x` (pre-8 prefixes), or to `app.`.
* Call to `pkg.mod.func` where the module or function does not exist in the
  effective (inherited) library.
* Module-level side effects (I/O, tag writes, `system.*` calls) in `code.py`:
  run at import, in every scope.
* `hintScope` that does not match usage (for example Gateway-only APIs in a module
  hinted for Designer only); informational.
* Name collision between a top-level package and a builtin or `system`.

### Gateway and session event files

* First line is not exactly the canonical header (wrong parameters, wrong name,
  defaults, wrapped header, one-line body, missing header).
* Use of a name that is not a parameter (for example `payload` in a timer).
* Timer `delay` of 0 or extremely small; `sharedThread` with long-running work.
* Tag change script with empty `paths`, invalid tag path syntax, or paths to
  tags that do not exist in the gateway config.
* Scheduled script with an invalid `cronExpression`.
* Message handler `threadType` not in `Shared`/`Dedicated`; handler never
  sent to (`system.util.sendMessage` with that name nowhere), or
  `sendMessage` to a handler that does not exist.
* Disabled event with non-trivial code (dead code), or enabled event with
  `pass` only.

### Views: structure

* Duplicate `meta.name` among siblings (breaks `getChild`, `/root/...` paths).
* `position` keys that do not belong to the parent container type.
* Unknown component `type` (not in the descriptors and not from a known module).
* Unknown `props` key for the component type (per schema), wrong value type.
* `meta.visible: false` component with bindings (wasted work).
* Coordinate container children that overlap or fall outside the view size.
* Very deep nesting or very large views (performance).

### Views: params, custom, propConfig

* Binding or script reads `view.params.x` / `view.custom.x` / `this.custom.x`
  that is not defined.
* Param without `propConfig` entry or without `paramDirection`.
* `output`/`inout` param never written; `input` param written by a script.
* Custom property defined but never read (`unused-custom-property`).
* `persistent: true` on large bound values (dataset/query results); binding on
  a persistent property.
* `onChange` with `enabled: false` (dead script) or empty script.
* `onChange` that writes the property it watches (loop) without an `origin`
  guard.
* `access: PRIVATE` property referenced from the client side.

### Bindings and transforms

* Binding type not in the registered set; config keys not in the schema.
* Expression syntax errors in `expr`, `expr-struct` leaves, query
  parameters, tag `expression` mode, `{value}` transforms.
* Property path that cannot resolve (unknown component in `/root/...`,
  `../X`, unknown property).
* Tag binding `indirect` whose `tagPath` placeholders and `references` keys do
  not match; `direct` mode containing `{...}`; tag path to a provider or tag that does
  not exist.
* Bidirectional binding on a read-only target or on an expression-mode tag.
* Query binding to a named query that does not exist; parameters missing or
  extra compared to the query; `QueryString` parameters fed from user input
  (SQL injection); polling at a high rate.
* Tag history binding polling faster than the data rate; unbounded
  `startDate`.
* HTTP binding with credentials in plain text, `http://` URLs.
* Script transform without `return`, or that ignores `value`.
* Map transform with duplicate inputs, missing fallback, `style-list` output
  naming a style class that does not exist, `inputType` mismatching mapping
  inputs.
* Disabled binding (`enabled: false`) left in place.
* Chain of transforms that could be one expression (performance).

### Events and actions

* Script action with empty or whitespace-only script (compile error, see the
  fixture `Sandbox/LintSamples`).
* Unknown action `type`; `scope` that does not match the type.
* `popup`/`dock` action to a view that does not exist; `close` without `id`;
  popup `viewParams` keys that are not params of the target view; non-literal
  values (property references) in `viewParams`.
* `nav` to a page URL not in page config, or to a view that does not exist.
* `system.perspective.navigate/openPopup/openDock/sendMessage` in scripts with
  literal view paths, page URLs, popup ids or message types that do not
  resolve.
* DOM `onClick` on a button that also has `onActionPerformed` (double firing).
* Event list with a script action after a `nav`/`logout` (may not run).

### Component scripts

* Custom method name that shadows a component API (`refreshBinding`,
  `getChild`, ...); custom method never called.
* Message handler `messageType` never sent; all three scope flags false (never
  receives).
* Extension function on a component type that has none, or with a name not
  in its descriptor; extension function enabled with only the default body.

### Style

* `props.style.classes` or map output naming a missing style class.
* Style class never used; empty style class.
* Inline styles duplicated across many components (should be a class).

### Page config and session

* Page `viewPath` or dock `viewPath` that does not exist; duplicate dock ids;
  page URL params (`:name`) that are not params of the page view.
* Session `custom` property never read; session binding to a missing tag.

### Named queries

* SQL syntax issues for the target database (best effort); `:param` in SQL
  not declared or declared but unused; `{param}` used for a `Parameter` type.
* `QueryString` parameters (injection risk).
* `UpdateQuery` with caching enabled; `Query` without a row limit.
* Named query never referenced.

### Tags and UDTs

* Tag event script syntax errors (body compiled under the tag headers).
* `expression`/`derive*` expression errors; references to tags that do not
  exist; `{Param}` in a UDT member that is not a UDT parameter.
* `typeId` that does not resolve to a UDT definition; UDT inheritance cycles.
* `historyProvider` naming a provider that does not exist.
* Alarm `setpointA`/`label` bindings with the wrong bind type; pipelines that do
  not exist; alarms on disabled tags.
* `db` tags with `executionMode` too fast; `datasource` that does not exist.
* Data files not listed in `unary-resource.json`.

### WebDev

* Enabled method whose file has only `pass`; disabled method with code.
* `require-auth: false` on mutating methods (`doPost`, `doPut`, `doDelete`).
* Header line mismatch (5.1).

---

## Appendix A. Resource types registered by 8.3.3 [jar]

`ignition/`: `alarm-journal`, `api-token`, `audit-profile`, `cobranding`,
`database-connection`, `database-driver`, `database-translator`,
`designer-properties`, `edge-sync-settings`, `edge-system-properties`,
`email-profile`, `event-scripts` (legacy aggregate), `gateway-network-incoming`,
`gateway-network-outgoing`, `gateway-network-proxy-rules`,
`gateway-network-queue-settings`, `gateway-network-settings`,
`general-alarm-settings`, `global-props`, `holiday`, `identity-provider`,
`images`, `keyboard_layout`, `local-system-properties`, `message`,
`metrics-dashboard`, `named-query`, `oauth2-client`, `opc-connection`,
`quickstart`, `roster-config`, `schedule`, `scheduled`, `script-app-library`,
`script-python`, `secret-provider`, `security-levels`, `security-properties`,
`security-zone`, `service-connector`, `shutdown`, `smtp.classic`, `startup`,
`store-and-forward-engine`, `system-properties`, `tag-change`,
`tag-definition`, `tag-group`, `tag-provider`, `tag-report-search`,
`tag-type-definition`, `tags`, `timer`, `translations`, `update`,
`user-source`, `web-server`.

`com.inductiveautomation.perspective/`: `accelerometer`, `auth-challenge`,
`barcode`, `bluetooth`, `fonts`, `form-submission-handler`, `gateway-scripts`,
`general-properties`, `icons`, `inactivity-properties`, `key-event`,
`message`, `nfc-scan`, `offline-mode-settings`, `page-config`,
`page-startup`, `session-permissions`, `session-props`, `session-scripts`,
`shutdown`, `startup`, `style-classes`, `stylesheet`,
`symbol-state-settings`, `tag-drop-settings`, `themes`, `views`.

`com.inductiveautomation.vision/`: `client-event-scripts`, `client-tags`,
`custom-palettes`, `designer-properties`, `general-properties`, `keystroke`,
`launch-properties`, `login-properties`, `menubar`, `message`,
`polling-properties`, `shutdown`, `shutdown-intercept`, `startup`,
`tag-change`, `templates`, `timer`, `ui-properties`, `windows`.

Other modules: `com.inductiveautomation.webdev/resources`,
`com.inductiveautomation.reporting/reports`,
`com.inductiveautomation.sfc/charts`, `com.inductiveautomation.sfc/chart-settings`,
`com.inductiveautomation.alarm-notification/alarm-pipelines`,
`com.inductiveautomation.alarm-notification/alarm-notification-profile`,
`com.inductiveautomation.sqlbridge/transaction-groups`,
`com.inductiveautomation.eventstream/event-streams`,
`com.inductiveautomation.historian/historian-provider`,
`com.inductiveautomation.opcua/{device,access-control,server-config,one-time,client-keystore,server-keystore,uuid}`,
`com.inductiveautomation.eam/*`, `com.inductiveautomation.sip-notification/script-settings`.

## Appendix B. Machine-readable sources inside the image

For `iglint_stubs` generation:

| Data | Location in the image |
|---|---|
| Perspective component ids, prop schemas, events, child position schemas, extension functions | `user-lib/modules/Perspective-module.modl` > `perspective-common-*.jar` > `ia.components.json` and `*.components.json` |
| Binding and transform config schemas | same jar, `schemas/binding-*.json`, `schemas/transform-*.json` |
| View props, meta, session props, style class schemas | same jar, `schemas/view-props-schema.json`, `meta-schema.json`, `session-props.json`, `style-class-schema.json` |
| Scripting object descriptors (session, page, view, component) | same jar, `descriptors/*.json` |
| DOM event names | `perspective-designer-*.jar` > `dom_events.json` |
| Generated script headers | call `ExtensionFunctionDescriptor.buildMethod`, `TagScriptManager.buildSignature`, `CustomMethodConfig.getSignature` with the bundled JRE (`lib/runtime/jre`) and Jython (`lib/core/common/jython-ia-*.jar`) |

## Appendix C. Fixtures

`crates/iglint_project/tests/fixtures/basic/`:

| Path | Covers |
|---|---|
| `projects/fixture-base` | Inheritable parent: script library, named queries (`Query`, `UpdateQuery` with `QueryString`), style classes (base, variants, pseudo, media query, animation), a view used by the child |
| `projects/fixture-hmi` | Child project: library override, every binding type, every transform type, script/popup/nav/dock/logout actions, action lists, view and component events, custom methods, message handlers, an extension function, every container `position` style, embedded views (view, flex repeater, carousel, accordion, table subview, tab container), page config with docks, session props with binding, stylesheet, session startup/shutdown/page-startup/message/key-event/barcode events and a form submission handler |
| `projects/fixture-hmi/.../views/Sandbox/LintSamples` | Deliberate problems: unknown param, missing view, empty script action, navigate/openPopup to missing views, missing style class, expression syntax error, transform without return, missing named query, duplicate sibling names, disabled `onChange` |
| `projects/fixture-api` | Standalone project: all gateway events (startup, shutdown, update, timer, scheduled, message, tag change), a WebDev python resource, a module that shadows its sub-package |
| `services/gateway/config/resources` | Collection chain (`external`, `core`, `production`, `local`), tag provider, tag group, database connection with a mode override, tag folders with `tags.json` and `udts.json`, UDT definitions with parameters, bound properties, alarms (with Tag/Expression bindings) and tag event scripts |
