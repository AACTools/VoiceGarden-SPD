# VoiceGarden-SPD

> A [speech-dispatcher](https://freebsoft.org/speechd) output module that connects **1300+ local sherpa-onnx voices** (Kokoro, Piper, Matcha, MMS, VITS) and **20 cloud engines** (Azure, credential-free Edge, OpenAI, ElevenLabs, Google, Cartesia, Deepgram, Polly, Watson, …) to every Linux application that speaks SSIP — Orca, Firefox, Okular, Foliate, Qt apps, Chromium, and anything using `libspeechd` or `spd-say`.

Powered by [rust-tts-wrapper](https://github.com/AACTools/rust-tts-wrapper). Linux sibling of [VoiceGarden-SAPI](https://github.com/AACTools/VoiceGarden-SAPI) (which does the same for Windows SAPI).

## What you get

- **Offline neural voices** — any sherpa-onnx model (Kokoro, Piper, Matcha, MMS, …) installed under `ModelsDir` appears as a system voice
- **Cloud voices** — 20 engines with credentials configured once in `engines.json` (see rust-tts-wrapper's catalogue; the crate's native `system` engine is deliberately **not** exposed here to avoid routing speech-dispatcher back into itself)
- **Streaming audio** — PCM is handed to the speech-dispatcher server as chunks arrive from the engine, so time-to-first-audio tracks the engine, not the whole clip
- **Word highlighting** — `<mark>` index marks (including speechd's own `__spd_N` pause marks) are mapped to engine word timings and reported in sync with playback: real timings for Azure/Edge/Google, estimated elsewhere
- **Stop/pause** — honoured mid-utterance with ~`ChunkMs` latency; audio is routed through the speech-dispatcher server, so pause/resume, mixing and output device selection all work like any stock module

## Architecture

```
Orca / Firefox / Okular / Qt apps / spd-say
        │ SSIP
  speech-dispatcher (unmodified)
        │ module protocol (stdin/stdout)
  sd_voicegarden            ← this repo
        │ rust-tts-wrapper (Rust API, no C ABI)
        ├─ sherpa-onnx (local models, PCM16 direct)
        └─ cloud engines (crate decodes MP3→PCM16 mono incrementally)
        │
        └─ PCM16 chunks + index marks → server speak queue (HDLC-escaped)
```

The module links `libspeechd_module` (from `libspeechd-dev`) statically: the library implements the protocol parsing, audio streaming and event replies; we provide the `module_*` callbacks in Rust.

## Verified against

- speech-dispatcher 0.12.1 (Debian): `spd-say -o voicegarden-spd -L`, synthesis with wait-for-completion, and mid-utterance STOP via `spd-say -S`
- Protocol conformance suite (`tests/fake_server.rs`): spawns the real binary and drives the actual `libspeechd_module` protocol code without a daemon
- Real-model synthesis: LIST VOICES → SPEAK → 705 audio chunks → 700 INDEX MARK → 702 END, all observed on the wire

## Install

### From source

```bash
sudo apt install libspeechd-dev libclang-dev   # Debian/Ubuntu
cargo build --release
sudo install -Dm755 target/release/sd_voicegarden /usr/lib/x86_64-linux-gnu/speech-dispatcher-modules/sd_voicegarden
sudo install -Dm644 config/voicegarden-spd.conf /etc/speech-dispatcher/modules/voicegarden-spd.conf
```

Enable it in `~/.config/speech-dispatcher/speechd.conf` (or the system file):

```
AddModule "voicegarden-spd" "sd_voicegarden" "voicegarden-spd.conf"
```

Restart speech-dispatcher (`spd-say -L` or re-login) and check:

```bash
spd-say -O                    # list output modules
spd-say -o voicegarden-spd -L # list voices
spd-say -o voicegarden-spd "Hello from VoiceGarden"
```

### Voices

**Local (sherpa-onnx).** Put models under `~/.rust-tts-wrapper/sherpaonnx/<model-id>/` (the layout used by VoiceGarden and rust-tts-wrapper). Each model × speaker becomes a voice named `<model-id>#<speaker>`, e.g. `kokoro-en-v1.1-v0_19#1`.

**Cloud.** Write credentials to `~/.config/voicegarden-spd/engines.json` (mode 0600):

```json
{
  "azure": { "subscriptionKey": "...", "region": "uksouth" },
  "openai": { "apiKey": "sk-..." }
}
```

then refresh the voice cache:

```bash
voicegarden-spd-refresh           # or: voicegarden-spd-refresh --config /path/to/voicegarden-spd.conf
```

Cloud voices are named `<engine>/<voice-id>` (e.g. `edge/en-US-AriaNeural`). The module itself never touches the network — it reads the cache file at startup. `edge` needs no credentials and is always included.

Select a voice system-wide:

```bash
spd-say -o voicegarden-spd -y edge/en-US-AriaNeural "Testing edge"
```

## Configuration

See [`config/voicegarden-spd.conf`](config/voicegarden-spd.conf): `ModelsDir`, `CredentialsFile`, `VoiceCacheFile`, `DefaultVoice`, `ChunkMs`, `NumThreads`.

## Development

```bash
cargo test          # unit tests + fake-server protocol tests (no daemon needed)
cargo clippy --all-targets -- -D warnings
```

The fake-server test spawns the real binary and drives the actual
`libspeechd_module` protocol code, so protocol regressions surface in CI
without a speechd daemon.

### Known limitations (v0.1)

- **Cloud sample rates are declared, not signalled.** rust-tts-wrapper delivers PCM16 mono through `on_audio` without a rate, so the module supplies the provider's fixed rate (24 kHz for Azure/Cartesia/Edge/OpenAI/…, 44.1 kHz for ElevenLabs). Non-default provider output formats could therefore play at the wrong speed.
- Sherpa-ONNX synthesises whole clips before PCM flows (engine design), so its time-to-first-audio is inherent; cloud engines stream as bytes arrive.
- Engines whose APIs return one JSON document with base64 audio (Google, ElevenLabs `with-timestamps`) can only deliver after the response completes.
- Estimated word timings (all engines except Azure/Edge/Google) arrive after synthesis, so their marks fire late on long utterances; real-timing engines report marks as audio plays.
- If a sherpa model directory passes the registry check but its files fail to load inside the C++ runtime, the utterance is silently dropped (BEGIN/END with no audio) and the engine instance stays cached.
- Sound icons are spoken as text (no icon file support).
- Punctuation/spelling/capital-letter modes are accepted but not applied.

## License

MIT. The built module statically links `libspeechd_module` (BSD-2-Clause, from `libspeechd-dev`).
