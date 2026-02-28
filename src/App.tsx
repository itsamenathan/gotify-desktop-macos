import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import gotifyLogo from "./assets/gotify-logo.png";
import { DiagnosticsPanel } from "./components/DiagnosticsPanel";
import { MessageFeed } from "./components/MessageFeed";
import { SettingsForm } from "./components/SettingsForm";
import type {
  AppGroup,
  ConnectionState,
  DrawerTab,
  GotifyMessage,
  PauseMode,
  PauseStateResponse,
  RuntimeDiagnostics,
  SelectionHistoryState,
  SettingsResponse,
  ThemePreference,
  UiMessage,
  UrlPreview,
} from "./types";
import { debugUi } from "./utils/debug";
import { compareMessagesNewestFirst, mergeUiMessages, toUiMessage } from "./utils/messages";
import { normalizePauseMode, normalizeSelectionMessageId, isSelectionHistoryState } from "./utils/selection";
import { formatPauseDuration } from "./utils/time";
import {
  computeWindowRange,
  WINDOW_DEFAULT_ROW_HEIGHT,
  WINDOW_MAX_ROW_HEIGHT,
  WINDOW_MIN_ROW_HEIGHT,
  WINDOWING_THRESHOLD,
} from "./utils/windowing";

const THEME_STORAGE_KEY = "gotify-theme-preference";
const PAUSE_FOREVER_SENTINEL = 0;

function loadThemePreference(): ThemePreference {
  if (typeof window === "undefined") return "system";
  const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
  return stored === "light" || stored === "dark" || stored === "system" || stored === "dracula" ? stored : "system";
}

function GearIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" style={{ width: 17, height: 17, fill: "none", stroke: "currentColor", strokeWidth: 2, strokeLinecap: "round", strokeLinejoin: "round" }}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

export function App() {
  const [connectionState, setConnectionState] = useState<ConnectionState>("Disconnected");
  const [baseUrl, setBaseUrl] = useState("");
  const [token, setToken] = useState("");
  const [minPriority, setMinPriority] = useState(0);
  const [cacheLimit, setCacheLimit] = useState(100);
  const [launchAtLogin, setLaunchAtLogin] = useState(false);
  const [startMinimizedToTray, setStartMinimizedToTray] = useState(false);
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
      setDiagnostics((current) => {
        if (!current) return current;
        const nowSec = Math.floor(Date.now() / 1000);
        return {
          ...current,
          last_stream_event_at: nowSec,
          last_message_at: nowSec,
          last_message_id: event.payload.id,
          stale_for_seconds: 0,
        };
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
        setLaunchAtLogin(settings.launch_at_login ?? false);
        setStartMinimizedToTray(settings.start_minimized_to_tray ?? false);
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

  // Keep diagnostics fresh while the diagnostics panel is open. Some macOS/WebKit
  // focus/throttle states can delay event delivery, so this lightweight poll keeps
  // counters (like stream idle) accurate in real time when the user is actively
  // viewing diagnostics.
  useEffect(() => {
    if (drawerTab !== "diagnostics") return;
    let disposed = false;

    const refreshDiagnostics = async () => {
      try {
        const runtime = await invoke<RuntimeDiagnostics>("get_runtime_diagnostics");
        if (disposed) return;
        setDiagnostics(runtime);
        setConnectionState(runtime.connection_state);
      } catch {
        // ignore transient invoke errors
      }
    };

    void refreshDiagnostics();
    const timer = window.setInterval(() => {
      void refreshDiagnostics();
    }, 1000);

    return () => {
      disposed = true;
      window.clearInterval(timer);
    };
  }, [drawerTab]);

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
      setLaunchAtLogin(refreshed.launch_at_login ?? false);
      setStartMinimizedToTray(refreshed.start_minimized_to_tray ?? false);
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
  const streamIdleSeconds = useMemo(() => {
    if (diagnostics?.last_stream_event_at != null) {
      return Math.max(0, clockSec - diagnostics.last_stream_event_at);
    }
    if (typeof diagnostics?.stale_for_seconds === "number" && Number.isFinite(diagnostics.stale_for_seconds)) {
      return Math.max(0, Math.floor(diagnostics.stale_for_seconds));
    }
    return 0;
  }, [clockSec, diagnostics]);

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

        <MessageFeed
          isQuickWindow={isQuickWindow}
          selectedApp={selectedApp}
          selectedAppName={selectedAppName}
          appGroups={appGroups}
          filteredMessages={filteredMessages}
          visibleMessages={visibleMessages}
          activeMessage={activeMessage}
          isWindowed={isWindowed}
          topSpacerPx={topSpacerPx}
          bottomSpacerPx={bottomSpacerPx}
          estimatedRowHeightRef={estimatedRowHeightRef}
          messageListRef={messageListRef}
          deletingMessageIds={deletingMessageIds}
          urlPreviews={urlPreviews}
          applySelection={applySelection}
          setSelectedMessageId={setSelectedMessageId}
          setWindowRange={setWindowRange}
          onDeleteMessage={onDeleteMessage}
        />
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
              <SettingsForm
                baseUrl={baseUrl}
                token={token}
                hasStoredToken={hasStoredToken}
                minPriority={minPriority}
                quietStart={quietStart}
                quietEnd={quietEnd}
                cacheLimit={cacheLimit}
                launchAtLogin={launchAtLogin}
                startMinimizedToTray={startMinimizedToTray}
                themePreference={themePreference}
                isLoading={isLoading}
                isSaving={isSaving}
                isTesting={isTesting}
                feedback={feedback}
                onSave={onSave}
                onTest={onTest}
                setBaseUrl={setBaseUrl}
                setToken={setToken}
                setMinPriority={setMinPriority}
                setQuietStart={setQuietStart}
                setQuietEnd={setQuietEnd}
                setCacheLimit={setCacheLimit}
                setLaunchAtLogin={setLaunchAtLogin}
                setStartMinimizedToTray={setStartMinimizedToTray}
                setThemePreference={setThemePreference}
              />
            ) : null}

            {drawerTab === "diagnostics" ? (
              <DiagnosticsPanel
                appVersion={__APP_VERSION__}
                baseUrl={baseUrl}
                connectionState={connectionState}
                diagnostics={diagnostics}
                messageCount={messages.length}
                streamIdleSeconds={streamIdleSeconds}
                onForceReconnect={() => {
                  void invoke("restart_stream").catch(() => {});
                }}
              />
            ) : null}
          </div>
            </aside>
          </>
        ) : null}
      </section>
    </main>
  );
}
