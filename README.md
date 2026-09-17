# Kubuno STT

Self-hosted **speech-to-text** for the [Kubuno](https://github.com/kubuno) platform.
STT turns spoken audio into text entirely on your own server — nothing is sent to
a third party. It powers the platform's voice search and is available to any
module that needs dictation or transcription.

It is a standard Kubuno **module**: a separate process that registers with the
core at startup, and which the core reverse-proxies. It ships as a single
self-contained `.kbpkg` — identical on Linux, Windows and macOS.

## Engine

STT runs on **Whisper** ([whisper.cpp](https://github.com/ggerganov/whisper.cpp)
via [`whisper-rs`](https://crates.io/crates/whisper-rs)), compiled from source and
**linked statically** into the binary. There is therefore **no external native
library to install** on the host — the reason the module can be distributed as a
self-contained package on every operating system.

Whisper is **multilingual**: a single model serves every language (the spoken
language is selected, or auto-detected, at inference time). Models are downloaded
on demand from the admin panel and stored in the module's writable data
directory; audio never leaves the server.

Supported languages include English, French, Spanish, Portuguese, Italian,
German, Greek, Russian, Arabic, Hebrew, Hindi, Chinese and Japanese.

## HTTP API

All routes are reached through the core proxy under `/api/v1/stt`.

| Method | Path | Purpose |
| --- | --- | --- |
| `GET`  | `/health` | Liveness probe. |
| `GET`  | `/status?lang=<code>` | Whether recognition is enabled (globally and per language) and the capture settings the client should use. |
| `POST` | `/transcribe?lang=<code>` | Batch transcription of a WAV body → `{ "text": … }`. |
| `GET`  | `/stream?lang=<code>&rate=<hz>` | Real-time transcription over a WebSocket (binary PCM in, JSON partial/final out). |

Admin routes (role-gated) manage the global switch, per-language configuration,
capture settings and the model catalogue (list / download / delete).

## Configuration & options

Per language, an administrator can set the model, an initial prompt (vocabulary
and spelling bias), punctuation and number normalisation, translation to English,
beam size (accuracy vs. speed) and automatic language detection. Global capture
settings cover the silence auto-stop, the sound threshold and an optional
profanity filter. Configuration and downloaded models live in the module's data
directory and survive upgrades.

## Install

STT is distributed only as a `.kbpkg` (no system packages). The Kubuno server
installs it itself — from the admin console, or offline from the command line:

```bash
sudo kubuno modules:install stt-<version>-<os>-<arch>.kbpkg
sudo systemctl restart kubuno
```

## Build from source

Building requires a C/C++ toolchain and **CMake** (whisper.cpp is compiled from
source), plus the Rust toolchain:

```bash
bash build_kbpkg.sh            # → dist/stt-<version>-<os>-<arch>.kbpkg
bash build_kbpkg.sh --install  # build, install into the local store, restart
```

## License

Licensed under the **GNU Affero General Public License v3.0** — see [LICENSE](LICENSE).
