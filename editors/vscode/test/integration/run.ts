import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import { runTests } from "@vscode/test-electron";

async function main(): Promise<void> {
  const extension = path.resolve(__dirname, "../../..");
  const executable = process.platform === "win32" ? "iglint.exe" : "iglint";
  const binary = path.resolve(process.env.IGLINT_TEST_BINARY ?? path.join(extension, "../../target/debug", executable));
  await fs.access(binary);
  const workspace = await fs.mkdtemp(path.join(os.tmpdir(), "iglint-vscode-"));
  try {
    const project = path.join(workspace, "projects/demo");
    const view = path.join(project, "com.inductiveautomation.perspective/views/Main");
    await fs.mkdir(view, { recursive: true });
    await fs.mkdir(path.join(workspace, ".vscode"));
    await fs.writeFile(path.join(workspace, ".vscode/settings.json"), JSON.stringify({ "iglint.path": binary }));
    await fs.writeFile(path.join(project, "project.json"), JSON.stringify({ title: "Demo", enabled: true }));
    await fs.writeFile(path.join(view, "view.json"), JSON.stringify({
      custom: { value: 1 },
      root: {
        type: "ia.display.label",
        props: { text: "" },
        propConfig: {
          "props.text": { binding: { type: "property", config: { path: "view.custom.value" } } },
        },
      },
    }));
    await runTests({
      vscodeExecutablePath: process.env.IGLINT_VSCODE_EXECUTABLE,
      extensionDevelopmentPath: extension,
      extensionTestsPath: path.join(__dirname, "smoke.js"),
      launchArgs: [
        workspace, "--disable-extensions", "--disable-workspace-trust", "--no-sandbox", "--disable-gpu",
        `--user-data-dir=${path.join(workspace, "user-data")}`,
        `--extensions-dir=${path.join(workspace, "extensions")}`,
      ],
    });
  } finally {
    await fs.rm(workspace, { recursive: true, force: true });
  }
}

main().catch((error: unknown) => {
  console.error(error);
  process.exitCode = 1;
});
