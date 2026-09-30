import { useEffect, useState } from "react";
import { theme } from "./theme";
import { Button, Card, PageTitle, Segmented } from "./ui";
import {
  getStyleSummaries,
  resetStyleProfile,
  type FormatMode,
  type Settings,
  type StyleProfileMode,
  type StyleSummary,
  EMPTY_STYLE_SUMMARIES,
} from "./api";

const DICTATION_MODES: { value: FormatMode; label: string; hint: string }[] = [
  { value: "auto", label: "Auto", hint: "Detected automatically from the app you're dictating into." },
  { value: "email", label: "Email", hint: "Always format as a well-structured email, whatever app you're in." },
  { value: "general", label: "General", hint: "Plain prose, no email/essay structure — overrides any app-based guess." },
  { value: "essay_report", label: "Essay/Report", hint: "Formal, structured writing: topic sentences, logical paragraphs." },
];

const STYLE_PROFILE_TABS: { value: StyleProfileMode; label: string }[] = [
  { value: "email", label: "Email" },
  { value: "general", label: "General" },
  { value: "essay_report", label: "Essay/Report" },
];

function SectionTitle({ children, sub }: { children: React.ReactNode; sub?: string }) {
  return (
    <div style={{ marginBottom: 14 }}>
      <div style={{ fontSize: 15, fontWeight: 600, color: theme.textStrong }}>{children}</div>
      {sub && <div style={{ color: theme.textMuted, fontSize: 13, marginTop: 4 }}>{sub}</div>}
    </div>
  );
}

function Tag({ children }: { children: React.ReactNode }) {
  return (
    <span
      style={{
        display: "inline-block",
        background: theme.cardBgSubtle,
        border: `1px solid ${theme.border}`,
        borderRadius: 999,
        padding: "5px 12px",
        fontSize: 12.5,
        color: theme.textBody,
        marginRight: 8,
        marginBottom: 8,
      }}
    >
      {children}
    </span>
  );
}

function ProgressBar({ value }: { value: number }) {
  return (
    <div
      style={{
        height: 8,
        borderRadius: 999,
        background: theme.cardBgSubtle,
        border: `1px solid ${theme.border}`,
        overflow: "hidden",
      }}
    >
      <div
        style={{
          height: "100%",
          width: `${Math.round(Math.min(1, Math.max(0, value)) * 100)}%`,
          background: theme.accent,
          transition: "width 300ms ease",
        }}
      />
    </div>
  );
}

