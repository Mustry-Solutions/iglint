import * as assert from "node:assert/strict";
import { test } from "node:test";

import { Environment, needsRestart, readSettings, resolveServerPath, serverSettings } from "../src/config";

function env(files: string[], overrides: Partial<Environment> = {}): Environment {
  return {
    platform: "linux",
    arch: "x64",
    pathVariable: "/usr/local/bin:/usr/bin",
    home: "/home/me",
    workspaceFolder: "/work/plant",
    extensionPath: "/ext",
    isFile: (f) => files.includes(f),
    ...overrides,
  };
}

test("configured path wins, with expansion", () => {
  assert.equal(resolveServerPath("/opt/iglint", env([])), "/opt/iglint");
  assert.equal(resolveServerPath("~/bin/iglint", env([])), "/home/me/bin/iglint");
  assert.equal(resolveServerPath("${workspaceFolder}/tools/iglint", env([])), "/work/plant/tools/iglint");
  assert.equal(resolveServerPath("tools/iglint", env([])), "/work/plant/tools/iglint");
});

test("bundled binary for this platform before PATH", () => {
  const bundled = ["/ext/bundled/linux-arm64/iglint", "/ext/bundled/linux-x64/iglint", "/usr/bin/iglint"];
  assert.equal(resolveServerPath("", env(bundled)), "/ext/bundled/linux-x64/iglint");
  assert.equal(resolveServerPath("", env(bundled, { arch: "arm64" })), "/ext/bundled/linux-arm64/iglint");
  // No binary for this platform: PATH.
  assert.equal(resolveServerPath("", env(bundled, { platform: "darwin" })), "/usr/bin/iglint");
});

test("first iglint on PATH", () => {
  assert.equal(resolveServerPath("", env(["/usr/bin/iglint"])), "/usr/bin/iglint");
  assert.equal(resolveServerPath("", env([])), undefined);
});

test("windows names and separators", () => {
  const e = env(["C:\\tools\\iglint.exe"], {
    platform: "win32",
    pathVariable: "C:\\Windows;C:\\tools",
    extensionPath: "C:\\ext",
  });
  assert.equal(resolveServerPath("", e), "C:\\tools\\iglint.exe");
});

test("settings defaults and validation", () => {
  const values: Record<string, unknown> = { run: "sometimes", args: ["--x", 3], path: "  /bin/iglint " };
  const s = readSettings((k) => values[k]);
  assert.deepEqual(s, { path: "/bin/iglint", args: ["--x"], run: "onType", scope: "open", unsafeFixes: false });
  assert.deepEqual(serverSettings({ ...s, run: "onSave", scope: "workspace" }), {
    run: "onSave",
    scope: "workspace",
    unsafeFixes: false,
  });
});

test("restart only for the binary and its arguments", () => {
  const s = readSettings(() => undefined);
  assert.equal(needsRestart(s, { ...s, run: "onSave" }), false);
  assert.equal(needsRestart(s, { ...s, path: "/x" }), true);
  assert.equal(needsRestart(s, { ...s, args: ["-v"] }), true);
});
