import DOMPurify from "dompurify";
import { marked } from "marked";
import type { GotifyMessage, UiMessage } from "../types";

export function toUiMessage(message: GotifyMessage): UiMessage {
  const html = marked.parse(message.message || "", { gfm: true, breaks: true, async: false });
  const urls = extractPlainUrls(message.message || "");
  const parsedTs = Date.parse(message.date || "");
  return {
    ...message,
    rendered_html: DOMPurify.sanitize(html),
    primary_url: urls.length > 0 ? urls[0] : null,
    parsed_ts: Number.isNaN(parsedTs) ? null : parsedTs,
    formatted_time: formatDateTime(message.date),
  };
}

export function mergeUiMessages(current: UiMessage[], incoming: GotifyMessage[]): UiMessage[] {
  if (incoming.length === 0) return current;
  const currentById = new Map<number, UiMessage>(current.map((item) => [item.id, item]));
  return incoming.map((message) => {
    const existing = currentById.get(message.id);
    if (existing && isSameRawMessage(existing, message)) {
      return existing;
    }
    return toUiMessage(message);
  });
}

export function compareMessagesNewestFirst(a: UiMessage, b: UiMessage): number {
  const ta = a.parsed_ts;
  const tb = b.parsed_ts;
  if (tb != null && ta != null && tb !== ta) {
    return tb - ta;
  }
  if (tb != null && ta == null) return -1;
  if (ta != null && tb == null) return 1;
  return b.id - a.id;
}

function isSameRawMessage(current: UiMessage, next: GotifyMessage): boolean {
  return (
    current.id === next.id &&
    current.app_id === next.app_id &&
    current.title === next.title &&
    current.message === next.message &&
    current.priority === next.priority &&
    current.app === next.app &&
    current.app_icon === next.app_icon &&
    current.date === next.date
  );
}

function extractPlainUrls(text: string): string[] {
  const withoutMarkdownLinks = text
    .replace(/!\[[^\]]*]\((https?:\/\/[^)\s]+)(?:\s+["'][^"']*["'])?\)/gi, " ")
    .replace(/\[[^\]]*]\((https?:\/\/[^)\s]+)(?:\s+["'][^"']*["'])?\)/gi, " ")
    .replace(/<https?:\/\/[^>\s]+>/gi, " ");
  const matches = withoutMarkdownLinks.match(/https?:\/\/[^\s)]+/g);
  if (!matches) return [];
  return Array.from(new Set(matches.map((url) => url.replace(/[.,!?;:]+$/g, ""))));
}

function formatDateTime(value: string): string {
  const ts = Date.parse(value || "");
  if (Number.isNaN(ts)) {
    return "Unknown time";
  }
  const date = new Date(ts);
  const abs = date.toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
  return `${abs} (${relativeTime(ts)})`;
}

function relativeTime(ts: number): string {
  const diffSec = Math.round((ts - Date.now()) / 1000);
  const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  const abs = Math.abs(diffSec);
  if (abs < 60) return rtf.format(diffSec, "second");
  const min = Math.round(diffSec / 60);
  if (Math.abs(min) < 60) return rtf.format(min, "minute");
  const hr = Math.round(min / 60);
  if (Math.abs(hr) < 24) return rtf.format(hr, "hour");
  const day = Math.round(hr / 24);
  return rtf.format(day, "day");
}
