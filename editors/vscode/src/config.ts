// Settings resolution, kept free of the `vscode` module so it can be unit
// tested with plain Node.

import * as path from "path";

export type RunTrigger = "onType" | "onSave";
export type Scope = "open" | "workspace";

export interface Settings {
  path: string;
  args: string[];
  run: RunTrigger;
  scope: Scope;
  unsafeFixes: boolean;
}

/** Initialization options and `workspace/didChangeConfiguration` payload. */
export interface ServerSettings {
  run: RunTrigger;
  scope: Scope;
  unsafeFixes: boolean;
}

/** Reads settings from an untyped configuration getter, with defaults. */
export function readSettings(get: (key: string) => unknown): Settings {
  const pathSetting = get("path");
  const args = get("args");
  const run = get("run");
  const scope = get("scope");
  return {
    path: typeof pathSetting === "string" ? pathSetting.trim() : "",
    args: Array.isArray(args) ? args.filter((a): a is string => typeof a === "string") : [],
    run: run === "onSave" ? "onSave" : "onType",
    scope: scope === "workspace" ? "workspace" : "open",
    unsafeFixes: get("unsafeFixes") === true,
  };
}

export function serverSettings(settings: Settings): ServerSettings {
  return { run: settings.run, scope: settings.scope, unsafeFixes: settings.unsafeFixes };
}

/** What the resolver may look at, injected for tests. */
export interface Environment {
  platform: NodeJS.Platform;
  /** `PATH`. */
  pathVariable: string;
  home: string;
  /** First workspace folder, if any. */
  workspaceFolder?: string;
  /** Directory of the extension (holds `bundled/`). */
  extensionPath: string;
  isFile: (file: string) => boolean;
}

export function executableName(platform: NodeJS.Platform): string {
  return platform === "win32" ? "iglint.exe" : "iglint";
}

/**
 * The `iglint` binary to run: `iglint.path` when set (with
 * `${workspaceFolder}` and `~` expanded, relative to the workspace folder),
 * else the bundled binary, else the first `iglint` on `PATH`. Returns
 * `undefined` when nothing is found.
 */
export function resolveServerPath(configured: string, env: Environment): string | undefined {
  const p = env.platform === "win32" ? path.win32 : path.posix;
  if (configured !== "") {
    let expanded = configured.replace(/\$\{workspaceFolder\}/g, env.workspaceFolder ?? "");
    if (expanded === "~" || expanded.startsWith("~/") || expanded.startsWith("~\\")) {
      expanded = p.join(env.home, expanded.slice(1));
    }
    if (!p.isAbsolute(expanded) && env.workspaceFolder !== undefined) {
      expanded = p.join(env.workspaceFolder, expanded);
    }
    return expanded;
  }
  const bundled = p.join(env.extensionPath, "bundled", executableName(env.platform));
  if (env.isFile(bundled)) {
    return bundled;
  }
  const separator = env.platform === "win32" ? ";" : ":";
  for (const dir of env.pathVariable.split(separator)) {
    if (dir === "") {
      continue;
    }
    const candidate = p.join(dir, executableName(env.platform));
    if (env.isFile(candidate)) {
      return candidate;
    }
  }
  return undefined;
}

/** Whether changing these settings needs a server restart. */
export function needsRestart(before: Settings, after: Settings): boolean {
  return before.path !== after.path || before.args.join("\0") !== after.args.join("\0");
}
