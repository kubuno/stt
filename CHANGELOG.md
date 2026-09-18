# Changelog

All notable changes to **kubuno-stt** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this
project adheres to [Semantic Versioning](https://semver.org/). Entries are added under
`[Unreleased]` **as the change is made**; `_tools/release.sh` stamps them under the version
number at release time, and CI publishes that section as the GitHub Release notes.

## [Unreleased]

### Removed

- **The build no longer downloads the Vosk shared library.** The engine was
  dropped in 0.1.0 but the release pipeline still fetched its library on every
  run, making each build depend on a third-party download it had no use for.


## [0.1.1] - 2026-09-18

### Security

- **Error library updated to a patched release.** `anyhow` moves from 1.0.103
  to 1.0.104, closing an unsoundness in `Error::downcast_mut()`
  (RUSTSEC-2026-0190).
- **TLS library updated to a patched release.** The pinned `rustls` carried
  RUSTSEC-2026-0285 (medium). Every outbound HTTPS connection goes through it.

## [0.1.0] - 2026-09-17

### Removed

- **The Vosk engine is gone; speech recognition now runs on Whisper only.**
  Vosk shipped as a prebuilt native shared library (`libvosk.so`/`.dll`/`.dylib`)
  that had to be present on the host, which the self-contained `.kbpkg` install
  model cannot provide and which differs on every operating system. Whisper
  (whisper.cpp) is compiled from source and linked statically into the binary, so
  the module now ships as one self-contained package with **no external native
  library to install** — identical behaviour on Linux, Windows and macOS. Whisper
  is multilingual, so a single model serves every language; per-language Vosk
  models and the streaming word-grammar option are no longer offered.

### Changed

- **This module now installs as a Kubuno package (`.kbpkg`) only.** Its system
  packages (Debian/RPM and the Windows and macOS installers) are no longer
  built: the module is distributed as one `.kbpkg` per platform (Linux, Windows,
  macOS) that the Kubuno server installs itself — from the admin console, or
  offline with `kubuno modules:install <file>.kbpkg`.



### Fixed

- **The package could not be built where `zip` is absent.** The Windows job of
  the continuous integration has no `zip`, so the Windows package was simply lost
  the first time it was attempted — a script failure, not a build failure. The
  builder now falls back to 7-Zip, then to PowerShell.
### Added

- **This module now ships a `.kbpkg`** — the single package format a Kubuno
  server installs by itself, the same file on Linux, Windows and macOS. It
  carries the same binary, interface and manifest as the system packages,
  arranged the way the server expects to find a module on disk, plus a
  `SHA256SUMS` so a copy carried offline can be checked without the catalogue.
  Nothing changes for existing installations: the `.deb`, `.rpm`, `.exe` and
  `.pkg` are still published, and a catalogue that sees both simply prefers the
  new one. It is also the only format the server can unpack without an external
  tool, which is what makes one-click installation possible away from
  Debian-like systems.
### Fixed

- **A built package could be thrown away instead of published.** The job that
  attaches a package to the release waited ten minutes for another workflow to
  create that release, then gave up with "release never appeared — build.yml
  likely failed". The diagnosis was wrong: on a repository whose `.deb` takes
  longer than ten minutes to build, the release simply did not exist yet, and a
  package that had built perfectly was discarded. The job now creates the release
  itself when it is missing, so it no longer depends on another workflow
  finishing first — which also means this repository, which has no `build.yml`
  at all, can publish its packages for the first time.
