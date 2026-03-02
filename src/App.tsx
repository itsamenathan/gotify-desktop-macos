import { FormEvent, useEffect, useMemo, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import gotifyLogo from "./assets/gotify-logo.png";
import { DiagnosticsPanel } from "./components/DiagnosticsPanel";
import { MessageFeed } from "./components/MessageFeed";
import { SettingsForm } from "./components/SettingsForm";
import type {
  AppUpdate,
  AppGroup,
  BootstrapState,
  ConnectionState,
  DomainSnapshot,
  DrawerTab,
  GotifyMessage,
  MessageRemovedData,
  PauseStateData,
  PriorityThreshold,
  PauseMode,
  RuntimeDiagnostics,
  SelectionHistoryState,
  SettingsResponse,
  StreamErrorData,
  ThemePreference,
  UiMessage,
  UrlPreview,
} from "./types";
import { debugUi } from "./utils/debug";
import { compareMessagesNewestFirst, toUiMessage } from "./utils/messages";
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
const THEME_BADGE_SENTINEL = "__THEME_BADGE__";
const DEFAULT_PRIORITY_THRESHOLDS: PriorityThreshold[] = [
  { value: 0, color: THEME_BADGE_SENTINEL },
];

type RevisionTracker = {
  settings: number;
  pause: number;
  messages: number;
  connection: number;
  runtime: number;
  stream_error: number;
};

function loadThemePreference(): ThemePreference {
  if (typeof window === "undefined") return "system";
  const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
  return stored === "light" || stored === "dark" || stored === "system" || stored === "dracula" ? stored : "system";
}

function normalizePriorityThresholds(input: PriorityThreshold[] | null | undefined): PriorityThreshold[] {
  const source = input && input.length > 0 ? input : DEFAULT_PRIORITY_THRESHOLDS;
  const sanitized = source.map((threshold) => ({
    value: Math.max(0, Number.isFinite(threshold.value) ? Math.floor(threshold.value) : 0),
    color:
      threshold.color === THEME_BADGE_SENTINEL ||
      (threshold.value === 0 && threshold.color.toUpperCase() === "#DEEAF8")
        ? THEME_BADGE_SENTINEL
        : /^#[0-9a-fA-F]{6}$/.test(threshold.color)
          ? threshold.color.toUpperCase()
          : "#6B8DB6",
  }));
  sanitized.sort((a, b) => a.value - b.value);

  const deduped: PriorityThreshold[] = [];
  for (const threshold of sanitized) {
    const last = deduped[deduped.length - 1];
    if (last && last.value === threshold.value) {
      deduped[deduped.length - 1] = threshold;
    } else {
      deduped.push(threshold);
    }
  }

  return deduped.length > 0 ? deduped : [...DEFAULT_PRIORITY_THRESHOLDS];
}

function toUiMessagesSnapshot(messages: GotifyMessage[]): UiMessage[] {
  return messages.map((message) => toUiMessage(message));
}

function GearIcon() {
  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" style={{ width: 17, height: 17, fill: "none", stroke: "currentColor", strokeWidth: 2, strokeLinecap: "round", strokeLinejoin: "round" }}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </svg>
  );
}

function CloseIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden="true"
      style={{ width: 16, height: 16, fill: "none", stroke: "currentColor", strokeWidth: 2.2, strokeLinecap: "round" }}
    >
      <path d="M6 6l12 12" />
      <path d="M18 6l-12 12" />
    </svg>
  );
}