export function StylePane({
  settings,
  onChange,
  onError,
}: {
  settings: Settings;
  onChange: (s: Settings) => void;
  onError: (headline: string, detail: string) => void;
}) {
  const [summaries, setSummaries] = useState(EMPTY_STYLE_SUMMARIES);
  const [viewMode, setViewMode] = useState<StyleProfileMode>("email");
  const [resetting, setResetting] = useState(false);

  const load = () => getStyleSummaries().then(setSummaries);
  useEffect(() => {
    void load();
  }, []);

  const summary: StyleSummary = summaries[viewMode];

  const reset = async () => {
    setResetting(true);
    try {
      await resetStyleProfile(viewMode);
      await load();
    } catch {
      onError("Couldn't reset", "Your style profile wasn't cleared — try again.");
    } finally {
      setResetting(false);
    }
  };

  const progress = summary.min_samples_to_apply
    ? summary.samples / summary.min_samples_to_apply
    : 0;

  return (
    <div style={{ maxWidth: 720 }}>
      <PageTitle>Style</PageTitle>

      <Card style={{ marginBottom: 16 }}>
        <SectionTitle sub="How dictation is shaped for the medium — Auto detects it from the app you're dictating into; a manual choice always overrides that guess.">
          Dictation mode
        </SectionTitle>
        <Segmented
          full
          options={DICTATION_MODES.map((m) => ({ value: m.value, label: m.label }))}
          value={settings.format_mode}
          onChange={(v) => onChange({ ...settings, format_mode: v })}
        />
        <div style={{ color: theme.textMuted, fontSize: 12.5, marginTop: 10 }}>
          {DICTATION_MODES.find((m) => m.value === settings.format_mode)?.hint}
        </div>
      </Card>

      <Card style={{ marginBottom: 16 }}>
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
          <div>
            <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
              Learn my writing voice
            </div>
            <div style={{ color: theme.textMuted, fontSize: 12.5, marginTop: 4, maxWidth: 480 }}>
              Passively learns your sentence length, tone, and favorite phrases from what you
              dictate — separately for Email, General, and Essay/Report — then nudges cleanup to
              sound like you. Runs entirely on-device — adds no extra API calls, just a little
              more context on the cleanup request you're already sending.
            </div>
          </div>
          <Segmented
            options={[
              { value: "on", label: "On" },
              { value: "off", label: "Off" },
            ]}
            value={settings.style_learning_enabled ? "on" : "off"}
            onChange={(v) => onChange({ ...settings, style_learning_enabled: v === "on" })}
          />
        </div>
      </Card>

      <div style={{ marginBottom: 16 }}>
        <Segmented
          full
          options={STYLE_PROFILE_TABS}
          value={viewMode}
          onChange={setViewMode}
        />
      </div>

      {!summary.ready ? (
        <Card>
          <SectionTitle sub={`A few more ${STYLE_PROFILE_TABS.find((t) => t.value === viewMode)?.label} dictations and it'll start shaping cleanup.`}>
            Still learning
          </SectionTitle>
          <div style={{ marginBottom: 8, fontSize: 13, color: theme.textMuted }}>
            {summary.samples} of {summary.min_samples_to_apply} dictations
          </div>
          <ProgressBar value={progress} />
        </Card>
      ) : (
        <>
          <Card style={{ marginBottom: 16 }}>
            <SectionTitle sub={`Learned from ${summary.samples} ${STYLE_PROFILE_TABS.find((t) => t.value === viewMode)?.label} dictations.`}>
              Your voice, so far
            </SectionTitle>
            <div style={{ display: "flex", flexDirection: "column", gap: 10, fontSize: 13.5, color: theme.textBody }}>
              <div>
                <b>Tone:</b> {summary.tone_label}
              </div>
              <div>
                <b>Average sentence length:</b> ~{Math.round(summary.avg_sentence_words)} words
              </div>
              <div>
                <b>Contraction use:</b> {Math.round(summary.contraction_rate * 100)}%
              </div>
            </div>
          </Card>

          {summary.top_phrases.length > 0 && (
            <Card style={{ marginBottom: 16 }}>
              <SectionTitle sub="Phrases that showed up often enough to be part of your voice.">
                Frequent phrases
              </SectionTitle>
              <div>
                {summary.top_phrases.map(([phrase]) => (
                  <Tag key={phrase}>{phrase}</Tag>
                ))}
              </div>
            </Card>
          )}

          {summary.top_fillers.length > 0 && (
            <Card style={{ marginBottom: 16 }}>
              <SectionTitle>Natural fillers</SectionTitle>
              <div>
                {summary.top_fillers.map(([word]) => (
                  <Tag key={word}>{word}</Tag>
                ))}
              </div>
            </Card>
          )}
        </>
      )}

      <Card>
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
          <div>
            <div style={{ fontSize: 14, fontWeight: 600, color: theme.textStrong }}>
              Reset {STYLE_PROFILE_TABS.find((t) => t.value === viewMode)?.label} profile
            </div>
            <div style={{ color: theme.textMuted, fontSize: 12.5, marginTop: 4 }}>
              Starts this mode's learning from scratch. The other two are unaffected. Can't be
              undone.
            </div>
          </div>
          <Button variant="ghost" onClick={() => void reset()} disabled={resetting}>
            {resetting ? "Resetting…" : "Reset"}
          </Button>
        </div>
      </Card>
    </div>
  );
}
