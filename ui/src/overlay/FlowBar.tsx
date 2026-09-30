import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { palette, pillFill, geometry, font } from "../tokens/values";

// Visual states, mirroring the Rust `BarState`.
export type BarState =
  | "idle"
  | "recording"
  | "locked"
  | "transcribing"
  | "done"
  | "cancelled"
  | "error";

type StateEvent = { state: BarState };
type WaveformEvent = { bars: number[] };
// Mirrors `diag::ErrorDto` in src-tauri/src/diag.rs.
type ErrorEvent = { headline: string; detail: string };
// Live raw transcript from the local Whisper pass, re-decoded every ~900ms
// while recording — NOT the cleaned-up text (that only arrives once, from
// `whimpr://transcript`, after the cloud cleanup pass runs at release).
type TranscriptEvent = { text: string };

async function tauriListen<T>(event: string, cb: (payload: T) => void): Promise<() => void> {
  try {
    const { listen } = await import("@tauri-apps/api/event");
    return await listen<T>(event, (e) => cb(e.payload as T));
  } catch {
    return () => {};
  }
}

// Tell the Rust side to resize + re-anchor the actual OS overlay window so
// its bounds match the pill's current on-screen size (the window is a tight
// hit-test box around the pill, not a full-screen canvas — see build_overlay
// in src-tauri/src/lib.rs).
async function resizeOverlayWindow(width: number, height: number): Promise<void> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke("resize_overlay", { width, height });
  } catch {
    // Best-effort — the pill still renders correctly even if the native
    // window doesn't track it (e.g. running outside Tauri in dev/browser).
  }
}

// A row of dot-like rounded bars driven by mic RMS — Wispr's dotted-waveform look:
// small dots when quiet, rising into a waveform when speaking.
function DottedWaveform({ bars }: { bars: number[] }) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const barsRef = useRef<number[]>(bars);
  barsRef.current = bars;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    let raf = 0;
    const N = 16;
    const draw = () => {
      const dpr = window.devicePixelRatio || 1;
      const w = canvas.clientWidth;
      const h = canvas.clientHeight;
      if (canvas.width !== w * dpr || canvas.height !== h * dpr) {
        canvas.width = w * dpr;
        canvas.height = h * dpr;
      }
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const dotW = 2.4;
      const gap = (w - N * dotW) / (N - 1);
      const t = performance.now();
      ctx.fillStyle = palette.waveBar;
      for (let i = 0; i < N; i++) {
        const real = barsRef.current[barsRef.current.length - 1 - (i % barsRef.current.length)];
        // Idle shimmer so the dotted line reads as "listening" even in near-silence.
        const shimmer = 0.12 + 0.06 * Math.abs(Math.sin(t / 260 + i * 0.7));
        const amp = Math.max(shimmer, real ?? 0);
        const bh = 3 + amp * 20; // 3px dot → up to ~23px bar
        const x = i * (dotW + gap);
        const y = (h - bh) / 2;
        ctx.beginPath();
        ctx.roundRect(x, y, dotW, bh, dotW / 2);
        ctx.fill();
      }
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  return <canvas ref={canvasRef} style={{ width: "100%", height: 28 }} />;
}

// Fire-and-forget invoke of a no-payload Tauri command (cancel/stop).
async function tauriInvoke(cmd: string): Promise<void> {
  try {
    const { invoke } = await import("@tauri-apps/api/core");
    await invoke(cmd);
  } catch {
    // no-op outside Tauri (e.g. browser dev preview)
  }
}

// Both buttons sit on top of the pill's drag region (see FlowBar's outer div)
// — stopPropagation on mousedown keeps a click from also being read as the
// start of a window drag.
function CancelButton() {
  return (
    <div
      title="Cancel (Esc)"
      role="button"
      onMouseDown={(e) => e.stopPropagation()}
      onClick={() => tauriInvoke("cancel_dictation")}
      style={{
        flex: "0 0 auto",
        width: 26,
        height: 26,
        borderRadius: 9999,
        background: "rgba(255,255,255,0.16)",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: "#fff",
        fontSize: 15,
        lineHeight: 1,
        cursor: "pointer",
      }}
    >
      ✕
    </div>
  );
}

