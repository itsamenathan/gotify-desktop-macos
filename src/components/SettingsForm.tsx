import { FormEvent } from "react";
import type { PriorityThreshold, ThemePreference } from "../types";

type SettingsFormProps = {
  baseUrl: string;
  token: string;
  hasStoredToken: boolean;
  minPriority: number;
  priorityThresholds: PriorityThreshold[];
  quietStart: string;
  quietEnd: string;
  cacheLimit: number;
  launchAtLogin: boolean;
  startMinimizedToTray: boolean;
  themePreference: ThemePreference;
  isLoading: boolean;
  isSaving: boolean;
  isTesting: boolean;
  testConnectionFlash: "ok" | "error" | null;
  feedback: { kind: "ok" | "error"; message: string } | null;
  onSave: (event: FormEvent<HTMLFormElement>) => void;
  onTest: () => void;
  setBaseUrl: (value: string) => void;
  setToken: (value: string) => void;
  setMinPriority: (value: number) => void;
  setPriorityThresholds: (value: PriorityThreshold[]) => void;
  onResetPriorityThresholds: () => void;
  setQuietStart: (value: string) => void;
  setQuietEnd: (value: string) => void;
  setCacheLimit: (value: number) => void;
  setLaunchAtLogin: (value: boolean) => void;
  setStartMinimizedToTray: (value: boolean) => void;
  setThemePreference: (value: ThemePreference) => void;
};

