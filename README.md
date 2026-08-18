# VoiceGarden-SPD

> A [speech-dispatcher](https://freebsoft.org/speechd) output module that connects **1300+ local sherpa-onnx voices** (Kokoro, Piper, Matcha, MMS, VITS) and **20 cloud engines** (Azure, credential-free Edge, OpenAI, ElevenLabs, Google, Cartesia, Deepgram, Watson, …) to every Linux application that speaks SSIP — Orca, Firefox, Okular, Foliate, Qt apps, Chromium, and anything using `libspeechd` or `spd-say`.

Powered by [rust-tts-wrapper](https://github.com/AACTools/rust-tts-wrapper). Linux sibling of [VoiceGarden-SAPI](https://github.com/AACTools/VoiceGarden-SAPI) (which does the same for Windows SAPI).

## What you get

- **Offline neural voices** — any sherpa-onnx model (Kokoro, Piper, Matcha, MMS, …) installed under `ModelsDir` appears as a system voice
- **Cloud voices** — 20 engines with credentials configured once in `engines.json` (see rust-tts-wrapper's catalogue; the crate's native `system` engine is deliberately **not** exposed here to avoid routing speech-dispatcher back into itself)
- **Streaming audio** — cloud PCM is handed to the speech-dispatcher server as chunks arrive from the network (rust-tts-wrapper decodes MP3 incrementally); sherpa-onnx models stream sentence batches as each sentence finishes synthesising
- **Word highlighting** — `<mark>` index marks (including speechd's own `__spd_N` pause marks) are mapped to engine word timings and reported in sync with playback: real timings for Azure/Edge/Google, progressively-anchored estimates elsewhere. Verified end-to-end with a raw SSIP client (marks arrive as each word is spoken).
- **Stop/pause/resume** — STOP lands within ~`ChunkMs` of audio; PAUSE aborts and speechd re-speaks from the pause mark on resume (same behaviour as stock modules)
- **SSML passthrough** — clients that enable SSML mode get their markup delivered to SSML-capable engines (Azure, Edge, Google): `<prosody>`, `<break>`, `<say-as>`, `<sub>` etc. all work, and SpeechMarkdown converts inside rust-tts-wrapper. `<mark>` tags are timed by the module itself either way. Envelopes are normalised on the way through — speech-dispatcher's bare `<speak>` wrapper (and SSML missing `version`/`xmlns`/`xml:lang`) is upgraded to a full document, since Edge/Azure silently return no audio otherwise.
- **Failures are reported, never silent** — an utterance the engine can't synthesise (or that yields no audio) reaches speech-dispatcher as `301 ERROR CANT SPEAK`, so the daemon logs it and clients can tell, instead of a success-shaped silence.
- **Accessibility modes** — punctuation announcement (`some`/`most`/`all`), spelling mode, and capital-letter recognition. Spelling uses native SSML `<say-as interpret-as="characters">` on SSML-capable engines (Azure/Edge/Google) and a text approximation elsewhere; punctuation and capitals are applied as text preprocessing (the engines don't implement them natively)
- **Sound icons** — `SOUND_ICON` messages play the named file from `SoundIconFolder` (Debian's `sound-icons` package provides the standard set), falling back to speaking the icon name
- **No root needed** — the installer puts everything under `~/.local` and `~/.config`

## Install

**Requires speech-dispatcher 0.12+** (Debian 13+, Ubuntu 25.04+, Fedora 41+, Arch, openSUSE Tumbleweed — Ubuntu 24.04's stock `0.12.0-rc2` also works). On older releases the module loads but speech cannot play — `voicegarden-spd doctor` explains if you hit this.

### One-liner (recommmended)

```bash
curl -fsSL https://raw.githubusercontent.com/AACTools/VoiceGarden-SPD/main/scripts/install.sh | sh
```

With root this installs the native **.deb/.rpm** from the latest release (package-manager upgrades + clean removal, `--user` forces the no-root path instead). x86_64 and aarch64 (Raspberry Pi 4/5, ARM servers) are prebuilt. Then:

```bash
voicegarden-spd doctor    # verifies setup end-to-end, explains any problem
spd-say -o voicegarden-spd "Hello from VoiceGarden"
```

### Distro packages

| Distro | Install |
|---|---|
| Debian/Ubuntu (0.12+ distros) | download the `.deb` from [releases](https://github.com/AACTools/VoiceGarden-SPD/releases), then `sudo apt install ./voicegarden-spd_*_amd64.deb` (or `_arm64.deb`) |
| Fedora / openSUSE | download the `.rpm`, then `sudo dnf install ./voicegarden-spd-*.rpm` |
| Arch | AUR: `voicegarden-spd-bin` (release) / `voicegarden-spd-git` (PKGBUILDs in [`packaging/aur/`](packaging/aur)) |

The packages install the module into speech-dispatcher's system module directory, where the daemon auto-detects it (no `speechd.conf` edit). If your `/etc/speech-dispatcher/speechd.conf` already registers modules with `AddModule` lines, an `AddModule "voicegarden-spd"` line is added in the same style instead. Installation warns if speech-dispatcher is older than 0.12.

### User-local (no root)

```bash
curl -fsSL .../install.sh | sh -s -- --user
# or from a checkout:
./target/release/voicegarden-spd install
```

Installs to `~/.local/libexec/speech-dispatcher-modules/sd_voicegarden-spd` and writes `~/.config/speech-dispatcher/modules/voicegarden-spd.conf`. speech-dispatcher **auto-detects** the module from its user module directory (the `voicegarden-spd` name comes from the binary), so nothing is written to your `speechd.conf` — a user `speechd.conf` containing only an `AddModule` line would disable auto-detection and drop every other output module from the session. If you maintain a user `speechd.conf` that already lists other modules, the installer manages an explicit `AddModule` line in it instead. The daemon is restarted automatically (`--no-restart` to skip).

### From source

Requires a C compiler (vendored protocol sources) and Rust ≥ 1.75.

```bash
git clone https://github.com/AACTools/VoiceGarden-SPD
cd VoiceGarden-SPD
cargo build --release
./target/release/voicegarden-spd install           # installs + registers user-locally
```

## Management CLI

`voicegarden-spd` configures and manages everything without a GUI.

```bash
voicegarden-spd status                    # installation + voice inventory
voicegarden-spd doctor                    # diagnose a broken setup
voicegarden-spd install [--models-dir DIR] [--no-restart]
voicegarden-spd uninstall [--no-restart]
voicegarden-spd refresh [ENGINES…]        # fetch cloud voice lists (network)
voicegarden-spd bench <voice> [text] [N]  # cold/warm synthesis timings
voicegarden-spd migrate-models            # move legacy model dirs to the primary path
```

### Engines

```bash
voicegarden-spd engine list               # every engine: credential status + voice counts
voicegarden-spd engine add azure          # interactive: prompt for keys (hidden), verify live, save 0600, refresh
voicegarden-spd engine add google --set apiKey=…   # non-interactive
voicegarden-spd engine test azure         # live credential check ("ok azure: … 1638 ms, 556 voices")
voicegarden-spd engine show google        # masked credentials + cache state
voicegarden-spd engine remove openai      # drop credentials + cached voices
```

`engine add` verifies credentials against the provider **before** saving (bad keys abort with nothing written; `--force` overrides) and refreshes just that engine's voice cache afterwards.

### Voice search

```bash
voicegarden-spd voice search sonia --source cloud --lang en-GB
voicegarden-spd voice search --source local --quality high
voicegarden-spd voice search --gender female --engine edge --lang en
voicegarden-spd voice search --multilingual
voicegarden-spd voice search kokoro --json          # machine-readable (no credentials)
voicegarden-spd voice info azure/en-GB-SoniaNeural  # full detail
voicegarden-spd voice test edge/en-GB-RyanNeural "Hello"   # hear it (no speechd)
```

Filters: `--source local|cloud` (offline/online), `--engine`, `--lang` (base `en` matches `en-US`), `--gender`, `--quality` (local models), `--multilingual`, plus free-text terms. Quality/gender/multilingual come from the sherpa registry and provider voice lists; multilingual cloud voices (e.g. Azure's `…MultilingualNeural`) are detected automatically.

### Model registry

```bash
voicegarden-spd model find --lang nl --quality high     # all 1300+ models, incl. not-installed
voicegarden-spd model find kokoro --multilingual        # ● marks installed; sizes + licences
```

`model find` prints each match's download URL and warns about **fp16 archives** (they abort in the CPU ONNX runtime this build links).

### Voices

**Local (sherpa-onnx).** Models live in `~/.local/share/voicegarden/sherpa-onnx-models/<model-id>/` — one directory per model. The legacy rust-tts-wrapper layout (`~/.rust-tts-wrapper/sherpaonnx`) is still scanned as a fallback so existing installs keep working, and `voicegarden-spd migrate-models` moves them over. Each model × speaker becomes a voice named `<model-id>#<speaker>`, e.g. `kokoro-en-v1.1-v0_19#1`.

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
  sd_voicegarden-spd        ← this repo
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

## Performance

Engine instances — and for sherpa-onnx, the loaded ONNX model — are cached for the module's lifetime. Only the **first utterance per model** pays the cold-start cost (model load + first inference); every subsequent utterance through the same voice is warm. Cloud engines pay one network round trip per utterance regardless.

Measure it yourself:

```bash
voicegarden-spd bench piper-nl_BE-rdh-low#0 "The quick brown fox jumps over the lazy dog." 5
```

CI runs the same bench on every push (informational; see the smoke job log). With `LOGLEVEL` set to 4+ in `speechd.conf`, the module logs the exact text each engine receives — handy when debugging preprocessing/passthrough.

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

Tags drive releases (`.github/workflows/release.yml`): the tag must match the workspace version in `Cargo.toml`; each release publishes, for x86_64 and aarch64, a tarball + `.deb` + `.rpm` (plus a stable `latest` tarball alias for the install one-liner). A package smoke job installs the freshly built `.deb` on a clean runner and synthesises through a real speech-dispatcher before anything publishes. See [`docs/PUBLISHING.md`](docs/PUBLISHING.md) for the release process and AUR publishing steps.

```bash
# release process
$EDITOR Cargo.toml              # bump [workspace.package] version
git commit -am "chore: release v0.2.1"
git tag v0.2.1 && git push --tags
```

## Roadmap

- **`model install <id>`** — download + verify + extract a registry model into the models dir straight from `model find` (until then the URL is printed; fp16 archives are flagged)
- Real-hardware aarch64 verification (Raspberry Pi)
- Debian/Fedora official-repo packaging once there's a track record
- Opt-in punctuation/spelling mode localisation tables

## Known limitations

- **Local sherpa-onnx models stream per sentence batch** (first sentence's audio starts playing while later sentences synthesise); a *single-sentence* utterance still completes synthesis before its audio flows — inherent to sentence-batched generation. Cloud engines stream as bytes arrive; engines whose APIs return one JSON document with base64 audio (Google, ElevenLabs `with-timestamps`) deliver only after the response completes — an API limitation.
- **Cloud PCM rates are declared, not signalled.** rust-tts-wrapper delivers PCM16 mono through `on_audio` without a rate, so the module supplies the provider's fixed rate (24 kHz for Azure/Cartesia/Edge/OpenAI/…, 44.1 kHz for ElevenLabs). Non-default provider output formats could therefore play at the wrong speed.
- **Estimated word timings** (all engines except Azure/Edge/Google) fire progressively, anchored to delivered audio — accurate pacing, but the estimate itself assumes ~150 wpm, so word positions can drift within a sentence on unusually fast/slow voices. Real-timing engines report exact positions.
- If a sherpa model directory matches a registry id but its files fail to load inside the C++ runtime (incomplete download, corrupt archive, or an unsupported variant such as fp16), every utterance through it fails with `301 ERROR CANT SPEAK` (visible in the daemon log) — **re-running does not help**; it fails deterministically until the model is fixed or removed. `voicegarden-spd doctor` validates every installed model (load + synthesis, crash-isolated) and tells you exactly which one is broken; `model find` flags fp16 archives before you download them.
- Punctuation/spelling/capital expansions are English wordings ("comma", "period"); localisation would need per-language tables.
- SSML passthrough ignores the SSIP rate/pitch/volume parameters for the utterance (prosody in the markup wins) — plain-text speech applies them as before.
