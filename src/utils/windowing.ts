export const WINDOWING_THRESHOLD = 100;
export const WINDOW_DEFAULT_ROW_HEIGHT = 260;
export const WINDOW_MIN_ROW_HEIGHT = 120;
export const WINDOW_MAX_ROW_HEIGHT = 520;
const WINDOW_OVERSCAN = 8;

export function computeWindowRange(
  total: number,
  scrollTop: number,
  viewportHeight: number,
  rowHeightEstimate: number
): { start: number; end: number } {
  if (total <= 0) return { start: 0, end: 0 };
  const rowHeight = Math.max(WINDOW_MIN_ROW_HEIGHT, Math.min(WINDOW_MAX_ROW_HEIGHT, rowHeightEstimate));
  const start = Math.max(0, Math.floor(scrollTop / rowHeight) - WINDOW_OVERSCAN);
  const visibleCount = Math.ceil(viewportHeight / rowHeight);
  const end = Math.min(total - 1, start + visibleCount + WINDOW_OVERSCAN * 2);
  return { start, end };
}