function StopButton() {
  return (
    <div
      title="Stop"
      role="button"
      onMouseDown={(e) => e.stopPropagation()}
      onClick={() => tauriInvoke("stop_dictation")}
      style={{
        flex: "0 0 auto",
        width: 26,
        height: 26,
        borderRadius: 9999,
        background: "#FF5A52",
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        cursor: "pointer",
      }}
    >
      <div style={{ width: 9, height: 9, borderRadius: 2, background: "#fff" }} />
    </div>
  );
}

export function FlowBar() {
  const [state, setState] = useState<BarState>("idle");
  const [bars, setBars] = useState<number[]>([]);
  // Set by `whimpr://error`, shown while state === "error". Falls back to a
  // generic line if the error-state event somehow arrives without one (e.g.
  // an older build, or a future ShowBar(Error) call that doesn't go through
  // `diag::report`).
  const [errorText, setErrorText] = useState<ErrorEvent | null>(null);
  const [partialText, setPartialText] = useState("");
  // Measured height (px) of the wrapped live-transcript text, so the pill —
  // and the real OS window behind it — can grow to fit the whole utterance
  // instead of clipping it to one line.
  const [textH, setTextH] = useState(0);
  const textRef = useRef<HTMLDivElement | null>(null);
  // True from mousedown-on-the-pill until mouseup, so a native window "moved"
  // event during that window is treated as a user drag (and persisted) —
  // as opposed to one of our own programmatic `resize_overlay`/reset calls,
  // which also move the window but shouldn't be saved as a new position.
  const draggingRef = useRef(false);
  const saveTimeoutRef = useRef<number | undefined>(undefined);

  useEffect(() => {
    let un1: (() => void) | undefined;
    let un2: (() => void) | undefined;
    let un3: (() => void) | undefined;
    let un4: (() => void) | undefined;
    tauriListen<StateEvent>("whimpr://flowbar/state", (p) => {
      setState(p.state);
      if (p.state === "idle" || p.state === "recording") {
        setPartialText("");
        setTextH(0);
      }
    }).then((u) => (un1 = u));
    tauriListen<WaveformEvent>("whimpr://audio/waveform", (p) => setBars(p.bars)).then((u) => (un2 = u));
    tauriListen<ErrorEvent>("whimpr://error", (p) => setErrorText(p)).then((u) => (un3 = u));
    tauriListen<TranscriptEvent>("whimpr://transcript/partial", (p) => setPartialText(p.text)).then(
      (u) => (un4 = u),
    );
    return () => {
      un1?.();
      un2?.();
      un3?.();
      un4?.();
    };
  }, []);

  // Drag-to-move: the pill has `data-tauri-drag-region`, which lets the OS
  // move the (undecorated) window natively on mousedown-drag. We just need
  // to notice when that happened and persist the resulting position so it
  // sticks — see `save_overlay_position` in src-tauri/src/lib.rs.
  useEffect(() => {
    const onUp = () => {
      draggingRef.current = false;
    };
    window.addEventListener("mouseup", onUp);
    let unmoved: (() => void) | undefined;
    (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const { invoke } = await import("@tauri-apps/api/core");
        const win = getCurrentWindow();
        unmoved = await win.onMoved(() => {
          if (!draggingRef.current) return;
          window.clearTimeout(saveTimeoutRef.current);
          saveTimeoutRef.current = window.setTimeout(async () => {
            try {
              const pos = await win.outerPosition();
              await invoke("save_overlay_position", { x: pos.x, y: pos.y });
            } catch {
              // Best-effort; the drag itself already worked visually either way.
            }
          }, 150);
        });
      } catch {
        // Not running under Tauri (e.g. plain browser dev preview) — no-op.
      }
    })();
    return () => {
      window.removeEventListener("mouseup", onUp);
      unmoved?.();
    };
  }, []);

  const recording = state === "recording" || state === "locked";
  const isIdle = state === "idle";
  const processing = state === "transcribing";
  const isError = state === "error";
  const statusText =
    state === "transcribing"
      ? "Cleaning up…"
      : isError
        ? errorText?.headline ?? "Something's off"
        : state === "cancelled"
          ? "Discarded"
          : "Done";

  // Pill dimensions per state. Error gets extra width so the specific
  // headline (e.g. "Accessibility permission needed") isn't clipped —
  // truncating it back down to "Something's off" would defeat the point.
  //
  // Recording is fixed-width but grows TALL: the live transcript wraps
  // instead of truncating, and the pill's height tracks the measured text
  // height (see `textH`) up to RECORDING_MAX_H, so the user can watch their
  // whole utterance build up rather than only ever seeing the last few words.
  // Past the cap it scrolls, auto-pinned to the newest line.
  const RECORDING_W = 340;
  const RECORDING_MAX_H = 260;
  const recordingH = partialText ? Math.min(RECORDING_MAX_H, Math.max(44, textH + 24)) : 44;
  const dims = isIdle
    ? { w: 76, h: 16 }
    : recording
      ? { w: RECORDING_W, h: recordingH }
      : isError
        ? { w: 280, h: 36 }
        : { w: 180, h: 36 };

  // Keep the actual OS overlay window's bounds matching the pill so it isn't
  // clipped at the window edge, and so it stays anchored bottom-center as it
  // grows (see `resize_overlay` in src-tauri/src/lib.rs).
  useEffect(() => {
    resizeOverlayWindow(dims.w, dims.h);
  }, [dims.w, dims.h]);

  // Re-measure the wrapped transcript's natural height whenever it changes,
  // and keep the view scrolled to the newest line once it's tall enough to
  // scroll.
  useLayoutEffect(() => {
    const el = textRef.current;
    if (!el) return;
    setTextH(el.scrollHeight);
    el.scrollTop = el.scrollHeight;
  }, [partialText]);

  return (
    <div
      // "deep" (not bare/"true") because Tauri's drag script only starts a
      // drag when the exact clicked element carries the attribute, not an
      // ancestor — the visible pill is a nested child of this full-window
      // wrapper, so a bare attribute here never actually matched a click.
      data-tauri-drag-region="deep"
      title="Drag to move · double-click to reset position"
      onMouseDown={() => {
        draggingRef.current = true;
      }}
      onDoubleClick={async () => {
        try {
          const { invoke } = await import("@tauri-apps/api/core");
          await invoke("reset_overlay_position");
        } catch {
          // no-op outside Tauri
        }
      }}
      style={{
        position: "fixed",
        inset: 0,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        fontFamily: font.ui,
        userSelect: "none",
        cursor: "move",
      }}
    >
      <div
        aria-label={`WhimprFlow ${state}${isError && errorText ? `: ${errorText.detail}` : ""}`}
        title={isError ? errorText?.detail : undefined}
        style={{
          display: "flex",
          alignItems: recording && partialText ? "flex-start" : "center",
          justifyContent: recording ? "space-between" : "center",
          gap: 10,
          height: dims.h,
          width: dims.w,
          padding: recording ? (partialText ? "9px 8px" : "0 8px") : 0,
          background: pillFill.base,
          border: `1px solid rgba(255,255,255,0.10)`,
          borderRadius: 9999,
          boxShadow: pillFill.shadow,
          color: palette.pillText,
          transition: `width ${geometry.morphMs}ms ${motionEase}, height ${geometry.morphMs}ms ${motionEase}`,
          overflow: "hidden",
          fontSize: 13,
        }}
      >
        {isIdle ? null : recording ? (
          <>
            <CancelButton />
            <div style={{ flex: 1, minWidth: 0, alignSelf: partialText ? "stretch" : "center" }}>
              {partialText ? (
                // Live raw transcript from the local (free, no-API-cost) Whisper
                // pass — re-decoded every ~900ms while recording. Wraps and
                // grows the pill (see `dims`/`textH` above) so the whole
                // utterance stays visible instead of only the newest words;
                // once it exceeds RECORDING_MAX_H this scrolls, auto-pinned to
                // the latest line. This is NOT the cleaned-up text — that
                // still only appears once, at release, via the existing
                // cloud cleanup pipeline.
                <div
                  ref={textRef}
                  data-tauri-drag-region="false"
                  style={{
                    maxHeight: 260 - 24,
                    overflowY: "auto",
                    whiteSpace: "normal",
                    wordBreak: "break-word",
                    color: palette.pillText,
                    fontSize: 13,
                    lineHeight: "18px",
                  }}
                >
                  {partialText}
                </div>
              ) : (
                <DottedWaveform bars={bars} />
              )}
            </div>
            <StopButton />
          </>
        ) : processing ? (
          <span style={{ color: palette.pillTextMuted }}>{statusText}</span>
        ) : (
          <span style={{ color: palette.pillTextMuted }}>{statusText}</span>
        )}
      </div>
    </div>
  );
}

const motionEase = "cubic-bezier(0.05,0.6,0.4,0.95)";