export function SettingsForm(props: SettingsFormProps) {
  const {
    baseUrl,
    token,
    hasStoredToken,
    minPriority,
    priorityThresholds,
    quietStart,
    quietEnd,
    cacheLimit,
    launchAtLogin,
    startMinimizedToTray,
    themePreference,
    isLoading,
    isSaving,
    isTesting,
    testConnectionFlash,
    feedback,
    onSave,
    onTest,
    setBaseUrl,
    setToken,
    setMinPriority,
    setPriorityThresholds,
    onResetPriorityThresholds,
    setQuietStart,
    setQuietEnd,
    setCacheLimit,
    setLaunchAtLogin,
    setStartMinimizedToTray,
    setThemePreference,
  } = props;
  const disabled = isLoading || isSaving || isTesting;
  const themeBadgeColor = getThemeBadgeColor();
  const addThreshold = () => {
    const last = priorityThresholds[priorityThresholds.length - 1];
    const nextValue = last ? last.value + 1 : 0;
    setPriorityThresholds([...priorityThresholds, { value: nextValue, color: "#E25555" }]);
  };

  return (
    <form id="settings-form" className="settings-form" onSubmit={onSave}>
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
              disabled={disabled}
            />
          </label>
          <label className="settings-field">
            <span className="settings-label">Client token</span>
            <span className="settings-hint">Found in Gotify - Clients</span>
            <input
              type="password"
              value={token}
              onChange={(event) => setToken(event.target.value)}
              placeholder={hasStoredToken ? "Leave blank to keep existing" : "Enter Gotify client token"}
              required={!hasStoredToken}
              autoComplete="off"
              disabled={disabled}
            />
          </label>
          <div className="settings-field">
            <div className="settings-inline-actions">
              <button
                type="button"
                className={
                  testConnectionFlash === "ok"
                    ? "secondary-button test-connection-button flash-ok"
                    : testConnectionFlash === "error"
                      ? "secondary-button test-connection-button flash-error"
                      : "secondary-button test-connection-button"
                }
                onClick={onTest}
                disabled={disabled}
              >
                {isTesting ? "Testing..." : "Test Connection"}
              </button>
            </div>
          </div>
        </div>
      </div>

      <div className="settings-group">
        <p className="settings-group-title">Notifications</p>
        <div className="settings-card">
          <label className="settings-field">
            <span className="settings-label">Minimum priority</span>
            <span className="settings-hint">Only notify for messages at this priority or above (0-10)</span>
            <input
              type="number"
              min={0}
              max={10}
              value={minPriority}
              onChange={(event) => setMinPriority(Number(event.target.value || 0))}
              disabled={disabled}
            />
          </label>
          <div className="settings-field">
            <span className="settings-label">Priority colors</span>
            <span className="settings-hint">Thresholds apply when priority is greater than or equal to each value</span>
            <div className="threshold-list">
              {priorityThresholds.map((threshold, index) => {
                const nextThreshold = priorityThresholds[index + 1];
                return (
                  <div key={`${threshold.value}-${index}`} className="threshold-row">
                    <label>
                      <span className="settings-sublabel">From</span>
                      <input
                        type="number"
                        min={0}
                        max={999}
                        value={threshold.value}
                        onChange={(event) => {
                          const next = [...priorityThresholds];
                          next[index] = {
                            ...threshold,
                            value: Math.max(0, Number(event.target.value || 0)),
                          };
                          setPriorityThresholds(next);
                        }}
                        disabled={disabled}
                      />
                    </label>
                    <label>
                      <span className="settings-sublabel">Color</span>
                      <input
                        type="color"
                        value={toColorInputValue(threshold.color, themeBadgeColor)}
                        onChange={(event) => {
                          const next = [...priorityThresholds];
                          next[index] = { ...threshold, color: event.target.value.toUpperCase() };
                          setPriorityThresholds(next);
                        }}
                        disabled={disabled}
                      />
                    </label>
                    <div className="threshold-preview">
                      {nextThreshold ? `>= ${threshold.value} and < ${nextThreshold.value}` : `>= ${threshold.value}`}
                    </div>
                    <button
                      type="button"
                      className="danger-button subtle"
                      onClick={() => {
                        setPriorityThresholds(priorityThresholds.filter((_, rowIndex) => rowIndex !== index));
                      }}
                      disabled={disabled || priorityThresholds.length <= 1}
                    >
                      Remove
                    </button>
                  </div>
                );
              })}
              <div className="threshold-actions">
                <button
                  type="button"
                  className="secondary-button"
                  onClick={addThreshold}
                  disabled={disabled}
                >
                  Add threshold
                </button>
                <button
                  type="button"
                  className="secondary-button"
                  onClick={onResetPriorityThresholds}
                  disabled={disabled}
                >
                  Reset to theme default
                </button>
              </div>
            </div>
          </div>
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
                  disabled={disabled}
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
                  disabled={disabled}
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
            <span className="settings-hint">Maximum messages stored locally (1-2000)</span>
            <input
              type="number"
              min={1}
              max={2000}
              value={cacheLimit}
              onChange={(event) => setCacheLimit(Number(event.target.value || 100))}
              disabled={disabled}
            />
          </label>
          <label className="settings-toggle">
            <span className="settings-label">Launch at login</span>
            <input
              type="checkbox"
              checked={launchAtLogin}
              onChange={(event) => setLaunchAtLogin(event.target.checked)}
              disabled={disabled}
            />
          </label>
          <label className="settings-toggle">
            <span className="settings-label">Start minimized to tray</span>
            <input
              type="checkbox"
              checked={startMinimizedToTray}
              onChange={(event) => setStartMinimizedToTray(event.target.checked)}
              disabled={disabled}
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
              disabled={disabled}
            >
              <option value="system">System (follows macOS)</option>
              <option value="light">Light</option>
              <option value="dark">Dark</option>
              <option value="dracula">Dracula</option>
            </select>
          </label>
        </div>
      </div>

      {feedback ? (
        <div className={feedback.kind === "ok" ? "feedback ok" : "feedback error"}>
          {feedback.message}
        </div>
      ) : null}
    </form>
  );
}

function toColorInputValue(raw: string, themeBadgeColor: string): string {
  if (raw === "__THEME_BADGE__") return themeBadgeColor;
  if (/^#[0-9a-fA-F]{6}$/.test(raw)) return raw.toUpperCase();
  return "#6B8DB6";
}

function getThemeBadgeColor(): string {
  if (typeof window === "undefined") return "#DEEAF8";
  const raw = getComputedStyle(document.documentElement).getPropertyValue("--badge-bg").trim();
  const rgbMatch = raw.match(/^rgba?\((\d+),\s*(\d+),\s*(\d+)/i);
  if (rgbMatch) {
    const toHex = (v: string) => Number(v).toString(16).padStart(2, "0");
    return `#${toHex(rgbMatch[1])}${toHex(rgbMatch[2])}${toHex(rgbMatch[3])}`.toUpperCase();
  }
  if (/^#[0-9a-fA-F]{6}$/.test(raw)) {
    return raw.toUpperCase();
  }
  return "#DEEAF8";
}
