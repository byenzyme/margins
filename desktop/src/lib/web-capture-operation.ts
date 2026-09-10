export interface WebCaptureOperation {
  readonly token: number;
  cancelled: boolean;
}

export class WebCaptureOperationController {
  private sequence = 0;
  private current: WebCaptureOperation | null = null;

  begin(): WebCaptureOperation {
    const operation = { token: ++this.sequence, cancelled: false };
    this.current = operation;
    return operation;
  }

  isCurrent(operation: WebCaptureOperation): boolean {
    return !operation.cancelled && this.current?.token === operation.token;
  }

  cancelCurrent(): boolean {
    if (!this.current) return false;
    this.current.cancelled = true;
    this.current = null;
    return true;
  }

  complete(operation: WebCaptureOperation): void {
    if (this.current?.token === operation.token) this.current = null;
  }

  async discardIfStale(
    operation: WebCaptureOperation,
    sessionName: string,
    discard: (sessionName: string) => Promise<unknown>,
  ): Promise<boolean> {
    if (this.isCurrent(operation)) return false;
    await discard(sessionName);
    return true;
  }
}
