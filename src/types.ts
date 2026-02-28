export type ConnectionState = "Connected" | "Disconnected" | "Connecting" | "Backoff";

export type SettingsResponse = {
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

export type PauseStateResponse = {
  pause_until: number | null;
  pause_mode: string | null;
};

export type GotifyMessage = {
  id: number;
  app_id: number;
  title: string;
  message: string;
  priority: number;
  app: string;
  app_icon: string | null;
  date: string;
};

export type UiMessage = GotifyMessage & {
  rendered_html: string;
  primary_url: string | null;
  parsed_ts: number | null;
  formatted_time: string;
};

export type RuntimeDiagnostics = {
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

export type AppGroup = {
  key: string;
  name: string;
  count: number;
  icon: string | null;
};

export type UrlPreview = {
  url: string;
  title: string | null;
  description: string | null;
  site_name: string | null;
  image: string | null;
};

export type ThemePreference = "system" | "light" | "dark" | "dracula";
export type DrawerTab = "settings" | "diagnostics";
export type PauseMode = "15m" | "1h" | "custom" | "forever";

export type SelectionHistoryState = {
  tag: "gotify-selection-v1";
  app: string;
  messageId: number | null;
};
