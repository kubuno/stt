<!--
  SPDX-FileCopyrightText: 2026 Kubuno contributors
  SPDX-License-Identifier: AGPL-3.0-or-later
-->

<div align="center">

<img src="https://raw.githubusercontent.com/kubuno/core/main/.github/logo.png" alt="Kubuno logo" width="120">

# Kubuno — Speech-to-Text

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/Rust-edition_2021-orange.svg)
![Whisper](https://img.shields.io/badge/engine-Whisper-8A2BE2.svg)
![Status](https://img.shields.io/badge/status-alpha-yellow.svg)

**Self-hosted speech-to-text for [Kubuno](https://github.com/kubuno/core) — the self-hosted, libre (AGPLv3) cloud platform, a sovereign alternative to Google Workspace and Microsoft 365.**

STT turns spoken audio into text entirely on your own server — nothing is sent to a third party. It powers the platform's voice search and is available to any module that needs dictation or transcription.

</div>

---

## Features

- **On-device transcription** — audio is transcribed on your server and never leaves it; there is no third-party service and no cloud API key.
- **Multilingual with one model** — a single model serves every language, with the spoken language selected or auto-detected at inference time. Supported languages include English, French, Spanish, Portuguese, Italian, German, Greek, Russian, Arabic, Hebrew, Hindi, Chinese and Japanese.
- **Batch and real-time** — transcribe a complete audio clip in one request, or stream microphone audio over a WebSocket and receive partial and final results as you speak.
- **Powers voice search** — provides the transcription behind Kubuno's core voice search, and exposes an HTTP API any other module can call for dictation or transcription.
- **Managed models** — download, list and delete recognition models from the core administration console (*Voice search*); they live in the module's writable data directory and survive upgrades.
- **Tunable per language** — an administrator can set the model, an initial prompt (vocabulary and spelling bias), punctuation and number normalisation, translation to English, beam size (accuracy vs. speed) and automatic language detection, plus global capture settings (silence auto-stop, sound threshold, optional profanity filter).

## Engine

STT runs on **Whisper** ([whisper.cpp](https://github.com/ggerganov/whisper.cpp) via [`whisper-rs`](https://crates.io/crates/whisper-rs)), compiled from source and **linked statically** into the binary. There is therefore **no external native library to install** on the host — the reason the module can be distributed as one self-contained package on every operating system.

## Architecture

Kubuno is **modular**: each module is a **separate process** that registers with the [core](https://github.com/kubuno/core) at startup, and which the core reverse-proxies. STT is an **internal** module — it is registered for routing (to power core voice search) but hidden from the admin module list; it has no frontend bundle and no database of its own. Configuration and downloaded models live in the module's writable data directory.

| | |
|---|---|
| Port | `3122` |
| Kind | Internal (powers core voice search) |
| Engine | Whisper (whisper.cpp, statically linked) |

### HTTP API

All routes are reached through the core proxy under `/api/v1/stt`.

| Method | Path | Purpose |
| --- | --- | --- |
| `GET`  | `/health` | Liveness probe. |
| `GET`  | `/status?lang=<code>` | Whether recognition is enabled (globally and per language) and the capture settings the client should use. |
| `POST` | `/transcribe?lang=<code>` | Batch transcription of a WAV body → `{ "text": … }`. |
| `GET`  | `/stream?lang=<code>&rate=<hz>` | Real-time transcription over a WebSocket (binary PCM in, JSON partial/final out). |

Admin routes (role-gated) manage the global switch, per-language configuration, capture settings and the model catalogue (list / download / delete).

## Install

This module ships in the **all-in-one [Kubuno](https://github.com/kubuno/core) Docker image** (`ghcr.io/kubuno/kubuno`) — the easiest way to self-host a full Kubuno instance (core + every module). See **[kubuno/docker](https://github.com/kubuno/docker)** for `docker compose` instructions.

A Kubuno module is distributed as a single **`.kbpkg`** — a self-contained package the Kubuno server installs itself, identical on Linux, Windows and macOS. Tagged releases attach one `.kbpkg` per platform to the [GitHub Releases](https://github.com/kubuno/stt/releases) page. Install it from the admin console, or offline from the command line:

```bash
sudo kubuno modules:install stt-<version>-<os>-<arch>.kbpkg
sudo systemctl restart kubuno              # the core loads the module on (re)start
```

## Build & development

Building requires a C/C++ toolchain, **CMake** (whisper.cpp is compiled from source) and **libclang** (the Rust bindings are generated at build time), plus the Rust toolchain (Rust ≥ 1.82).

```bash
bash build_kbpkg.sh                         # → dist/stt-<version>-<os>-<arch>.kbpkg
bash build_kbpkg.sh --install               # build + install into the local store + restart
```

## Tech stack

Rust 2021 · Axum 0.7 · Tokio · `whisper-rs` (whisper.cpp) · `hound` (WAV).

## Contributing

Contributions are welcome. Please open an issue to discuss any significant change before submitting a pull request.

## License

[AGPL-3.0-or-later](LICENSE) © Kubuno contributors.
