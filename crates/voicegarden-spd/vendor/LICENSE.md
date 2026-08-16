# Vendored speech-dispatcher module protocol sources

These files are vendored from speech-dispatcher 0.12.1
(https://github.com/brailcom/speechd, tag `0.12.1`) so the module builds
on any distro without requiring `libspeechd-dev` — several still-current
distros (e.g. Ubuntu 24.04) ship speech-dispatcher 0.11, which has no
`libspeechd_module.a`. Compiling these three translation units into the
binary is exactly what linking the distro archive did.

| File | Upstream path | Licence |
|------|---------------|---------|
| module_process.c | src/modules/module_process.c | BSD-2-Clause (Samuel Thibault) |
| module_readline.c | src/modules/module_readline.c | BSD-2-Clause (Samuel Thibault) |
| spd_module_main.h | src/modules/spd_module_main.h | BSD-2-Clause (Samuel Thibault) |
| spd_audio.h | src/modules/spd_audio.h | LGPL-2.1+ (Brailcom) — header only |
| spd_audio_plugin.h | include/spd_audio_plugin.h | LGPL-2.1+ (Brailcom) — header only |
| speechd_types.h | include/speechd_types.h | LGPL-2.1+ (Brailcom) — header only |

`module_main.c` (the `main()` entry) is deliberately NOT vendored — the
Rust binary provides its own `main()` mirroring it.

Runtime requirement: speech-dispatcher 0.12+ (server-side audio output
and the speak queue were introduced in 0.12).