export function App() {
  const [connectionState, setConnectionState] = useState<ConnectionState>("Disconnected");
  const [baseUrl, setBaseUrl] = useState("");
  const [token, setToken] = useState("");
  const [minPriority, setMinPriority] = useState(0);
  const [priorityThresholds, setPriorityThresholds] = useState<PriorityThreshold[]>(() => [...DEFAULT_PRIORITY_THRESHOLDS]);
  const [activePriorityThresholds, setActivePriorityThresholds] = useState<PriorityThreshold[]>(() => [...DEFAULT_PRIORITY_THRESHOLDS]);
  const [cacheLimit, setCacheLimit] = useState(100);
  const [activeCacheLimit, setActiveCacheLimit] = useState(100);
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
  const [testConnectionFlash, setTestConnectionFlash] = useState<"ok" | "error" | null>(null);
  const [feedback, setFeedback] = useState<{ kind: "ok" | "error"; message: string } | null>(null);
  const [diagnostics, setDiagnostics] = useState<RuntimeDiagnostics | null>(null);
  const [deletingMessageIds, setDeletingMessageIds] = useState<Record<string, boolean>>({});
  const [urlPreviews, setUrlPreviews] = useState<Record<string, UrlPreview | null>>({});
  const [themePreference, setThemePreference] = useState<ThemePreference>(() => loadThemePreference());
  const [activeThemePreference, setActiveThemePreference] = useState<ThemePreference>(() => loadThemePreference());
  // JS object keys are always strings at runtime, so use Record<string, boolean>
  // to match the actual key type (message IDs coerce to string on assignment).

  const [pauseMenuOpen, setPauseMenuOpen] = useState(false);
  const lastRecoveryAttemptRef = useRef(0);
  const revisionsRef = useRef<RevisionTracker>({
    settings: 0,
    pause: 0,
    messages: 0,
    connection: 0,
    runtime: 0,
    stream_error: 0,
  });
  const updateChannelRef = useRef<Channel<AppUpdate> | null>(null);
  const cacheLimitRef = useRef(activeCacheLimit);
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

  const applySettingsSnapshot = (snapshot: DomainSnapshot<SettingsResponse>) => {
    if (snapshot.revision <= revisionsRef.current.settings) return false;
    revisionsRef.current.settings = snapshot.revision;
    const settings = snapshot.data;
    setBaseUrl(settings.base_url ?? "");
    setHasStoredToken(settings.has_token);
    setMinPriority(settings.min_priority ?? 0);
    const normalizedThresholds = normalizePriorityThresholds(settings.priority_thresholds);
    setPriorityThresholds(normalizedThresholds);
    setActivePriorityThresholds(normalizedThresholds);
    const normalizedCacheLimit = settings.cache_limit ?? 100;
    setCacheLimit(normalizedCacheLimit);
    setActiveCacheLimit(normalizedCacheLimit);
    setLaunchAtLogin(settings.launch_at_login ?? false);
    setStartMinimizedToTray(settings.start_minimized_to_tray ?? false);
    setQuietStart(settings.quiet_hours_start == null ? "" : String(settings.quiet_hours_start));
    setQuietEnd(settings.quiet_hours_end == null ? "" : String(settings.quiet_hours_end));
    applyPauseState(settings.pause_until ?? null, settings.pause_mode ?? null);
    return true;
  };

  const applyPauseSnapshot = (snapshot: DomainSnapshot<PauseStateData>) => {
    if (snapshot.revision <= revisionsRef.current.pause) return false;
    revisionsRef.current.pause = snapshot.revision;
    applyPauseState(snapshot.data.pause_until ?? null, snapshot.data.pause_mode ?? null);
    return true;
  };

  const applyMessagesReplaceSnapshot = (snapshot: DomainSnapshot<GotifyMessage[]>) => {
    if (snapshot.revision <= revisionsRef.current.messages) return false;
    revisionsRef.current.messages = snapshot.revision;
    setMessages(toUiMessagesSnapshot(snapshot.data));
    return true;
  };

  const applyMessageUpsertSnapshot = (snapshot: DomainSnapshot<GotifyMessage>) => {
    if (snapshot.revision <= revisionsRef.current.messages) return false;
    revisionsRef.current.messages = snapshot.revision;
    const incoming = toUiMessage(snapshot.data);
    setMessages((current) => {
      const withoutExisting = current.filter((item) => item.id !== incoming.id);
      return [incoming, ...withoutExisting].slice(0, cacheLimitRef.current);
    });
    return true;
  };

  const applyMessageRemoveSnapshot = (snapshot: DomainSnapshot<MessageRemovedData>) => {
    if (snapshot.revision <= revisionsRef.current.messages) return false;
    revisionsRef.current.messages = snapshot.revision;
    setMessages((current) => current.filter((item) => item.id !== snapshot.data.message_id));
    return true;
  };

  const applyConnectionSnapshot = (snapshot: DomainSnapshot<{ state: ConnectionState }>) => {
    if (snapshot.revision <= revisionsRef.current.connection) return false;
    revisionsRef.current.connection = snapshot.revision;
    setConnectionState(snapshot.data.state);
    return true;
  };

  const applyRuntimeSnapshot = (snapshot: DomainSnapshot<RuntimeDiagnostics>) => {
    if (snapshot.revision <= revisionsRef.current.runtime) return false;
    revisionsRef.current.runtime = snapshot.revision;
    setDiagnostics(snapshot.data);
    setConnectionState(snapshot.data.connection_state);
    return true;
  };

  const applyStreamErrorSnapshot = (snapshot: DomainSnapshot<StreamErrorData>) => {
    if (snapshot.revision <= revisionsRef.current.stream_error) return false;
    revisionsRef.current.stream_error = snapshot.revision;
    setFeedback({ kind: "error", message: snapshot.data.message });
    return true;
  };

  const applyBootstrap = (bootstrap: BootstrapState) => {
    applySettingsSnapshot(bootstrap.settings);
    applyPauseSnapshot(bootstrap.pause);
    applyMessagesReplaceSnapshot(bootstrap.messages);
    applyConnectionSnapshot(bootstrap.connection);
    applyRuntimeSnapshot(bootstrap.runtime);
  };

  const handleAppUpdate = (update: AppUpdate) => {
    switch (update.type) {
      case "settings.updated":
        applySettingsSnapshot(update.payload);
        return;
      case "pause.updated":
        applyPauseSnapshot(update.payload);
        return;
      case "messages.replace":
        applyMessagesReplaceSnapshot(update.payload);
        return;
      case "messages.upsert":
        applyMessageUpsertSnapshot(update.payload);
        return;
      case "messages.remove":
        applyMessageRemoveSnapshot(update.payload);
        return;
      case "connection.updated":
        applyConnectionSnapshot(update.payload);
        return;
      case "runtime.updated":
        applyRuntimeSnapshot(update.payload);
        return;
      case "stream.error":
        applyStreamErrorSnapshot(update.payload);
        return;
      default:
        return;
    }
  };

  useEffect(() => {
    cacheLimitRef.current = Math.max(1, activeCacheLimit);
  }, [activeCacheLimit]);

  useEffect(() => {
    try {
      const label = getCurrentWebviewWindow().label;
      setIsQuickWindow(label === "quick");
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
    let destroyed = false;
    let unlistenNotification: (() => void) | undefined;
    let unlistenNotificationClicked: (() => void) | undefined;

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

    const initialize = async () => {
      try {
        const bootstrap = await invoke<BootstrapState>("bootstrap_state");
        if (destroyed) return;
        applyBootstrap(bootstrap);

        const channel = new Channel<AppUpdate>((update) => {
          if (destroyed) return;
          handleAppUpdate(update);
        });
        updateChannelRef.current = channel;
        await invoke("subscribe_app_updates", { channel });
      } catch (error) {
        updateChannelRef.current = null;
        setFeedback({ kind: "error", message: String(error) });
      } finally {
        if (!destroyed) {
          setIsLoading(false);
        }
      }
    };

    void initialize();

    return () => {
      destroyed = true;
      if (unlistenNotification) unlistenNotification();
      if (unlistenNotificationClicked) unlistenNotificationClicked();
      updateChannelRef.current = null;
      void invoke("unsubscribe_app_updates").catch(() => {});
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
    window.localStorage.setItem(THEME_STORAGE_KEY, activeThemePreference);
    const root = document.documentElement;
    if (activeThemePreference === "system") {
      root.removeAttribute("data-theme");
      return;
    }
    root.setAttribute("data-theme", activeThemePreference);
  }, [activeThemePreference]);

  // When the theme is changed in the main window, the quick window (a separate
  // WebView) won't see the React state update. The browser fires a `storage`
  // event in all *other* same-origin windows when localStorage is written, so
  // we listen for it here to keep the quick window's theme in sync.
  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key !== THEME_STORAGE_KEY || event.newValue === null) return;
      const val = event.newValue;
      if (val === "light" || val === "dark" || val === "system" || val === "dracula") {
        setActiveThemePreference(val);
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

  useEffect(() => {
    if (testConnectionFlash == null) return;
    const timer = window.setTimeout(() => {
      setTestConnectionFlash(null);
    }, 1400);
    return () => {
      window.clearTimeout(timer);
    };
  }, [testConnectionFlash]);

  useEffect(() => {
    const triggerRecovery = async () => {
      const now = Date.now();
      if (now - lastRecoveryAttemptRef.current < 2000) return;
      lastRecoveryAttemptRef.current = now;
      try {
        const snapshot = await invoke<DomainSnapshot<RuntimeDiagnostics>>("recover_stream");
        applyRuntimeSnapshot(snapshot);
      } catch {
        // ignore recovery invoke failures
      }
    };

    const onOnline = () => void triggerRecovery();
    const onFocus = () => void triggerRecovery();
    const onVisibility = () => {
      if (!document.hidden) {
        void triggerRecovery();
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
      const normalizedThresholds = normalizePriorityThresholds(priorityThresholds);

      const settingsSnapshot = await invoke<DomainSnapshot<SettingsResponse>>("save_settings", {
        baseUrl,
        token,
        minPriority,
        priorityColorMode: "thresholds",
        priorityThresholds: normalizedThresholds,
        cacheLimit,
        launchAtLogin,
        startMinimizedToTray,
        quietHoursStart,
        quietHoursEnd,
      });
      applySettingsSnapshot(settingsSnapshot);
      setActiveThemePreference(themePreference);
      setToken("");
      setFeedback({ kind: "ok", message: "Settings saved. Reconnecting..." });
      setDrawerTab(null);
      // Restart stream with new credentials; connection/runtime updates flow through app updates.
      void invoke<DomainSnapshot<RuntimeDiagnostics>>("restart_stream")
        .then((snapshot) => {
          applyRuntimeSnapshot(snapshot);
        })
        .catch(() => {});
    } catch (error) {
      setFeedback({ kind: "error", message: String(error) });
    } finally {
      setIsSaving(false);
    }
  };

  const onTest = async () => {
    setIsTesting(true);
    setTestConnectionFlash(null);
    try {
      await invoke<string>("test_connection", {
        baseUrl,
        token: token.trim().length > 0 ? token : null,
      });
      setTestConnectionFlash("ok");
    } catch (error) {
      console.error(error);
      setTestConnectionFlash("error");
    } finally {
      setIsTesting(false);
    }
  };

  const onResetPriorityThresholds = () => {
    setPriorityThresholds([...DEFAULT_PRIORITY_THRESHOLDS]);
  };

  const onPause = async (minutes: number) => {
    setPauseMenuOpen(false);
    try {
      const snapshot = await invoke<DomainSnapshot<PauseStateData>>("set_pause", {
        input: { minutes },
      });
      applyPauseSnapshot(snapshot);
    } catch (error) {
      setFeedback({ kind: "error", message: String(error) });
    }
  };

  const onPauseForever = async () => {
    setPauseMenuOpen(false);
    try {
      const snapshot = await invoke<DomainSnapshot<PauseStateData>>("set_pause", {
        input: { forever: true },
      });
      applyPauseSnapshot(snapshot);
    } catch (error) {
      setFeedback({ kind: "error", message: String(error) });
    }
  };

  const onResumeNotifications = async () => {
    setPauseMenuOpen(false);
    try {
      const snapshot = await invoke<DomainSnapshot<PauseStateData>>("resume_pause");
      applyPauseSnapshot(snapshot);
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
      const snapshot = await invoke<DomainSnapshot<GotifyMessage[]>>("delete_message", { messageId });
      applyMessagesReplaceSnapshot(snapshot);
    } catch (error) {
      // Server rejected the delete — restore the message
      if (snapshot) {
        setMessages((current) => {
          const restored = toUiMessage(snapshot);
          const withoutExisting = current.filter((item) => item.id !== restored.id);
          return [restored, ...withoutExisting].slice(0, cacheLimitRef.current);
        });
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
          priorityThresholds={activePriorityThresholds}
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
            {drawerTab === "settings" ? (
              <aside className="drawer-side-info" aria-label="About Gotify Desktop">
                <div className="drawer-side-atmosphere" aria-hidden="true" />
                <div className="drawer-side-card">
                  <div className="drawer-side-brand">
                    <span className="drawer-side-logo-wrap">
                      <img src={gotifyLogo} alt="" className="drawer-side-logo" />
                    </span>
                    <div>
                      <div className="drawer-side-kicker">Desktop Companion</div>
                      <div className="drawer-side-title">Gotify Desktop</div>
                      <div className="drawer-side-version">Version {__APP_VERSION__}</div>
                    </div>
                  </div>
                  <p className="drawer-side-summary">
                    Native macOS companion for your Gotify server: real-time message stream, focused inbox, and notification controls.
                  </p>
                  <div className="drawer-side-links">
                    <a href="https://github.com/itsamenathan/gotify-desktop-macos" target="_blank" rel="noreferrer">
                      Project GitHub
                    </a>
                    <a href="https://gotify.net/" target="_blank" rel="noreferrer">
                      Gotify Website
                    </a>
                  </div>
                </div>
              </aside>
            ) : null}
            <aside className={drawerTab ? "drawer-panel open" : "drawer-panel"} aria-hidden={drawerTab == null}>
          <div className="drawer-head">
            <h2>{drawerTab === "diagnostics" ? "Diagnostics" : "Settings"}</h2>
            <button
              type="button"
              className="utility-button icon-button"
              aria-label="Close panel"
              title="Close panel"
              onClick={() => setDrawerTab(null)}
            >
              <CloseIcon />
            </button>
          </div>
          <div className="drawer-tabs">
            <div className="drawer-tab-list">
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
            <div className="drawer-tab-actions">
              {drawerTab === "settings" ? (
                <button
                  type="submit"
                  form="settings-form"
                  className="drawer-save-button"
                  disabled={isLoading || isSaving || isTesting}
                >
                  {isSaving ? "Saving..." : "Save"}
                </button>
              ) : null}
            </div>
          </div>

          <div className="drawer-body">
            {drawerTab === "settings" ? (
              <SettingsForm
                baseUrl={baseUrl}
                token={token}
                hasStoredToken={hasStoredToken}
                minPriority={minPriority}
                priorityThresholds={priorityThresholds}
                quietStart={quietStart}
                quietEnd={quietEnd}
                cacheLimit={cacheLimit}
                launchAtLogin={launchAtLogin}
                startMinimizedToTray={startMinimizedToTray}
                themePreference={themePreference}
                isLoading={isLoading}
                isSaving={isSaving}
                isTesting={isTesting}
                testConnectionFlash={testConnectionFlash}
                feedback={feedback}
                onSave={onSave}
                onTest={onTest}
                setBaseUrl={setBaseUrl}
                setToken={setToken}
                setMinPriority={setMinPriority}
                setPriorityThresholds={setPriorityThresholds}
                onResetPriorityThresholds={onResetPriorityThresholds}
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
                baseUrl={baseUrl}
                connectionState={connectionState}
                diagnostics={diagnostics}
                messageCount={messages.length}
                streamIdleSeconds={streamIdleSeconds}
                onForceReconnect={() => {
                  void invoke<DomainSnapshot<RuntimeDiagnostics>>("restart_stream")
                    .then((snapshot) => {
                      applyRuntimeSnapshot(snapshot);
                    })
                    .catch(() => {});
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
