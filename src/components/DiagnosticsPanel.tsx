import type { ConnectionState, RuntimeDiagnostics } from "../types";

type DiagnosticsPanelProps = {
  baseUrl: string;
  connectionState: ConnectionState;
  diagnostics: RuntimeDiagnostics | null;
  messageCount: number;
  streamIdleSeconds: number;
  onForceReconnect: () => void;
};

export function DiagnosticsPanel({
  baseUrl,
  connectionState,
  diagnostics,
  messageCount,
  streamIdleSeconds,
  onForceReconnect,
}: DiagnosticsPanelProps) {
  return (
    <>
      <div className="diagnostics">
        <div><span>Server:</span> <strong>{baseUrl || "—"}</strong></div>
        <div><span>Connection:</span> <strong>{diagnostics?.connection_state ?? connectionState}</strong></div>
        <div><span>Cached messages:</span> <strong>{messageCount}</strong></div>
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
        <div><span>Stream idle:</span> <strong>{streamIdleSeconds}s</strong></div>
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
        <button type="button" className="utility-button" onClick={onForceReconnect}>
          Force Reconnect
        </button>
      </div>
    </>
  );
}
