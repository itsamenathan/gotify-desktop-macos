const ENABLE_MESSAGE_DEBUG = import.meta.env.DEV;

export function debugUi(event: string, payload: Record<string, unknown>): void {
  if (!ENABLE_MESSAGE_DEBUG) return;
  console.debug(`[gotify-ui] ${event}`, payload);
}
