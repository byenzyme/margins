import assert from "node:assert/strict";
import test from "node:test";

import { settingsSaveErrorMessage } from "../src/lib/settings-save-error.ts";

test("hosted save failures do not expose raw backend errors", () => {
  const message = settingsSaveErrorMessage(
    new Error("failed to delete macOS Keychain item api-key: No such file or directory"),
    true,
  );

  assert.equal(message, "Couldn’t save settings. Check that Margins is connected, then try again.");
  assert.doesNotMatch(message, /Keychain|No such file|Error:/);
});

test("desktop save failures retain a useful normalized detail", () => {
  assert.equal(
    settingsSaveErrorMessage("Error: permission was revoked", false),
    "Couldn’t save settings: permission was revoked",
  );
});

test("hosted credential policy rejection remains actionable", () => {
  const message = "Hosted credentials are managed by the server environment; remove api_key from this settings request and configure it on the server.";
  assert.equal(settingsSaveErrorMessage(new Error(message), true), message);
});
