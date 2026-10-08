import { useCallback, useEffect, useState } from "react";
import { font } from "../tokens/values";
import { theme } from "./theme";
import { Button, Card } from "./ui";
import {
  cancelModelDownload,
  downloadModel,
  listModels,
  onModelProgress,
  useModel,
  type ModelInfo,
  type ModelProgress,
} from "./api";

const fmtSize = (bytes: number) =>
  bytes >= 1e9 ? `${(bytes / 1e9).toFixed(1)} GB` : `${Math.round(bytes / 1e6)} MB`;

function Bar({ fraction }: { fraction: number }) {
  return (
    <div style={{ height: 6, borderRadius: 3, background: theme.track, overflow: "hidden", marginTop: 8 }}>
      <div
        style={{
          width: `${Math.max(0, Math.min(1, fraction)) * 100}%`,
          height: "100%",
          background: theme.accent,
          transition: "width 150ms linear",
        }}
      />
    </div>
  );
}

function Row({
  model,
  progress,
  busyElsewhere,
  onDownload,
  onUse,
}: {
  model: ModelInfo;
  progress: ModelProgress | undefined;
  busyElsewhere: boolean;
  onDownload: () => void;
  onUse: () => void;
}) {
  const live = progress && (progress.state === "downloading" || progress.state === "verifying");
  const failed = progress?.state === "error";
  const note = progress?.state === "done" ? progress.message : null;
  const fraction = progress && progress.total > 0 ? progress.downloaded / progress.total : 0;

  return (
    <div style={{ padding: "12px 0", borderTop: `1px solid ${theme.border}` }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 14 }}>
        <div style={{ minWidth: 0 }}>
          <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
            {model.title} <span style={{ color: theme.textFaint, fontWeight: 400 }}>· {fmtSize(model.size_bytes)}</span>
          </div>
          <div style={{ fontSize: 12.5, color: theme.textMuted, marginTop: 2, lineHeight: 1.45 }}>{model.blurb}</div>
        </div>
        <div style={{ flex: "0 0 auto" }}>
          {live ? (
            <Button variant="ghost" size="sm" onClick={() => void cancelModelDownload()}>
              Cancel
            </Button>
          ) : model.selectable ? (
            <Button variant="ghost" size="sm" onClick={onUse}>
              Use
            </Button>
          ) : model.installed ? (
            <span style={{ fontSize: 12.5, fontWeight: 600, color: model.active ? theme.accentDeep : theme.textMuted }}>
              {model.active ? "In use" : "Installed"}
            </span>
          ) : (
            <Button variant="accent" size="sm" disabled={busyElsewhere} onClick={onDownload}>
              {failed ? "Try again" : "Download"}
            </Button>
          )}
        </div>
      </div>
      {live && (
        <>
          <Bar fraction={progress.state === "verifying" ? 1 : fraction} />
          <div style={{ fontSize: 12, color: theme.textMuted, marginTop: 4 }}>
            {progress.state === "verifying"
              ? "Checking the download…"
              : `${fmtSize(progress.downloaded)} of ${fmtSize(progress.total)} · ${Math.round(fraction * 100)}%`}
          </div>
        </>
      )}
      {failed && progress.message && (
        <div style={{ fontSize: 12.5, color: "#ff6b6b", marginTop: 6, lineHeight: 1.45 }}>{progress.message}</div>
      )}
      {note && <div style={{ fontSize: 12.5, color: theme.textMuted, marginTop: 6 }}>{note}</div>}
    </div>
  );
}

// The speech models: what's installed, and the way to get or upgrade one. Used on the setup
// screen and in Settings. Downloads resume if interrupted and are checked before use.
export function ModelsCard({ intro, onChanged }: { intro?: string; onChanged?: () => void }) {
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [progress, setProgress] = useState<Record<string, ModelProgress>>({});
  const [startError, setStartError] = useState<string | null>(null);

  const reload = useCallback(() => void listModels().then(setModels), []);

  useEffect(() => {
    reload();
    let off = () => {};
    let cancelled = false;
    void onModelProgress((p) => {
      setProgress((prev) => ({ ...prev, [p.id]: p }));
      if (p.state === "done" || p.state === "cancelled" || p.state === "error") {
        reload();
        onChanged?.();
      }
    }).then((unlisten) => {
      if (cancelled) unlisten();
      else off = unlisten;
    });
    return () => {
      cancelled = true;
      off();
    };
  }, [reload, onChanged]);

  const busy = models.some((m) => m.downloading) ||
    Object.values(progress).some((p) => p.state === "downloading" || p.state === "verifying");

  const start = async (id: string) => {
    setStartError(null);
    setProgress((prev) => ({ ...prev, [id]: { id, state: "downloading", downloaded: 0, total: 0, message: null } }));
    try {
      await downloadModel(id);
    } catch (e) {
      setProgress((prev) => {
        const { [id]: _drop, ...rest } = prev;
        return rest;
      });
      setStartError(String(e));
    }
  };

  const use = async (id: string) => {
    setStartError(null);
    try {
      await useModel(id);
    } catch (e) {
      setStartError(String(e));
    }
    reload();
  };

  return (
    <Card style={{ marginBottom: 16 }}>
      <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong, fontFamily: font.ui }}>Speech model</div>
      <div style={{ color: theme.textMuted, fontSize: 12.5, marginTop: 4, lineHeight: 1.5, marginBottom: 6 }}>
        {intro ??
          "WhimprFlow listens with a speech model that runs on this Mac. It downloads once, resumes if your connection drops, and is checked before it's used."}
      </div>
      {models.map((m) => (
        <Row
          key={m.id}
          model={m}
          progress={progress[m.id]}
          busyElsewhere={busy}
          onDownload={() => void start(m.id)}
          onUse={() => void use(m.id)}
        />
      ))}
      {startError && <div style={{ fontSize: 12.5, color: "#ff6b6b", marginTop: 8 }}>{startError}</div>}
    </Card>
  );
}
