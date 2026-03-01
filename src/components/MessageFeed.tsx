import type { MutableRefObject, RefObject } from "react";
import type { AppGroup, PriorityThreshold, UiMessage, UrlPreview } from "../types";
import { initials } from "../utils/selection";
import { computeWindowRange } from "../utils/windowing";

type MessageFeedProps = {
  isQuickWindow: boolean;
  selectedApp: string;
  selectedAppName: string;
  appGroups: AppGroup[];
  filteredMessages: UiMessage[];
  visibleMessages: UiMessage[];
  activeMessage: UiMessage | null;
  isWindowed: boolean;
  topSpacerPx: number;
  bottomSpacerPx: number;
  estimatedRowHeightRef: MutableRefObject<number>;
  messageListRef: RefObject<HTMLUListElement | null>;
  deletingMessageIds: Record<string, boolean>;
  urlPreviews: Record<string, UrlPreview | null>;
  priorityThresholds: PriorityThreshold[];
  applySelection: (appKey: string, messageId: number | null, pushHistory: boolean) => void;
  setSelectedMessageId: (id: number | null) => void;
  setWindowRange: (range: { start: number; end: number }) => void;
  onDeleteMessage: (messageId: number) => Promise<void>;
};

export function MessageFeed({
  isQuickWindow,
  selectedApp,
  selectedAppName,
  appGroups,
  filteredMessages,
  visibleMessages,
  activeMessage,
  isWindowed,
  topSpacerPx,
  bottomSpacerPx,
  estimatedRowHeightRef,
  messageListRef,
  deletingMessageIds,
  urlPreviews,
  priorityThresholds,
  applySelection,
  setSelectedMessageId,
  setWindowRange,
  onDeleteMessage,
}: MessageFeedProps) {
  const themeBadgeColor = getThemeBadgeColor();
  return (
    <section className="content-grid">
      {!isQuickWindow ? (
        <aside className="apps-panel">
          <h2>Applications</h2>
          <button
            type="button"
            className={selectedApp === "all" ? "app-chip selected" : "app-chip"}
            onClick={() => applySelection("all", null, true)}
          >
            <span className="chip-left">All Messages</span>
          </button>

          {appGroups.map((group) => (
            <button
              key={group.key}
              type="button"
              className={selectedApp === group.key ? "app-chip selected" : "app-chip"}
              onClick={() => applySelection(group.key, null, true)}
            >
              <span className="chip-left">
                {group.icon ? (
                  <img
                    src={group.icon}
                    alt=""
                    className="app-icon"
                    onError={(event) => {
                      event.currentTarget.style.display = "none";
                      const fallback = event.currentTarget.nextElementSibling as HTMLSpanElement | null;
                      if (fallback) fallback.style.display = "inline-flex";
                    }}
                  />
                ) : null}
                <span className="app-icon-fallback" style={{ display: group.icon ? "none" : "inline-flex" }}>
                  {initials(group.name)}
                </span>
                {group.name}
              </span>
              <span>{group.count}</span>
            </button>
          ))}
        </aside>
      ) : null}

      <section className={isQuickWindow ? "feed-panel quick-feed-panel" : "feed-panel"}>
        {!isQuickWindow ? (
          <div className="feed-header">
            <div>
              <h2>{selectedAppName}</h2>
            </div>
          </div>
        ) : null}

        {filteredMessages.length === 0 ? (
          <p className="help">No messages cached yet.</p>
        ) : (
          <ul
            ref={messageListRef}
            className={isWindowed ? "message-list message-list-windowed" : "message-list"}
            onScroll={
              isWindowed
                ? (event) => {
                    const target = event.currentTarget;
                    setWindowRange(
                      computeWindowRange(
                        filteredMessages.length,
                        target.scrollTop,
                        target.clientHeight,
                        estimatedRowHeightRef.current
                      )
                    );
                  }
                : undefined
            }
          >
            {isWindowed && topSpacerPx > 0 ? (
              <li aria-hidden="true" className="message-spacer" style={{ height: `${topSpacerPx}px` }} />
            ) : null}
            {visibleMessages.map((message) => {
              const preview = message.primary_url ? urlPreviews[message.primary_url] : null;
              const color = resolvePriorityColor(message.priority, priorityThresholds, themeBadgeColor);
              const textColor = pickForegroundColor(color);
              return (
                <li
                  key={message.id}
                  data-message-id={message.id}
                  className={message.id === activeMessage?.id ? "message-item selected" : "message-item"}
                  onClick={() => {
                    if (isQuickWindow) {
                      setSelectedMessageId(message.id);
                      return;
                    }
                    const targetApp =
                      selectedApp === "all"
                        ? (message.app_id > 0 ? String(message.app_id) : "all")
                        : selectedApp;
                    applySelection(targetApp, message.id, true);
                  }}
                >
                  <div className="message-row-top">
                    <div className="message-title-wrap">
                      {isQuickWindow ? (
                        <span className="quick-message-icon-wrap">
                          {message.app_icon ? (
                            <img
                              src={message.app_icon}
                              alt=""
                              className="quick-message-icon"
                              onError={(event) => {
                                event.currentTarget.style.display = "none";
                                const fallback = event.currentTarget.nextElementSibling as HTMLSpanElement | null;
                                if (fallback) fallback.style.display = "inline-flex";
                              }}
                            />
                          ) : null}
                          <span
                            className="quick-message-icon-fallback"
                            style={{ display: message.app_icon ? "none" : "inline-flex" }}
                          >
                            {initials(message.app || "App")}
                          </span>
                        </span>
                      ) : null}
                      <strong className="message-title">{message.title || "(No title)"}</strong>
                    </div>
                    <span className="message-time">{message.formatted_time}</span>
                  </div>
                  <div className="message-row-meta">
                    <span>{message.app || "Unknown app"}</span>
                    <span
                      className="priority-pill"
                      style={{
                        borderColor: color,
                        background: color,
                        color: textColor,
                      }}
                    >
                      P{message.priority}
                    </span>
                  </div>
                  <div
                    className="markdown-body list-message-body"
                    dangerouslySetInnerHTML={{
                      __html: message.rendered_html,
                    }}
                  />
                  {preview ? (
                    <a className="preview-card" href={preview.url} target="_blank" rel="noreferrer">
                      {preview.image ? <img src={preview.image} alt="" className="preview-image" /> : null}
                      <div className="preview-content">
                        <div className="preview-site">{preview.site_name || new URL(preview.url).host}</div>
                        <div className="preview-title">{preview.title || preview.url}</div>
                        {preview.description ? <div className="preview-desc">{preview.description}</div> : null}
                      </div>
                    </a>
                  ) : null}
                  <div className="message-row-actions">
                    <button
                      type="button"
                      className="danger-button subtle icon-button"
                      aria-label="Delete message"
                      title="Delete message"
                      onClick={(event) => {
                        event.stopPropagation();
                        void onDeleteMessage(message.id);
                      }}
                    >
                      <TrashIcon className={deletingMessageIds[message.id] ? "trash-icon spinning" : "trash-icon"} />
                    </button>
                  </div>
                </li>
              );
            })}
            {isWindowed && bottomSpacerPx > 0 ? (
              <li aria-hidden="true" className="message-spacer" style={{ height: `${bottomSpacerPx}px` }} />
            ) : null}
          </ul>
        )}
      </section>
    </section>
  );
}

