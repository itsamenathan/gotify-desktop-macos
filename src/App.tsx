import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import DOMPurify from "dompurify";
import { marked } from "marked";
import gotifyLogo from "./assets/gotify-logo.png";

type ConnectionState = "Connected" | "Disconnected" | "Connecting" | "Backoff";

type SettingsResponse = {
  base_url: string;
  has_token: boolean;
  min_priority: number;
  cache_limit: number;
  launch_at_login: boolean;
  start_minimized_to_tray: boolean;
  pause_until: number | null;
  pause_mode: string | null;
  quiet_hours_start: number | null;
  quiet_hours_end: number | null;
};

type PauseStateResponse = {
  pause_until: number | null;
  pause_mode: string | null;
};

type GotifyMessage = {
  id: number;
  app_id: number;
  title: string;
  message: string;
  priority: number;
  app: string;
  app_icon: string | null;
  date: string;
};

type UiMessage = GotifyMessage & {
  rendered_html: string;
  primary_url: string | null;
  parsed_ts: number | null;
  formatted_time: string;
};

type RuntimeDiagnostics = {
  connection_state: ConnectionState;
  should_run: boolean;
  last_connected_at: number | null;
  last_stream_event_at: number | null;
  last_message_at: number | null;
  last_message_id: number | null;
  stale_for_seconds: number | null;
  last_error: string | null;
  backoff_seconds: number;
  reconnect_attempts: number;
};

type AppGroup = {
  key: string;
  name: string;
  count: number;
  icon: string | null;
};

type UrlPreview = {
  url: string;
  title: string | null;
  description: string | null;
  site_name: string | null;
  image: string | null;
};

type ThemePreference = "system" | "light" | "dark" | "dracula";
type DrawerTab = "settings" | "diagnostics";
type PauseMode = "15m" | "1h" | "custom" | "forever";
type SelectionHistoryState = {
  tag: "gotify-selection-v1";
  app: string;
  messageId: number | null;
};

const THEME_STORAGE_KEY = "gotify-theme-preference";
const ENABLE_MESSAGE_DEBUG = import.meta.env.DEV;
const PAUSE_FOREVER_SENTINEL = 0;
const WINDOWING_THRESHOLD = 100;
const WINDOW_DEFAULT_ROW_HEIGHT = 260;
const WINDOW_MIN_ROW_HEIGHT = 120;
const WINDOW_MAX_ROW_HEIGHT = 520;
const WINDOW_OVERSCAN = 8;

function debugUi(event: string, payload: Record<string, unknown>): void {
  if (!ENABLE_MESSAGE_DEBUG) return;
  console.debug(`[gotify-ui] ${event}`, payload);
}

function loadThemePreference(): ThemePreference {
  if (typeof window === "undefined") return "system";
  const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
  return stored === "light" || stored === "dark" || stored === "system" || stored === "dracula" ? stored : "system";
}

