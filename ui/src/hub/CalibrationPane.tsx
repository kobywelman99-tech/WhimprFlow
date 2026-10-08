import { useEffect, useState } from "react";
import { font } from "../tokens/values";
import { theme } from "./theme";
import { Button, Card, PageTitle } from "./ui";
import {
  calibrationRecordStart,
  calibrationRecordStop,
  EMPTY_BRAIN,
  getBrain,
  getCalibrationPrompts,
  learnFromCalibration,
  resetBrain,
  type BrainOverview,
} from "./api";

const LINES = 10;

type Line = { tag: string; text: string; heard?: string };
type Stage = "intro" | "reading" | "learned";

function Patterns({ brain }: { brain: BrainOverview }) {
  return (
    <>
      <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong, margin: "14px 0 6px" }}>
        Sounds it has noticed
      </div>
      {brain.patterns.length === 0 ? (
        <div style={{ fontSize: 13, color: theme.textMuted }}>
          {brain.observations === 0
            ? "Nothing yet. Calibrate and it will start to notice how you sound."
            : "No pattern stands out yet. More sentences help it tell habits from one-off slips."}
        </div>
      ) : (
        brain.patterns.map((p) => (
          <div
            key={p.meant}
            style={{ padding: "9px 4px", borderBottom: `1px solid ${theme.border}`, fontSize: 14, color: theme.textStrong }}
          >
            {p.meant} → <b>{p.heard}</b>
            <div style={{ fontSize: 12, color: theme.textMuted }}>
              {p.count} of {p.opportunities} times · {Math.round(p.rate * 100)}%
            </div>
          </div>
        ))
      )}
    </>
  );
}

