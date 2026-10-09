# Speech and cleanup models

There are two *separate* models. **You only ever need the first one**, and WhimprFlow
downloads it for you.

| Model | What it's for | Required? |
|---|---|---|
| Whisper (`ggml-*.bin`) | Speech-to-text: turns your voice into a raw transcript | **Yes** |
| Qwen (`*.gguf`) | Optional *local, offline* cleanup pass (fillers, punctuation, self-corrections) | **No**. Skip it and either use the cloud Cleanup Engine (OpenAI/Anthropic/OpenRouter key in Settings) or set Auto Cleanup to **None** for raw, verbatim text. |

If dictation types nothing at all, that's not a missing model problem; see the Troubleshooting
section in the README first (Accessibility permission on macOS, or the loud in-app error the
pill/Hub shows).

## 1. Speech-to-text (Whisper): required

**On macOS, WhimprFlow downloads it for you.** On first run, the setup screen offers two models;
you can add or switch later under **Settings ▸ Speech model**.

| Model | File | Size | Good for |
|---|---|---|---|
| **Fast** | `ggml-base.en.bin` | 148 MB | Ready in about a minute; quick on any Mac. The default. |
| **Best** | `ggml-large-v3-turbo.bin` | 1.62 GB | The most accurate, especially with names and unusual words. About 5× slower to transcribe than Fast, but still under half a second for a long sentence on Apple Silicon. |

What the downloader does for you:

- Saves to `~/Library/Application Support/WhimprFlow/models/`.
- Writes to a `.part` file first. If your connection drops, you cancel, or the app quits, the
  next try continues where it left off instead of starting over.
- Checks the finished file against its published SHA-256 before using it. A file that doesn't
  match is deleted, never loaded.
- Starts using the new model straight away, with no restart. A dictation already in progress
  finishes on the model it started with.
- Never re-downloads a model that is already there.

### Parakeet (experimental, optional)

An alternative recogniser from NVIDIA, available under **Settings ▸ Speech model** (about 660 MB,
four files in `models/parakeet-tdt-0.6b-v2-int8/`). It runs on the CPU, so it needs no GPU.
On Apple Silicon it is roughly twice as fast as the Best whisper model and adds its own
punctuation and capitalisation. Trade-offs, which is why it is opt-in:

- **It can't use your dictionary to hear names.** Whisper is told your words in advance;
  Parakeet isn't, so "Manvi" may come out as "man V". The "also heard as" entries and the
  learned voice patterns still fix such mishears afterwards.
- It keeps filler words ("um", "uh") that whisper drops; Cleanup removes them.
- It can occasionally return nothing for a very short, abruptly-started clip. When the voice
  detector heard real speech but Parakeet returned nothing, WhimprFlow retries once on the
  whole recording.
- English only.

Download it, then press **Use**. Switching takes a few seconds in the background, and **Use** on
a whisper model switches back. If Parakeet can't load, WhimprFlow falls back to whisper instead
of leaving you without dictation.

Model: NVIDIA Parakeet TDT 0.6B v2, ONNX export by istupakov, licensed
[CC-BY-4.0](https://creativecommons.org/licenses/by/4.0/); source
<https://huggingface.co/istupakov/parakeet-tdt-0.6b-v2-onnx> (revision `0bbb45a3`).

If more than one whisper model is installed, the most accurate present one is used, in this order:
`ggml-large-v3-turbo.bin`, `ggml-medium.en.bin`, `ggml-small.en.bin`, `ggml-base.en.bin`.

### Placing a model by hand (if you're offline)

Download one file and save it with its exact name:

```
https://huggingface.co/ggerganov/whisper.cpp/resolve/main/<exact filename>
```

| File (exact name) | Size | SHA-256 |
|---|---|---|
| `ggml-base.en.bin` | 147,964,211 bytes | `a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002` |
| `ggml-large-v3-turbo.bin` | 1,624,555,275 bytes | `1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69` |
| `ggml-medium.en.bin` | 1.53 GB | not downloadable in-app; recognised if you place it |
| `ggml-small.en.bin` | 488 MB | not downloadable in-app; recognised if you place it |

Place it at `~/Library/Application Support/WhimprFlow/models/`. Any other filename in that repo (quantized
`-q5_1`/`-q8_0` variants, multilingual models, CoreML bundles, `tiny.en`) is **not** recognised
today: renaming one won't work, because the file format has to match too.

To check a file yourself: `shasum -a 256 <file>` should print the hash above.

## 2. Local cleanup LLM (Qwen, GGUF) — optional

Skip this section unless you specifically want cleanup to run fully
on-device/offline instead of via a cloud API key. Everything else in
WhimprFlow (recording, transcription, pasting) works without it.

Download **one** file, then **rename it to the exact lowercase name shown**
(Hugging Face's own filenames use mixed case; WhimprFlow's default is
case-sensitive on Linux even though macOS/Windows aren't, so renaming avoids
any ambiguity), and place it in the same models folder as the Whisper model
above.

**Smaller / faster — `qwen2.5-1.5b-instruct-q4_k_m.gguf` (986 MB):**

```
https://huggingface.co/bartowski/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/Qwen2.5-1.5B-Instruct-Q4_K_M.gguf
```

**Better quality, needs more RAM — `qwen3-4b-instruct-2507-q4_k_m.gguf` (2.5 GB):**

```
https://huggingface.co/unsloth/Qwen3-4B-Instruct-2507-GGUF/resolve/main/Qwen3-4B-Instruct-2507-Q4_K_M.gguf
```

If both are present, WhimprFlow prefers the 4B one. Then in the Hub, set
**Settings → Cleanup Engine → Local**.

## Checking it worked

Launch WhimprFlow from a terminal (`./dev.sh`). You should see:

```
[whimpr] ASR model ggml-base.en.bin loaded and warmed (… ms) — ready to transcribe
```

at startup. If instead you see `ASR model not found at <path>`, no speech model is installed:
open **Settings ▸ Speech model** and download one, or check that a hand-placed file's name and
folder match exactly what's listed above.
