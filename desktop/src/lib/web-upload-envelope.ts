export interface WebUploadEnvelope {
  ok?: boolean;
  error?: string;
}

/** Hosted upload endpoints deliberately return a JSON command envelope even
 * for application-level rejection. HTTP 200 is transport success, not proof
 * that the server durably accepted the bytes. */
export async function requireSuccessfulWebUpload(
  response: Response,
  label: string,
): Promise<void> {
  const envelope = await response.json().catch(() => null) as WebUploadEnvelope | null;
  if (!response.ok || envelope?.ok !== true) {
    throw new Error(
      envelope?.error ?? `${label} failed (HTTP ${response.status}${response.ok ? ", invalid success envelope" : ""})`,
    );
  }
}