function resolvePriorityColor(priority: number, thresholds: PriorityThreshold[], themeBadgeColor: string): string {
  if (thresholds.length === 0) return "#6B8DB6";
  let current = thresholds[0];
  for (const threshold of thresholds) {
    if (priority >= threshold.value) {
      current = threshold;
    } else {
      break;
    }
  }
  if (current.color === "__THEME_BADGE__") return themeBadgeColor;
  return /^#[0-9a-fA-F]{6}$/.test(current.color) ? current.color : "#6B8DB6";
}

function pickForegroundColor(hexColor: string): string {
  const hex = hexColor.trim();
  const normalized = /^#[0-9a-fA-F]{6}$/.test(hex) ? hex.slice(1) : "6B8DB6";
  const red = parseInt(normalized.slice(0, 2), 16);
  const green = parseInt(normalized.slice(2, 4), 16);
  const blue = parseInt(normalized.slice(4, 6), 16);
  const brightness = (red * 299 + green * 587 + blue * 114) / 1000;
  return brightness > 150 ? "#1f2937" : "#f8fafc";
}

function getThemeBadgeColor(): string {
  if (typeof window === "undefined") return "#6B8DB6";
  const raw = getComputedStyle(document.documentElement).getPropertyValue("--badge-bg").trim();
  const rgbMatch = raw.match(/^rgba?\((\d+),\s*(\d+),\s*(\d+)/i);
  if (rgbMatch) {
    const toHex = (v: string) => Number(v).toString(16).padStart(2, "0");
    return `#${toHex(rgbMatch[1])}${toHex(rgbMatch[2])}${toHex(rgbMatch[3])}`.toUpperCase();
  }
  if (/^#[0-9a-fA-F]{6}$/.test(raw)) {
    return raw.toUpperCase();
  }
  return "#6B8DB6";
}

function TrashIcon({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 24 24" aria-hidden="true">
      <path d="M3 6h18" />
      <path d="M8 6V4h8v2" />
      <path d="M7 6l1 14h8l1-14" />
      <path d="M10 10v7" />
      <path d="M14 10v7" />
    </svg>
  );
}
