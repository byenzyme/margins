export type JobBannerCancelAction = "__dismissNoteError" | "__cancelProcessing";

export function jobBannerCancelAction(error: boolean): JobBannerCancelAction {
  return error ? "__dismissNoteError" : "__cancelProcessing";
}
