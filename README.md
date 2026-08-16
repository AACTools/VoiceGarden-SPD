# VoiceGarden-SPD

> A [speech-dispatcher](https://freebsoft.org/speechd) output module that connects **1300+ local sherpa-onnx voices** (Kokoro, Piper, Matcha, MMS, VITS) and **20 cloud engines** (Azure, credential-free Edge, OpenAI, ElevenLabs, Google, Cartesia, Deepgram, Watson, …) to every Linux application that speaks SSIP — Orca, Firefox, Okular, Foliate, Qt apps, Chromium, and anything using `libspeechd` or `spd-say`.

Powered by [rust-tts-wrapper](https://github.com/AACTools/rust-tts-wrapper). Linux sibling of [VoiceGarden-SAPI](https://github.com/AACTools/VoiceGarden-SAPI) (which does the same for Windows SAPI).

## What you get

- **Offline neural voices** — any sherpa-onnx model (Kokoro, Piper, Matcha, MMS, …) installed under `ModelsDir` appears as a system voice
- **Cloud voices** — 20 engines with credentials configured once in `engines.json` (see rust-tts-wrapper's catalogue; the crate's native `system` engine is deliberately **not** exposed here to avoid routing speech-dispatcher back into itself)
- **Streaming audio** — cloud PCM is handed to the speech-dispatcher server as chunks arrive from the network (rust-tts-wrapper decodes MP3 incrementally); sherpa-onnx synthesises whole clips by design, then streams them to the server
- **Word highlighting** — `<mark>` index marks (including speechd's own `__spd_N` pause marks) are mapped to engine word timings and reported in sync with playback: real timings for Azure/Edge/Google, estimated elsewhere. Verified end-to-end with a raw SSIP client (marks arrive as each word is spoken).
- **Stop/pause/resume** — STOP lands within ~`ChunkMs` of audio; PAUSE aborts and speechd re-speaks from the pause mark on resume (same behaviour as stock modules)
- **No root needed** — the installer puts everything under `~/.local` and `~/.config`

## Install

### One-liner (release tarball)

```bash
curl -fsSL https://raw.githubusercontent.com/AACTools/VoiceGarden-SPD/main/scripts/install.sh | sh
```

This downloads the latest x86_64 release, installs to `~/.local/libexec/speech-dispatcher-modules`, writes `~/.config/speech-dispatcher/modules/voicegarden-spd.conf`, and adds the `AddModule` line to `~/.config/speech-dispatcher/speechd.conf`. aarch64: build from source (below) for now; packaged builds are on the roadmap.

Then restart speech-dispatcher and test:

```bash
systemctl --user restart speech-dispatcher.socket   # or re-login
spd-say -o voicegarden-spd -L                       # list voices
spd-say -o voicegarden-spd "Hello from VoiceGarden"
```

### From source

Requires a C compiler (vendored protocol sources) and Rust ≥ 1.75; **runtime requires speech-dispatcher 0.12+** (Debian 13+, Ubuntu 25.04+, Fedora 41+, Arch, openSUSE Tumbleweed — server-side audio and the speak queue arrived in 0.12; on 0.11 distros the module loads but speech cannot play).

```bash
git clone https://github.com/AACTools/VoiceGarden-SPD
cd VoiceGarden-SPD
cargo build --release
./target/release/voicegarden-spd install           # installs + registers user-locally
```

## Management CLI

`voicegarden-spd` is the headless companion to the module (a GTK configuration app is on the roadmap and will sit on the same library calls):

```bash
voicegarden-spd status                    # installation + voice inventory
voicegarden-spd install [--models-dir DIR]
voicegarden-spd uninstall
voicegarden-spd refresh [--config FILE]   # fetch cloud voice lists (network)
voicegarden-spd voices                    # merged local + cloud voice list
voicegarden-spd speak <voice> "<text>"    # direct preview (bypasses speechd)
```

### Voices

**Local (sherpa-onnx).** Put models under `~/.rust-tts-wrapper/sherpaonnx/<model-id>/` (the layout used by VoiceGarden and rust-tts-wrapper tooling). Each model × speaker becomes a voice named `<model-id>#<speaker>`, e.g. `kokoro-en-v1.1-v0_19#1`.

**Cloud.** Write credentials to `~/.config/voicegarden-spd/engines.json` (mode 0600):

```json
{
  "azure": { "subscriptionKey": "...", "region": "uksouth" },
  "google": { "apiKey": "..." },
  "openai": { "apiKey": "sk-..." }
}
```

then run `voicegarden-spd refresh`. Cloud voices are named `<engine>/<voice-id>` (e.g. `edge/en-US-AriaNeural`). The module itself never touches the network — it reads the cache file at startup. `edge` needs no credentials and is always included.

Select a voice:

```bash
spd-say -o voicegarden-spd -y edge/en-US-AriaNeural "Testing edge"
spd-say -o voicegarden-spd -y kokoro-en-v1.1-v0_19#1 -l en "Local neural voice"
```

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

The speech-dispatcher module-protocol sources (`module_process.c`, `module_readline.c`, BSD-2-Clause) are vendored under `vendor/` and compiled in — no `libspeechd-dev` needed at build time, and the build works on any distro regardless of its speech-dispatcher version. We provide the `module_*` callbacks in Rust. Audio is routed through the speech-dispatcher server, so output-device selection, mixing, volume and pause all work exactly like stock modules.

## Configuration

See [`config/voicegarden-spd.conf`](config/voicegarden-spd.conf): `ModelsDir`, `CredentialsFile`, `VoiceCacheFile`, `DefaultVoice`, `ChunkMs`, `NumThreads`.

## Licences

**Code.** MIT (see [LICENSE](LICENSE)). The speech-dispatcher module-protocol translation units are vendored under `vendor/` under their upstream licences (see [`vendor/LICENSE.md`](crates/voicegarden-spd/vendor/LICENSE.md)).

**Sherpa-onnx models.** Model licences vary per model and are tracked in the [sherpa-onnx-tts-models](https://github.com/AACTools/sherpa-onnx-tts-models) registry (`license` / `license_url` on every entry; surfaced through rust-tts-wrapper's `SherpaModelInfo`). Common families: Piper voices are mostly MIT/Apache-2.0 (per-voice training datasets may carry additional terms), Kokoro is Apache-2.0, MMS models are CC-BY-NC 4.0 (**non-commercial**), Matcha examples are mostly MIT. Users are responsible for checking a model's licence before use — `voicegarden-spd voices` output and the model registry are the source of truth.

**Cloud engines.** Using a cloud voice sends your text to that provider; usage is subject to each provider's terms and privacy policy. Credentials are stored locally in `engines.json` (0600) and only leave the machine in API requests to the configured provider.

## Development

```bash
cargo test          # unit tests + fake-server protocol suite (no daemon needed)
cargo clippy --all-targets -- -D warnings
cargo fmt --all
```

CI (`.github/workflows/ci.yml`): rustfmt + clippy + tests, plus a **real-speechd smoke job** that registers the freshly built module with a real speech-dispatcher + PulseAudio null sink on the runner, checks LIST VOICES with a stub model, and synthesises a real Piper model end-to-end through `spd-say`.

### Verified against

- speech-dispatcher 0.12.1 (Debian): `spd-say -L`, wait-mode synthesis, mid-utterance STOP, PAUSE/RESUME round-trip, index-mark notifications to a raw SSIP client (Azure real word timings)
- Protocol conformance suite (`tests/fake_server.rs`): spawns the real binary and drives the actual `libspeechd_module` protocol code without a daemon

### Releases

Tags drive releases (`.github/workflows/release.yml`): the tag must match the workspace version in `Cargo.toml`; the build produces a tarball (`sd_voicegarden`, `voicegarden-spd-refresh`, `voicegarden-spd`, sample config, `install.sh`) attached to a GitHub Release, plus a stable `latest` tarball alias for the install one-liner.

```bash
# release process
$EDITOR Cargo.toml              # bump [workspace.package] version
git commit -am "chore: release v0.1.0"
git tag v0.1.0 && git push --tags
```

## Roadmap

- **GTK4/libadwaita configuration app** (VoiceGarden.UI's Linux counterpart): model browser/downloader with licence display, cloud credential editor, voice preview, engine toggles — on top of the same library calls the CLI uses
- aarch64 release builds
- Flatpak packaging of the config app (the module itself is inherently system-level: it must live outside the sandbox where speech-dispatcher can exec it)
- Opt-in punctuation/spelling mode mapping

## Known limitations (v0.1)

- Sherpa-ONNX synthesises whole clips before PCM flows (engine design), so its time-to-first-audio is inherent; cloud engines stream as bytes arrive.
- Engines whose APIs return one JSON document with base64 audio (Google, ElevenLabs `with-timestamps`) can only deliver after the response completes — API limitation, not buffering.
- Cloud PCM rates are declared per engine rather than signalled on the wire (24 kHz for Azure/Cartesia/Edge/OpenAI/Google — Google is pinned to 24 kHz server-side; 44.1 kHz for ElevenLabs defaults). Selecting non-default provider output formats via credentials can therefore play at the wrong speed.
- Estimated word timings (all engines except Azure/Edge/Google) arrive after synthesis, so their marks fire late on long utterances; real-timing engines report marks as audio plays.
- If a sherpa model directory passes the registry check but its files fail to load inside the C++ runtime, the utterance is silently dropped (BEGIN/END with no audio) and the failed engine stays cached until the module restarts.
- Sound icons are spoken as text (no icon file support).
- Punctuation/spelling/capital-letter modes are accepted but not applied.