// Read some lines aloud and WhimprFlow's brain learns how you sound. The speech
// model isn't retrained: the brain learns which of your sounds come out
// differently, and uses that to fix words, including ones you never practised.
export function CalibrationPane({ onError }: { onError: (headline: string, detail: string) => void }) {
  const [stage, setStage] = useState<Stage>("intro");
  const [lines, setLines] = useState<Line[]>([]);
  const [index, setIndex] = useState(0);
  const [listening, setListening] = useState<"idle" | "listening" | "working">("idle");
  const [brain, setBrain] = useState<BrainOverview>(EMPTY_BRAIN);
  const [own, setOwn] = useState("");

  useEffect(() => {
    void getBrain().then(setBrain);
  }, []);

  const line = lines[index];

  const begin = async () => {
    const prompts = await getCalibrationPrompts(LINES, Math.floor(Math.random() * 2 ** 31));
    if (prompts.length === 0) {
      onError("Calibration unavailable", "Couldn't load the practice lines.");
      return;
    }
    setLines(prompts.map((p) => ({ tag: p.tag, text: p.text })));
    setIndex(0);
    setStage("reading");
  };

  const toggleMic = async () => {
    if (listening === "idle") {
      try {
        await calibrationRecordStart();
        setListening("listening");
      } catch (e) {
        onError("Microphone didn't start", String(e));
      }
    } else if (listening === "listening") {
      setListening("working");
      try {
        const heard = await calibrationRecordStop();
        setLines((ls) => ls.map((l, i) => (i === index ? { ...l, heard } : l)));
      } catch (e) {
        onError("Couldn't hear that", String(e));
      }
      setListening("idle");
    }
  };

  const finish = async () => {
    const attempts = lines
      .filter((l) => l.heard !== undefined)
      .map((l) => ({ expected: l.text, heard: l.heard as string }));
    try {
      setBrain(await learnFromCalibration(attempts));
      setStage("learned");
    } catch (e) {
      onError("Couldn't save what it learned", String(e));
    }
  };

  const next = () => (index >= lines.length - 1 ? void finish() : setIndex(index + 1));

  const forget = async () => {
    try {
      setBrain(await resetBrain());
    } catch (e) {
      onError("Couldn't forget", String(e));
    }
  };

  const addOwn = () => {
    const text = own.trim();
    if (!text) return;
    setLines((ls) => [...ls, { tag: "yours", text }]);
    setOwn("");
  };

  return (
    <div>
      <PageTitle sub="Read some lines aloud so WhimprFlow learns how you sound.">Your voice</PageTitle>

      {stage === "intro" && (
        <Card>
          <p style={{ margin: 0, color: theme.textBody, fontSize: 14, lineHeight: 1.55 }}>
            You'll read about {LINES} short lines from novels, poems, speeches and everyday talk, in your normal voice.
            WhimprFlow compares what you said with what it heard, and learns which of your sounds come out differently.
          </p>
          <p style={{ color: theme.textMuted, fontSize: 12.5 }}>
            It doesn't retrain the speech model. It learns your sound patterns, like a th that comes out as f, and uses
            them to pick the right words, including words you haven't practised.
          </p>
          <Button variant="accent" onClick={() => void begin()}>
            Start
          </Button>
          <div style={{ marginTop: 18, fontSize: 12.5, color: theme.textMuted }}>
            Learned from {brain.observations} {brain.observations === 1 ? "sentence" : "sentences"} · fixing{" "}
            {brain.learned_words} {brain.learned_words === 1 ? "word" : "words"} automatically
          </div>
          <Patterns brain={brain} />
          {brain.observations > 0 && (
            <div style={{ marginTop: 14 }}>
              <Button variant="ghost" size="sm" onClick={() => void forget()}>
                Forget everything it learned
              </Button>
            </div>
          )}
        </Card>
      )}

      {stage === "reading" && line && (
        <>
          <Card>
            <div style={{ fontSize: 12, color: theme.textMuted, marginBottom: 10 }}>
              Line {index + 1} of {lines.length} · {line.tag}
            </div>
            <div style={{ fontSize: 20, lineHeight: 1.45, color: theme.textStrong, fontFamily: font.ui }}>
              {line.text}
            </div>
            {line.heard !== undefined && (
              <div style={{ marginTop: 14, fontSize: 13.5, color: theme.textBody }}>
                <span style={{ color: theme.textMuted }}>Heard: </span>
                {line.heard || "Nothing"}
              </div>
            )}
            <div style={{ display: "flex", gap: 8, marginTop: 18 }}>
              {line.heard === undefined ? (
                <>
                  <Button variant="accent" disabled={listening === "working"} onClick={() => void toggleMic()}>
                    {listening === "idle" && "Tap, then read the line"}
                    {listening === "listening" && "Listening… tap when done"}
                    {listening === "working" && "Working…"}
                  </Button>
                  <Button variant="ghost" disabled={listening !== "idle"} onClick={next}>
                    Skip
                  </Button>
                </>
              ) : (
                <>
                  <Button variant="accent" onClick={next}>
                    {index >= lines.length - 1 ? "Finish" : "Next line"}
                  </Button>
                  <Button
                    variant="ghost"
                    onClick={() =>
                      setLines((ls) => ls.map((l, i) => (i === index ? { ...l, heard: undefined } : l)))
                    }
                  >
                    Say it again
                  </Button>
                </>
              )}
            </div>
          </Card>

          <Card style={{ marginTop: 14 }}>
            <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong, marginBottom: 8 }}>
              Your own words
            </div>
            <div style={{ display: "flex", gap: 8 }}>
              <input
                value={own}
                onChange={(e) => setOwn(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") addOwn();
                }}
                placeholder="A name or term you want to add"
                style={{
                  flex: 1,
                  background: theme.cardBgSubtle,
                  border: `1px solid ${theme.border}`,
                  borderRadius: 10,
                  padding: "9px 12px",
                  color: theme.textBody,
                  fontFamily: font.ui,
                  fontSize: 13.5,
                  outline: "none",
                }}
              />
              <Button variant="ghost" onClick={addOwn} disabled={!own.trim()}>
                Add
              </Button>
            </div>
            <div style={{ fontSize: 12, color: theme.textMuted, marginTop: 8 }}>
              They join the end of the list. Say the word on its own.
            </div>
            <div style={{ marginTop: 12 }}>
              <Button
                variant="ghost"
                size="sm"
                disabled={!lines.some((l) => l.heard !== undefined) || listening !== "idle"}
                onClick={() => void finish()}
              >
                Done early
              </Button>
            </div>
          </Card>
        </>
      )}

      {stage === "learned" && (
        <Card>
          <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
            Learned from {brain.observations} {brain.observations === 1 ? "sentence" : "sentences"} so far.
          </div>
          <Patterns brain={brain} />
          <p style={{ color: theme.textMuted, fontSize: 12.5 }}>
            Run this again any time; each run teaches it more.
          </p>
          <Button variant="ghost" onClick={() => setStage("intro")}>
            Done
          </Button>
        </Card>
      )}
    </div>
  );
}
