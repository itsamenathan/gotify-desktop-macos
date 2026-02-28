import type { PauseMode, SelectionHistoryState } from "../types";

export function normalizeSelectionMessageId(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

export function normalizePauseMode(value: unknown): PauseMode | null {
  if (value === "15m" || value === "1h" || value === "custom" || value === "forever") {
    return value;
  }
  return null;
}

export function isSelectionHistoryState(value: unknown): value is SelectionHistoryState {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<SelectionHistoryState>;
  return (
    candidate.tag === "gotify-selection-v1" &&
    typeof candidate.app === "string" &&
    (typeof candidate.messageId === "number" || candidate.messageId === null)
  );
}

export function initials(name: string): string {
  const parts = name
    .split(/\s+/)
    .map((part) => part.trim())
    .filter(Boolean)
    .slice(0, 2);
  if (parts.length === 0) return "?";
  return parts.map((part) => part[0]?.toUpperCase() ?? "").join("");
}
