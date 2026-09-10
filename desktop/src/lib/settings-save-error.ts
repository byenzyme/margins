export function settingsSaveErrorMessage(error: unknown, hosted: boolean): string {
  const message = error instanceof Error ? error.message : String(error);
  const detail = message.replace(/^(?:error:\s*)+/i, "").trim();
  if (hosted) {
    if (detail.startsWith("Hosted credentials are managed by the server environment")) {
      return detail;
    }
    return "Couldn’t save settings. Check that Margins is connected, then try again.";
  }

  return detail ? `Couldn’t save settings: ${detail}` : "Couldn’t save settings. Try again.";
}
