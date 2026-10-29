# Solarum – notes for working on the code

Techno drum machine: orbit sequencer, pads, bass & acid, auto mode, live effects, WAV export, MIDI input. Rust + eframe/egui 0.31, cpal for audio, midir for MIDI. Architecture: `docs/ARCHITECTURE.md`; user docs: `README.md`. Keep both up to date with every feature.

## Commands

| Command | What it does |
|---|---|
| `cargo test --release` | all tests (fast) |
| `cargo clippy --release --all-targets -- -D warnings` | must stay free of warnings |
| `cargo fmt` | CI checks `cargo fmt --check` (`max_width = 140`) |
| `cargo test --release render_ -- --ignored` | screenshots to `target/snapshots/*.png` and demo loops to `target/snapshots/sounds/*.wav` – look at (and listen to) them after UI or sound changes |
| `cargo test --release print_levels -- --ignored --nocapture` | loudness of every track alone |
| `./scripts/build-mac.sh --install` | builds `dist/Solarum.app` (universal), replaces `/Applications/Solarum.app` and removes the build copy; `NATIVE=1` builds for this Mac only |
| `./scripts/build-mac.sh --zip` | builds `dist/Solarum-macos-universal.zip` (release asset) |

Commit only when `cargo test` exits 0.

## Conventions

- **Language:** UI, README, docs, release notes, commit messages and code comments in English.
- **egui pins:** egui/eframe 0.31 with `egui-phosphor` 0.9 – upgrade them together (same as Mindful).
- **Audio thread:** `Engine::render` runs in the device callback – no locks, no blocking, no file I/O there. Talk to it with `Cmd`, read state from `Shared`.
- **Song is `Copy`:** keep it that way (fixed arrays, no `Vec`/`String`) so it can be swapped between threads cheaply.
- **New `Song` / `Track` / `Step` fields:** add `#[serde(default)]` so saved state from older versions still loads.
- **Randomness:** use `rng::Rng` (seedable), so tests and exports are repeatable.
- **Visual checks:** verify UI with the offscreen renderer (`src/offscreen.rs`, tests in `src/snapshot.rs`, `SolarumApp::headless` + `fake_live`). Don't take screenshots of the user's desktop.
- **Sound checks:** render the demo loops and check `print_levels`; nothing may clip (tests assert peak ≤ 1).

## Website

`site/` is the static page at https://tj537.github.io/solarum/ (one `index.html`, fonts, screenshot, icon); `.github/workflows/pages.yml` publishes it on every push to `main` that touches `site/`. Preview it with the `site` entry in `.claude/launch.json`. Update it with new features and when the first-launch steps change. SEO: the JSON-LD and the FAQ in `index.html` say the same things (keep them in sync), `llms.txt` is the summary for AI assistants, and `sitemap.xml` gets a new `lastmod` when the page changes. The `robots.txt` lives in the root site repo `tj537/tj537.github.io` (crawlers only read it at the domain root).

## Releases

Asset names never change (`Solarum-macos-universal.zip`, `Solarum-windows-x64.zip`) so `releases/latest/download/…` links keep working. Releases up to 0.9.0 also carry the old `Bunker-…` names. Saved settings moved from the old "Bunker" storage folder to "Solarum"; `main.rs` moves them over once on the first start.

1. Raise `version` in `Cargo.toml`, commit.
2. Write `.github/release-notes/vX.Y.Z.md` (English).
3. `git tag -a vX.Y.Z -m "Solarum X.Y.Z" && git push origin vX.Y.Z` – `.github/workflows/release.yml` builds both platforms, adds `SHA256SUMS.txt` and publishes the release.

## Platform notes

- Windows: `build.rs` embeds `assets/icon.ico` (only when building on Windows); `main.rs` sets the GUI subsystem.
- Linux builds need `libasound2-dev` and `libgtk-3-dev` (CI installs them); Linux isn't released.
- The Mac bundle is ad-hoc signed only (no notarization yet), so first launch needs *Open Anyway*.
- Icons: the logo is drawn by `widgets::logo` (the lime sun with its rings of light on black – the app icon); `cargo test --release render_icon -- --ignored` renders it to `target/snapshots/icon.png` (1024 px), and `icon.png` (512), `icon.icns` (sips + iconutil) and `icon.ico` (PNG entries) come from that. `assets/icon.svg` is the same drawing as a vector.
