import * as assert from "node:assert/strict";
import * as vscode from "vscode";

function isMissingProperty(diagnostic: vscode.Diagnostic): boolean {
  return (typeof diagnostic.code === "object" ? diagnostic.code.value : diagnostic.code) === "BIND005";
}

function waitForDiagnostics(uri: vscode.Uri, missing: boolean): Promise<void> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      subscription.dispose();
      reject(new Error(`Timed out waiting for BIND005 to ${missing ? "appear" : "clear"}`));
    }, 20_000);
    const subscription = vscode.languages.onDidChangeDiagnostics((event) => {
      if (!event.uris.some((changed) => changed.toString() === uri.toString())) return;
      if (vscode.languages.getDiagnostics(uri).some(isMissingProperty) !== missing) return;
      clearTimeout(timer);
      subscription.dispose();
      resolve();
    });
  });
}

export async function run(): Promise<void> {
  // Running a contributed command activates the extension just as it does
  // from the Command Palette, including starting the actual server binary.
  await vscode.commands.executeCommand("iglint.showOutput");
  assert.equal(vscode.extensions.getExtension("mustry-solutions.iglint")?.isActive, true);

  const folder = vscode.workspace.workspaceFolders![0].uri;
  const uri = vscode.Uri.joinPath(folder, "projects/demo/com.inductiveautomation.perspective/views/Main/view.json");
  const document = await vscode.workspace.openTextDocument(uri);
  await vscode.window.showTextDocument(document);
  const source = document.getText();
  const original = "view.custom.value";
  const offset = source.indexOf(original);
  assert.ok(offset >= 0);

  const reported = waitForDiagnostics(uri, true);
  const edit = new vscode.WorkspaceEdit();
  edit.replace(uri, new vscode.Range(document.positionAt(offset), document.positionAt(offset + original.length)), "view.custom.missing");
  assert.equal(await vscode.workspace.applyEdit(edit), true);
  await reported;
  console.log("iglint: missing custom property reported for an unsaved binding change");

  const cleared = waitForDiagnostics(uri, false);
  const restore = new vscode.WorkspaceEdit();
  restore.replace(uri, new vscode.Range(document.positionAt(0), document.positionAt(document.getText().length)), source);
  assert.equal(await vscode.workspace.applyEdit(restore), true);
  await cleared;
  console.log("iglint: diagnostic cleared after correcting the binding");
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
}