function initials(name: string): string {
  const parts = name
    .split(/\s+/)
    .map((part) => part.trim())
    .filter(Boolean)
    .slice(0, 2);
  if (parts.length === 0) return "?";
  return parts.map((part) => part[0]?.toUpperCase() ?? "").join("");
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

function GearIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" style={{ width: 17, height: 17, fill: "none", stroke: "currentColor", strokeWidth: 2, strokeLinecap: "round", strokeLinejoin: "round" }}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

function normalizeSelectionMessageId(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function normalizePauseMode(value: unknown): PauseMode | null {
  if (value === "15m" || value === "1h" || value === "custom" || value === "forever") {
    return value;
  }
  return null;
}

function isSelectionHistoryState(value: unknown): value is SelectionHistoryState {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<SelectionHistoryState>;
  return (
    candidate.tag === "gotify-selection-v1" &&
    typeof candidate.app === "string" &&
    (typeof candidate.messageId === "number" || candidate.messageId === null)
  );
}

export function App() {
  const [connectionState, setConnectionState] = useState<ConnectionState>("Disconnected");
  const [baseUrl, setBaseUrl] = useState("");
  const [token, setToken] = useState("");
  const [minPriority, setMinPriority] = useState(0);
  const [cacheLimit, setCacheLimit] = useState(100);
  const [launchAtLogin, setLaunchAtLogin] = useState(true);
  const [startMinimizedToTray, setStartMinimizedToTray] = useState(true);
  const [quietStart, setQuietStart] = useState("");
  const [quietEnd, setQuietEnd] = useState("");
  const [hasStoredToken, setHasStoredToken] = useState(false);
  const [pauseUntil, setPauseUntil] = useState<number | null>(null);
  const [pauseMode, setPauseMode] = useState<PauseMode | null>(null);
  const [clockSec, setClockSec] = useState<number>(() => Math.floor(Date.now() / 1000));
  const [isQuickWindow, setIsQuickWindow] = useState(false);

  const [messages, setMessages] = useState<UiMessage[]>([]);
  const [selectedMessageId, setSelectedMessageId] = useState<number | null>(null);
  const [selectedApp, setSelectedApp] = useState<string>("all");

  const [drawerTab, setDrawerTab] = useState<DrawerTab | null>(null);

  const [isLoading, setIsLoading] = useState(true);
  const [isSaving, setIsSaving] = useState(false);
  const [isTesting, setIsTesting] = useState(false);
  const [feedback, setFeedback] = useState<{ kind: "ok" | "error"; message: string } | null>(null);
  const [diagnostics, setDiagnostics] = useState<RuntimeDiagnostics | null>(null);
  const [deletingMessageIds, setDeletingMessageIds] = useState<Record<string, boolean>>({});
  const [urlPreviews, setUrlPreviews] = useState<Record<string, UrlPreview | null>>({});
  const [themePreference, setThemePreference] = useState<ThemePreference>(() => loadThemePreference());
  // JS object keys are always strings at runtime, so use Record<string, boolean>
  // to match the actual key type (message IDs coerce to string on assignment).

  const [pauseMenuOpen, setPauseMenuOpen] = useState(false);
  const lastRecoveryAttemptRef = useRef(0);
  const cacheLimitRef = useRef(cacheLimit);
  const messageListRef = useRef<HTMLUListElement | null>(null);
  const estimatedRowHeightRef = useRef(WINDOW_DEFAULT_ROW_HEIGHT);
  const pendingScrollMessageIdRef = useRef<number | null>(null);
  const pauseMenuRef = useRef<HTMLDivElement | null>(null);
  const [windowRange, setWindowRange] = useState({ start: 0, end: 0 });

  const applyPauseState = (pauseUntilValue: number | null, pauseModeValue: string | null) => {
    const normalizedMode = normalizePauseMode(pauseModeValue);
    setPauseUntil((current) => (current === pauseUntilValue ? current : pauseUntilValue));
    setPauseMode((current) => (current === normalizedMode ? current : normalizedMode));
  };

  useEffect(() => {
    cacheLimitRef.current = Math.max(1, cacheLimit);
  }, [cacheLimit]);

  useEffect(() => {
    try {
      setIsQuickWindow(getCurrentWebviewWindow().label === "quick");
    } catch {
      setIsQuickWindow(false);
    }
  }, []);

  useEffect(() => {
    const timer = window.setInterval(() => {
      setClockSec(Math.floor(Date.now() / 1000));
    }, 1000);
    return () => {
      window.clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    if (!pauseMenuOpen) return;
    const onMouseDown = (event: MouseEvent) => {
      const root = pauseMenuRef.current;
      if (!root) return;
      if (!root.contains(event.target as Node)) {
        setPauseMenuOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setPauseMenuOpen(false);
      }
    };
    window.addEventListener("mousedown", onMouseDown);
    window.addEventListener("keydown", onKeyDown);
    return () => {
      window.removeEventListener("mousedown", onMouseDown);
      window.removeEventListener("keydown", onKeyDown);
    };
  }, [pauseMenuOpen]);

  useEffect(() => {
    if (!isQuickWindow) return;
    const currentWindow = getCurrentWebviewWindow();
    const hideQuick = () => {
      void currentWindow.hide();
    };
    const onBlur = () => {
      hideQuick();
    };
    const onVisibility = () => {
      if (document.hidden) {
        hideQuick();
      }
    };
    window.addEventListener("blur", onBlur);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      window.removeEventListener("blur", onBlur);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [isQuickWindow]);

  useEffect(() => {
    const onDocumentClick = (event: MouseEvent) => {
      const target = event.target as Element | null;
      const anchor = target?.closest("a[href]") as HTMLAnchorElement | null;
      if (!anchor) return;

      const href = anchor.getAttribute("href")?.trim();
      if (!href) return;
      if (!/^(https?:|mailto:)/i.test(href)) return;

      event.preventDefault();
      event.stopPropagation();
      void invoke("open_external_url", { url: href }).catch((error) => {
        setFeedback({ kind: "error", message: String(error) });
      });
    };
    document.addEventListener("click", onDocumentClick, true);
    return () => {
      document.removeEventListener("click", onDocumentClick, true);
    };
  }, []);

  const applySelection = (appKey: string, messageId: number | null, pushHistory: boolean) => {
    const normalizedApp = appKey.trim().length > 0 ? appKey : "all";
    const normalizedMessageId = normalizeSelectionMessageId(messageId);
    pendingScrollMessageIdRef.current = normalizedMessageId;
    setSelectedApp(normalizedApp);
    setSelectedMessageId(normalizedMessageId);

    if (!pushHistory || typeof window === "undefined") return;
    const current = window.history.state;
    if (
      isSelectionHistoryState(current) &&
      current.app === normalizedApp &&
      normalizeSelectionMessageId(current.messageId) === normalizedMessageId
    ) {
      return;
    }
    const nextState: SelectionHistoryState = {
      tag: "gotify-selection-v1",
      app: normalizedApp,
      messageId: normalizedMessageId,
    };
    window.history.pushState(nextState, "");
  };

  useEffect(() => {
    if (typeof window === "undefined") return;
    const current = window.history.state;
    if (isSelectionHistoryState(current)) {
      setSelectedApp(current.app || "all");
      setSelectedMessageId(normalizeSelectionMessageId(current.messageId));
      return;
    }
    const initialState: SelectionHistoryState = {
      tag: "gotify-selection-v1",
      app: "all",
      messageId: null,
    };
    window.history.replaceState(initialState, "");
  }, []);

  useEffect(() => {
    if (typeof window === "undefined") return;
    const onPopState = (event: PopStateEvent) => {
      if (!isSelectionHistoryState(event.state)) return;
      setSelectedApp(event.state.app || "all");
      setSelectedMessageId(normalizeSelectionMessageId(event.state.messageId));
    };
    window.addEventListener("popstate", onPopState);
    return () => {
      window.removeEventListener("popstate", onPopState);
    };
  }, []);

  useEffect(() => {
    if (!isQuickWindow) return;
    if (selectedApp !== "all") {
      setSelectedApp("all");
    }
  }, [isQuickWindow, selectedApp]);

  useEffect(() => {
    const root = document.documentElement;
    const body = document.body;
    if (isQuickWindow) {
      root.classList.add("quick-mode");
      body.classList.add("quick-mode");
    } else {
      root.classList.remove("quick-mode");
      body.classList.remove("quick-mode");
    }
    return () => {
      root.classList.remove("quick-mode");
      body.classList.remove("quick-mode");
    };
  }, [isQuickWindow]);

  useEffect(() => {
    // `destroyed` is set to true synchronously in the cleanup function.
    // Each .then() checks it before storing the unlisten handle: if the component
    // already unmounted while the listen() promise was in-flight, we call the
    // returned unlisten function immediately so the IPC subscription is released.
    let destroyed = false;
    let unlistenConnection: (() => void) | undefined;
    let unlistenMessage: (() => void) | undefined;
    let unlistenNotification: (() => void) | undefined;
    let unlistenNotificationClicked: (() => void) | undefined;
    let unlistenError: (() => void) | undefined;
    let unlistenDiagnostics: (() => void) | undefined;
    let unlistenMessagesSynced: (() => void) | undefined;
    let unlistenMessagesUpdated: (() => void) | undefined;
    let unlistenNotificationsPaused: (() => void) | undefined;
    let unlistenNotificationsResumed: (() => void) | undefined;
    let unlistenPauseState: (() => void) | undefined;
    let unlistenPauseMode: (() => void) | undefined;

    listen<ConnectionState>("connection-state", (event) => {
      setConnectionState(event.payload);
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenConnection = fn;
    });

    listen<GotifyMessage>("message-received", (event) => {
      debugUi("message-received handled", { id: event.payload.id, at: Date.now() });
      const incoming = toUiMessage(event.payload);
      setMessages((current) => {
        const withoutExisting = current.filter((item) => item.id !== incoming.id);
        return [incoming, ...withoutExisting].slice(0, cacheLimitRef.current);
      });
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenMessage = fn;
    });

    listen<GotifyMessage>("notification-message", (event) => {
      applySelection(String(event.payload.app_id || "all"), event.payload.id, false);
      setFeedback({ kind: "ok", message: `Notification: ${event.payload.title || "New message"}` });
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenNotification = fn;
    });

    listen<GotifyMessage>("notification-clicked", (event) => {
      applySelection(String(event.payload.app_id || "all"), event.payload.id, false);
      setFeedback({ kind: "ok", message: `Opened from notification: ${event.payload.title || "New message"}` });
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenNotificationClicked = fn;
    });

    listen<string>("connection-error", (event) => {
      setFeedback({ kind: "error", message: event.payload });
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenError = fn;
    });

    listen<RuntimeDiagnostics>("runtime-diagnostics", (event) => {
      setDiagnostics(event.payload);
      setConnectionState(event.payload.connection_state);
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenDiagnostics = fn;
    });

    listen<boolean>("messages-synced", () => {
      void invoke<GotifyMessage[]>("load_messages")
        .then((cached) => setMessages((current) => mergeUiMessages(current, cached)))
        .catch(() => {});
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenMessagesSynced = fn;
    });

    listen<GotifyMessage[]>("messages-updated", (event) => {
      setMessages((current) => mergeUiMessages(current, event.payload));
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenMessagesUpdated = fn;
    });

    listen<number>("notifications-paused-until", (event) => {
      setPauseUntil(event.payload);
      if (event.payload === PAUSE_FOREVER_SENTINEL) {
        setPauseMode("forever");
      }
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenNotificationsPaused = fn;
    });

    listen<boolean>("notifications-resumed", () => {
      setPauseUntil(null);
      setPauseMode(null);
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenNotificationsResumed = fn;
    });

    listen<number | null>("notifications-pause-state", (event) => {
      setPauseUntil(event.payload);
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenPauseState = fn;
    });

    listen<string | null>("notifications-pause-mode", (event) => {
      setPauseMode(normalizePauseMode(event.payload));
    }).then((fn) => {
      if (destroyed) { fn(); return; }
      unlistenPauseMode = fn;
    });

    const load = async () => {
      try {
        const [settings, cachedMessages, runtime] = await Promise.all([
          invoke<SettingsResponse>("load_settings"),
          invoke<GotifyMessage[]>("load_messages"),
          invoke<RuntimeDiagnostics>("get_runtime_diagnostics"),
        ]);

        setBaseUrl(settings.base_url ?? "");
        setHasStoredToken(settings.has_token);
        setMinPriority(settings.min_priority ?? 0);
        setCacheLimit(settings.cache_limit ?? 100);
        setLaunchAtLogin(settings.launch_at_login ?? true);
        setStartMinimizedToTray(settings.start_minimized_to_tray ?? true);
        setPauseUntil(settings.pause_until ?? null);
        setPauseMode(normalizePauseMode(settings.pause_mode));
        setQuietStart(settings.quiet_hours_start == null ? "" : String(settings.quiet_hours_start));
        setQuietEnd(settings.quiet_hours_end == null ? "" : String(settings.quiet_hours_end));
        setMessages((current) => mergeUiMessages(current, cachedMessages));
        setDiagnostics(runtime);
        setConnectionState(runtime.connection_state);
      } catch (error) {
        setFeedback({ kind: "error", message: String(error) });
      } finally {
        setIsLoading(false);
      }
    };

    void load();

    return () => {
      // Setting destroyed=true before calling any unlisten ensures that any
      // in-flight listen() promises that resolve after this point will also
      // immediately release their subscription rather than storing a stale handle.
      destroyed = true;
      if (unlistenConnection) unlistenConnection();
      if (unlistenMessage) unlistenMessage();
      if (unlistenNotification) unlistenNotification();
      if (unlistenNotificationClicked) unlistenNotificationClicked();
      if (unlistenError) unlistenError();
      if (unlistenDiagnostics) unlistenDiagnostics();
      if (unlistenMessagesSynced) unlistenMessagesSynced();
      if (unlistenMessagesUpdated) unlistenMessagesUpdated();
      if (unlistenNotificationsPaused) unlistenNotificationsPaused();
      if (unlistenNotificationsResumed) unlistenNotificationsResumed();
      if (unlistenPauseState) unlistenPauseState();
      if (unlistenPauseMode) unlistenPauseMode();
    };
  }, []);

  useEffect(() => {
    const previewTargets = new Set<string>();
    for (const message of messages) {
      if (message.primary_url) {
        previewTargets.add(message.primary_url);
      }
    }

    for (const url of previewTargets) {
      if (Object.prototype.hasOwnProperty.call(urlPreviews, url)) {
        continue;
      }
      void invoke<UrlPreview>("fetch_url_preview", { url })
        .then((preview) => {
          setUrlPreviews((current) => ({ ...current, [url]: preview }));
        })
        .catch(() => {
          setUrlPreviews((current) => ({ ...current, [url]: null }));
        });
    }
  // urlPreviews is intentionally omitted from deps: the hasOwnProperty guard
  // inside the loop prevents duplicate fetches, so adding urlPreviews would
  // cause an O(n²) cascade where every resolved preview triggers a re-scan.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [messages]);

  useEffect(() => {
    window.localStorage.setItem(THEME_STORAGE_KEY, themePreference);
    const root = document.documentElement;
    if (themePreference === "system") {
      root.removeAttribute("data-theme");
      return;
    }
    root.setAttribute("data-theme", themePreference);
  }, [themePreference]);

  // When the theme is changed in the main window, the quick window (a separate
  // WebView) won't see the React state update. The browser fires a `storage`
  // event in all *other* same-origin windows when localStorage is written, so
  // we listen for it here to keep the quick window's theme in sync.
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key !== THEME_STORAGE_KEY || event.newValue === null) return;
      const val = event.newValue;
      if (val === "light" || val === "dark" || val === "system" || val === "dracula") {
        setThemePreference(val);
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  useEffect(() => {
    if (!feedback || feedback.kind !== "ok") return;
    const timer = window.setTimeout(() => {
      setFeedback((current) => (current?.kind === "ok" ? null : current));
    }, 3000);
    return () => {
      window.clearTimeout(timer);
    };
  }, [feedback]);

  // Diagnostics are pushed by the backend via runtime-diagnostics events and are
  // initialised in the startup load() call above. No polling loop needed.

  // macOS WebKit throttles JavaScript callbacks (including Tauri event listeners)
  // when a window is open but does not have keyboard focus. The message-received
  // listener fires instantly when the window is active, but may be deferred when
  // the window is backgrounded. A 5 s fallback poll ensures messages appear within
  // one backend sync cycle even through that throttle, without hammering the IPC
  // channel at 600 ms like the previous loop did.
  useEffect(() => {
    let disposed = false;
    const syncMessages = async () => {
      if (document.hidden || disposed) return;
      try {
        const cached = await invoke<GotifyMessage[]>("load_messages");
        if (disposed) return;
        setMessages((current) => mergeUiMessages(current, cached));
      } catch {
        // ignore transient invoke errors
      }
    };

    void syncMessages();
    const timer = window.setInterval(() => {
      void syncMessages();
    }, 5000);

    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, []);

  useEffect(() => {
    const refreshMessages = async () => {
      try {
        const cached = await invoke<GotifyMessage[]>("load_messages");
        setMessages((current) => mergeUiMessages(current, cached));
      } catch {
        // ignore transient invoke errors
      }
    };

    const refreshPauseState = async () => {
      try {
        const pauseState = await invoke<PauseStateResponse>("get_pause_state");
        applyPauseState(pauseState.pause_until ?? null, pauseState.pause_mode ?? null);
      } catch {
        // ignore transient invoke errors
      }
    };

    const triggerRecovery = async () => {
      const now = Date.now();
      if (now - lastRecoveryAttemptRef.current < 2000) return;
      lastRecoveryAttemptRef.current = now;
      try {
        await invoke("recover_stream");
      } catch {
        // ignore recovery invoke failures
      }
    };

    const onOnline = () => void triggerRecovery();
    const onFocus = () => {
      void triggerRecovery();
      void refreshMessages();
      void refreshPauseState();
    };
    const onVisibility = () => {
      if (!document.hidden) {
        void triggerRecovery();
        void refreshMessages();
        void refreshPauseState();
      }
    };

    window.addEventListener("online", onOnline);
    window.addEventListener("focus", onFocus);
    document.addEventListener("visibilitychange", onVisibility);

    return () => {
      window.removeEventListener("online", onOnline);
      window.removeEventListener("focus", onFocus);
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, []);

  useEffect(() => {
    if (connectionState !== "Connected") return;
    void invoke<GotifyMessage[]>("load_messages")
      .then((cached) => setMessages((current) => mergeUiMessages(current, cached)))
      .catch(() => {});
  }, [connectionState]);

  // Pause state is pushed by the backend via notifications-pause-state,
  // notifications-pause-mode, notifications-paused-until, and notifications-resumed
  // events, and is refreshed on focus/visible by the recovery effect above.
  // No polling loop needed.

  const onSave = async (event: FormEvent) => {
    event.preventDefault();
    setFeedback(null);

    // Validate quiet hours before touching async state.
    const parsedQuietStart = quietStart.trim() === "" ? null : parseInt(quietStart, 10);
    const parsedQuietEnd = quietEnd.trim() === "" ? null : parseInt(quietEnd, 10);
    if (parsedQuietStart !== null && (isNaN(parsedQuietStart) || parsedQuietStart < 0 || parsedQuietStart > 23)) {
      setFeedback({ kind: "error", message: "Quiet start hour must be a number between 0 and 23" });
      return;
    }
    if (parsedQuietEnd !== null && (isNaN(parsedQuietEnd) || parsedQuietEnd < 0 || parsedQuietEnd > 23)) {
      setFeedback({ kind: "error", message: "Quiet end hour must be a number between 0 and 23" });
      return;
    }

    setIsSaving(true);

    try {
      const quietHoursStart = parsedQuietStart;
      const quietHoursEnd = parsedQuietEnd;

      await invoke("save_settings", {
        baseUrl,
        token,
        minPriority,
        cacheLimit,
        launchAtLogin,
        startMinimizedToTray,
        quietHoursStart,
        quietHoursEnd,
      });

      const refreshed = await invoke<SettingsResponse>("load_settings");
      setHasStoredToken(refreshed.has_token);
      setMinPriority(refreshed.min_priority ?? 0);
      setCacheLimit(refreshed.cache_limit ?? 100);
      setLaunchAtLogin(refreshed.launch_at_login ?? true);
      setStartMinimizedToTray(refreshed.start_minimized_to_tray ?? true);
      setPauseUntil(refreshed.pause_until ?? null);
      setPauseMode(normalizePauseMode(refreshed.pause_mode));
      setQuietStart(refreshed.quiet_hours_start == null ? "" : String(refreshed.quiet_hours_start));
      setQuietEnd(refreshed.quiet_hours_end == null ? "" : String(refreshed.quiet_hours_end));
      setToken("");
      setFeedback({ kind: "ok", message: "Settings saved. Reconnecting..." });
      setDrawerTab(null);
      // Restart stream with new credentials — fire-and-forget; result reported via events
      void invoke("restart_stream").catch(() => {});
    } catch (error) {
      setFeedback({ kind: "error", message: String(error) });
    } finally {
      setIsSaving(false);
    }
  };

  const onTest = async () => {
    setFeedback(null);
    setIsTesting(true);
    try {
      const message = await invoke<string>("test_connection", {
        baseUrl,
        token: token.trim().length > 0 ? token : null,
      });
      setFeedback({ kind: "ok", message });
    } catch (error) {
      setFeedback({ kind: "error", message: String(error) });
    } finally {
      setIsTesting(false);
    }
  };

  const onPause = async (minutes: number) => {
    setPauseMenuOpen(false);
    try {
      await invoke("pause_notifications", { minutes });
      // Pause state is updated by backend-emitted notifications-pause-* events.
    } catch (error) {
      setFeedback({ kind: "error", message: String(error) });
    }
  };

  const onPauseUntilTomorrow = async () => {
    const now = new Date();
    const tomorrow = new Date(now);
    tomorrow.setDate(now.getDate() + 1);
    tomorrow.setHours(0, 0, 0, 0);
    const ms = Math.max(60_000, tomorrow.getTime() - now.getTime());
    const minutes = Math.ceil(ms / 60_000);
    await onPause(minutes);
  };

  const onPauseForever = async () => {
    setPauseMenuOpen(false);
    try {
      await invoke("pause_notifications_forever");
      // Pause state is updated by backend-emitted notifications-pause-* events.
    } catch (error) {
      setFeedback({ kind: "error", message: String(error) });
    }
  };

  const onResumeNotifications = async () => {
    setPauseMenuOpen(false);
    try {
      await invoke("resume_notifications");
      // Pause state is cleared by backend-emitted notifications-resumed event.
    } catch (error) {
      setFeedback({ kind: "error", message: String(error) });
    }
  };

  const ANIM_MS = 700;
  const onDeleteMessage = async (messageId: number) => {
    debugUi("delete-click", { messageId, at: Date.now() });

    const snapshot = messages.find((m) => m.id === messageId);

    // Start exit animation; remove from list after it completes
    setDeletingMessageIds((current) => ({ ...current, [messageId]: true }));
    if (selectedMessageId === messageId) setSelectedMessageId(null);
    window.setTimeout(() => {
      setMessages((current) => current.filter((m) => m.id !== messageId));
      setDeletingMessageIds((current) => { const next = { ...current }; delete next[messageId]; return next; });
    }, ANIM_MS);

    try {
      await invoke("delete_message", { messageId });
    } catch (error) {
      // Server rejected the delete — restore the message
      if (snapshot) {
        setMessages((current) => mergeUiMessages(current, [snapshot]));
      }
      setDeletingMessageIds((current) => { const next = { ...current }; delete next[messageId]; return next; });
      setFeedback({ kind: "error", message: String(error) });
    }
  };

  const sortedMessages = useMemo(() => {
    const renderStart = performance.now();
    const sorted = [...messages].sort(compareMessagesNewestFirst);
    const elapsedMs = Math.round((performance.now() - renderStart) * 100) / 100;
    debugUi("list sort/render prep", {
      count: sorted.length,
      elapsedMs,
      at: Date.now(),
    });
    return sorted;
  }, [messages]);

  const appGroups = useMemo<AppGroup[]>(() => {
    const groups = new Map<string, AppGroup>();
    for (const msg of sortedMessages) {
      const key = String(msg.app_id || 0);
      const current = groups.get(key);
      if (current) {
        current.count += 1;
      } else {
        groups.set(key, {
          key,
          name: msg.app || "Unknown app",
          count: 1,
          icon: msg.app_icon ?? null,
        });
      }
    }
    // Keep insertion order from newest-first messages so apps are ordered by most recent activity.
    return Array.from(groups.values());
  }, [sortedMessages]);

  const filteredMessages = useMemo(() => {
    if (isQuickWindow) return sortedMessages;
    if (selectedApp === "all") return sortedMessages;
    return sortedMessages.filter((message) => String(message.app_id || 0) === selectedApp);
  }, [isQuickWindow, sortedMessages, selectedApp]);
  const isWindowed = filteredMessages.length > WINDOWING_THRESHOLD;

  useEffect(() => {
    if (!isWindowed) {
      setWindowRange({ start: 0, end: Math.max(0, filteredMessages.length - 1) });
      return;
    }

    const compute = () => {
      const list = messageListRef.current;
      const viewportHeight = list?.clientHeight ?? 800;
      const scrollTop = list?.scrollTop ?? 0;
      setWindowRange(
        computeWindowRange(filteredMessages.length, scrollTop, viewportHeight, estimatedRowHeightRef.current)
      );
    };

    compute();
    window.addEventListener("resize", compute);
    return () => {
      window.removeEventListener("resize", compute);
    };
  }, [filteredMessages.length, isWindowed]);

  const visibleMessages = useMemo(() => {
    if (!isWindowed) return filteredMessages;
    return filteredMessages.slice(windowRange.start, windowRange.end + 1);
  }, [filteredMessages, isWindowed, windowRange.end, windowRange.start]);
  const selectedFilteredMessage = useMemo(
    () => filteredMessages.find((message) => message.id === selectedMessageId) ?? null,
    [filteredMessages, selectedMessageId]
  );
  const activeMessage = selectedFilteredMessage ?? filteredMessages[0] ?? null;
  const topSpacerPx = isWindowed ? windowRange.start * estimatedRowHeightRef.current : 0;
  const bottomSpacerPx = isWindowed
    ? Math.max(0, filteredMessages.length - windowRange.end - 1) * estimatedRowHeightRef.current
    : 0;

  useEffect(() => {
    if (!isWindowed) return;
    const list = messageListRef.current;
    if (!list) return;
    const rows = Array.from(list.querySelectorAll<HTMLElement>(".message-item"));
    if (rows.length === 0) return;

    const heights = rows.map((row) => row.getBoundingClientRect().height).filter((height) => Number.isFinite(height) && height > 0);
    if (heights.length === 0) return;
    const avg = heights.reduce((sum, value) => sum + value, 0) / heights.length;
    const clamped = Math.round(Math.max(WINDOW_MIN_ROW_HEIGHT, Math.min(WINDOW_MAX_ROW_HEIGHT, avg)));
    const prev = estimatedRowHeightRef.current;
    if (Math.abs(clamped - prev) < 12) return;

    estimatedRowHeightRef.current = clamped;
    debugUi("window row height tuned", { prev, next: clamped, sampleCount: heights.length });
    const viewportHeight = list.clientHeight || 800;
    const scrollTop = list.scrollTop || 0;
    setWindowRange(computeWindowRange(filteredMessages.length, scrollTop, viewportHeight, clamped));
  }, [filteredMessages.length, isWindowed, visibleMessages]);

  useEffect(() => {
    if (selectedApp === "all") return;
    const pendingMessageId = pendingScrollMessageIdRef.current;
    if (pendingMessageId == null || pendingMessageId !== selectedMessageId) return;

    const list = messageListRef.current;
    if (!list) return;

    const targetIndex = filteredMessages.findIndex((message) => message.id === pendingMessageId);
    if (targetIndex < 0) return;

    const scrollToTargetRow = () => {
      const row = list.querySelector<HTMLElement>(`[data-message-id="${pendingMessageId}"]`);
      if (!row) return false;
      row.scrollIntoView({ block: "center", inline: "nearest" });
      pendingScrollMessageIdRef.current = null;
      return true;
    };

    if (isWindowed) {
      const viewportHeight = list.clientHeight || 800;
      const rowHeight = estimatedRowHeightRef.current || WINDOW_DEFAULT_ROW_HEIGHT;
      const targetScrollTop = Math.max(0, targetIndex * rowHeight - viewportHeight * 0.35);
      setWindowRange(computeWindowRange(filteredMessages.length, targetScrollTop, viewportHeight, rowHeight));
      list.scrollTop = targetScrollTop;
      requestAnimationFrame(() => {
        if (scrollToTargetRow()) return;
        requestAnimationFrame(() => {
          if (!scrollToTargetRow()) {
            pendingScrollMessageIdRef.current = null;
          }
        });
      });
      return;
    }

    requestAnimationFrame(() => {
      if (!scrollToTargetRow()) {
        pendingScrollMessageIdRef.current = null;
      }
    });
  }, [filteredMessages, isWindowed, selectedApp, selectedMessageId]);

  const selectedAppName =
    isQuickWindow || selectedApp === "all"
      ? "All Messages"
      : appGroups.find((group) => group.key === selectedApp)?.name ?? selectedApp;
  const pauseIsForever = pauseMode === "forever" || pauseUntil === PAUSE_FOREVER_SENTINEL;
  const pauseRemainingSec =
    pauseUntil == null || pauseIsForever ? 0 : Math.max(0, pauseUntil - clockSec);
  const pauseIsActive = pauseIsForever || pauseRemainingSec > 0;
  const pauseControlLabel = pauseIsForever
    ? "Paused · Forever"
    : pauseIsActive
      ? `Paused · ${formatPauseDuration(pauseRemainingSec)} left`
      : "Notifications On";

  return (
    <main className={isQuickWindow ? "app-shell quick-shell" : "app-shell"}>
      <section className="dashboard">
        {!isQuickWindow ? (
          <header className="topbar">
            <div className="topbar-brand">
              <img src={gotifyLogo} alt="" className="brand-logo" />
              <div className="brand-copy">
                <h1>Gotify Desktop</h1>
              </div>
            </div>
            <div className="topbar-actions">
              <div className="topbar-connection">
                <div className="pause-dropdown" ref={pauseMenuRef}>
                  <button
                    type="button"
                    className={pauseIsActive ? "pause-control paused" : "pause-control"}
                    onClick={() => setPauseMenuOpen((current) => !current)}
                    aria-haspopup="menu"
                    aria-expanded={pauseMenuOpen}
                  >
                    <span>{pauseControlLabel}</span>
                    <span className="pause-caret">▾</span>
                  </button>
                  {pauseMenuOpen ? (
                    <div className="pause-menu" role="menu" aria-label="Pause notifications">
                      <button type="button" role="menuitem" onClick={() => void onPause(15)}>
                        {pauseIsActive && pauseMode === "15m" ? "Pause 15 minutes ✓" : "Pause 15 minutes"}
                      </button>
                      <button type="button" role="menuitem" onClick={() => void onPause(60)}>
                        {pauseIsActive && pauseMode === "1h" ? "Pause 1 hour ✓" : "Pause 1 hour"}
                      </button>
                      <button type="button" role="menuitem" onClick={() => void onPauseUntilTomorrow()}>
                        {pauseIsActive && pauseMode === "custom" ? "Pause until tomorrow ✓" : "Pause until tomorrow"}
                      </button>
                      <button type="button" role="menuitem" onClick={() => void onPauseForever()}>
                        {pauseIsForever ? "Pause forever ✓" : "Pause forever"}
                      </button>
                      {pauseIsActive ? (
                        <>
                          <div className="pause-menu-separator" />
                          <button type="button" role="menuitem" className="pause-menu-resume" onClick={() => void onResumeNotifications()}>
                            Resume now
                          </button>
                        </>
                      ) : null}
                    </div>
                  ) : null}
                </div>
              </div>
              <div className="topbar-utility">
                <button
                  type="button"
                  className={drawerTab ? "utility-button icon-button active" : "utility-button icon-button"}
                  aria-label="Settings"
                  title="Settings"
                  onClick={() => setDrawerTab((current) => (current ? null : "settings"))}
                >
                  <GearIcon />
                </button>
              </div>
            </div>
          </header>
        ) : null}

        {feedback ? <div className={feedback.kind === "ok" ? "feedback ok" : "feedback error"}>{feedback.message}</div> : null}

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
                        <span className="priority-pill">P{message.priority}</span>
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
        {!isQuickWindow ? (
          <>
            <button
              type="button"
              aria-label="Close panel"
              className={drawerTab ? "drawer-backdrop open" : "drawer-backdrop"}
              onClick={() => setDrawerTab(null)}
            />
            <aside className={drawerTab ? "drawer-panel open" : "drawer-panel"} aria-hidden={drawerTab == null}>
          <div className="drawer-head">
            <h2>{drawerTab === "diagnostics" ? "Diagnostics" : "Settings"}</h2>
            <button type="button" className="utility-button" onClick={() => setDrawerTab(null)}>
              Close
            </button>
          </div>
          <div className="drawer-tabs">
            <button
              type="button"
              className={drawerTab === "settings" ? "drawer-tab active" : "drawer-tab"}
              onClick={() => setDrawerTab("settings")}
            >
              Settings
            </button>
            <button
              type="button"
              className={drawerTab === "diagnostics" ? "drawer-tab active" : "drawer-tab"}
              onClick={() => setDrawerTab("diagnostics")}
            >
              Diagnostics
            </button>
          </div>

          <div className="drawer-body">
            {drawerTab === "settings" ? (
              <form className="settings-form" onSubmit={onSave}>

                <div className="settings-group">
                  <p className="settings-group-title">Connection</p>
                  <div className="settings-card">
                    <label className="settings-field">
                      <span className="settings-label">Server URL</span>
                      <input
                        type="url"
                        value={baseUrl}
                        onChange={(event) => setBaseUrl(event.target.value)}
                        placeholder="https://gotify.example.com"
                        required
                        disabled={isLoading || isSaving || isTesting}
                      />
                    </label>
                    <label className="settings-field">
                      <span className="settings-label">Client token</span>
                      <span className="settings-hint">Found in Gotify → Clients</span>
                      <input
                        type="password"
                        value={token}
                        onChange={(event) => setToken(event.target.value)}
                        placeholder={hasStoredToken ? "Leave blank to keep existing" : "Enter Gotify client token"}
                        required={!hasStoredToken}
                        autoComplete="off"
                        disabled={isLoading || isSaving || isTesting}
                      />
                    </label>
                  </div>
                </div>

                <div className="settings-group">
                  <p className="settings-group-title">Notifications</p>
                  <div className="settings-card">
                    <label className="settings-field">
                      <span className="settings-label">Minimum priority</span>
                      <span className="settings-hint">Only notify for messages at this priority or above (0–10)</span>
                      <input
                        type="number"
                        min={0}
                        max={10}
                        value={minPriority}
                        onChange={(event) => setMinPriority(Number(event.target.value || 0))}
                        disabled={isLoading || isSaving || isTesting}
                      />
                    </label>
                    <div className="settings-field">
                      <span className="settings-label">Quiet hours</span>
                      <span className="settings-hint">Suppress notifications between these hours (24-hour clock)</span>
                      <div className="settings-two-col">
                        <label>
                          <span className="settings-sublabel">From</span>
                          <input
                            type="number"
                            min={0}
                            max={23}
                            value={quietStart}
                            onChange={(event) => setQuietStart(event.target.value)}
                            placeholder="22"
                            disabled={isLoading || isSaving || isTesting}
                          />
                        </label>
                        <label>
                          <span className="settings-sublabel">Until</span>
                          <input
                            type="number"
                            min={0}
                            max={23}
                            value={quietEnd}
                            onChange={(event) => setQuietEnd(event.target.value)}
                            placeholder="7"
                            disabled={isLoading || isSaving || isTesting}
                          />
                        </label>
                      </div>
                    </div>
                  </div>
                </div>

                <div className="settings-group">
                  <p className="settings-group-title">Behaviour</p>
                  <div className="settings-card">
                    <label className="settings-field">
                      <span className="settings-label">Message cache size</span>
                      <span className="settings-hint">Maximum messages stored locally (1–2000)</span>
                      <input
                        type="number"
                        min={1}
                        max={2000}
                        value={cacheLimit}
                        onChange={(event) => setCacheLimit(Number(event.target.value || 100))}
                        disabled={isLoading || isSaving || isTesting}
                      />
                    </label>
                    <label className="settings-toggle">
                      <span className="settings-label">Launch at login</span>
                      <input
                        type="checkbox"
                        checked={launchAtLogin}
                        onChange={(event) => setLaunchAtLogin(event.target.checked)}
                        disabled={isLoading || isSaving || isTesting}
                      />
                    </label>
                    <label className="settings-toggle">
                      <span className="settings-label">Start minimized to tray</span>
                      <input
                        type="checkbox"
                        checked={startMinimizedToTray}
                        onChange={(event) => setStartMinimizedToTray(event.target.checked)}
                        disabled={isLoading || isSaving || isTesting}
                      />
                    </label>
                  </div>
                </div>

                <div className="settings-group">
                  <p className="settings-group-title">Appearance</p>
                  <div className="settings-card">
                    <label className="settings-field">
                      <span className="settings-label">Theme</span>
                      <select
                        value={themePreference}
                        onChange={(event) => setThemePreference(event.target.value as ThemePreference)}
                        disabled={isLoading || isSaving || isTesting}
                      >
                        <option value="system">System (follows macOS)</option>
                        <option value="light">Light</option>
                        <option value="dark">Dark</option>
                        <option value="dracula">Dracula</option>
                      </select>
                    </label>
                  </div>
                </div>

                <div className="settings-actions">
                  <button type="submit" disabled={isLoading || isSaving || isTesting}>
                    {isSaving ? "Saving..." : "Save Settings"}
                  </button>
                  <button type="button" onClick={onTest} disabled={isLoading || isSaving || isTesting}>
                    {isTesting ? "Testing..." : "Test Connection"}
                  </button>
                </div>

                {feedback ? (
                  <div className={feedback.kind === "ok" ? "feedback ok" : "feedback error"}>
                    {feedback.message}
                  </div>
                ) : null}

              </form>
            ) : null}

            {drawerTab === "diagnostics" ? (
              <>
                <div className="diagnostics">
                  <div><span>Version:</span> <strong>v{__APP_VERSION__}</strong></div>
                  <div><span>Server:</span> <strong>{baseUrl || "—"}</strong></div>
                  <div><span>Connection:</span> <strong>{diagnostics?.connection_state ?? connectionState}</strong></div>
                  <div><span>Cached messages:</span> <strong>{messages.length}</strong></div>
                  <div>
                    <span>Last connected:</span>{" "}
                    <strong>
                      {diagnostics?.last_connected_at
                        ? new Date(diagnostics.last_connected_at * 1000).toLocaleString()
                        : "Never"}
                    </strong>
                  </div>
                  <div>
                    <span>Last stream event:</span>{" "}
                    <strong>
                      {diagnostics?.last_stream_event_at
                        ? new Date(diagnostics.last_stream_event_at * 1000).toLocaleString()
                        : "Never"}
                    </strong>
                  </div>
                  <div><span>Stream idle:</span> <strong>{diagnostics?.stale_for_seconds ?? 0}s</strong></div>
                  <div><span>Reconnect attempts:</span> <strong>{diagnostics?.reconnect_attempts ?? 0}</strong></div>
                  {(diagnostics?.backoff_seconds ?? 0) > 0 ? (
                    <div><span>Backoff:</span> <strong>{diagnostics!.backoff_seconds}s</strong></div>
                  ) : null}
                  <div>
                    <span>Last message:</span>{" "}
                    <strong>
                      {diagnostics?.last_message_at
                        ? `${new Date(diagnostics.last_message_at * 1000).toLocaleString()} (id ${diagnostics.last_message_id ?? "?"})`
                        : "Never"}
                    </strong>
                  </div>
                  {diagnostics?.last_error ? (
                    <div>
                      <span>Last error:</span>{" "}
                      <strong style={{ color: "var(--pill-disconnected-color)" }}>{diagnostics.last_error}</strong>
                    </div>
                  ) : null}
                </div>
                <div className="actions" style={{ marginTop: 12 }}>
                  <button
                    type="button"
                    className="utility-button"
                    onClick={() => { void invoke("restart_stream").catch(() => {}); }}
                  >
                    Force Reconnect
                  </button>
                </div>
              </>
            ) : null}
          </div>
            </aside>
          </>
        ) : null}
      </section>
    </main>
  );
}

function toUiMessage(message: GotifyMessage): UiMessage {
  // async: false makes the return type `string` rather than `string | Promise<string>`,
  // avoiding the silent `[object Promise]` bug if marked ever defaults to async mode.
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

function mergeUiMessages(current: UiMessage[], incoming: GotifyMessage[]): UiMessage[] {
  // Return current untouched when the backend sends nothing — an empty response
  // is a transient condition (startup race, cache miss) and should not wipe the UI.
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
  return Array.from(
    new Set(
      matches.map((url) => url.replace(/[.,!?;:]+$/g, ""))
    )
  );
}

function formatPauseDuration(totalSeconds: number): string {
  const seconds = Math.max(0, Math.floor(totalSeconds));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  const remMinutes = minutes % 60;
  if (remMinutes === 0) return `${hours}h`;
  return `${hours}h ${remMinutes}m`;
}

function compareMessagesNewestFirst(a: UiMessage, b: UiMessage): number {
  const ta = a.parsed_ts;
  const tb = b.parsed_ts;
  if (tb != null && ta != null && tb !== ta) {
    return tb - ta;
  }
  if (tb != null && ta == null) return -1;
  if (ta != null && tb == null) return 1;
  return b.id - a.id;
}

function computeWindowRange(
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
