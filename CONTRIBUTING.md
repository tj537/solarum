# Contributing to Solarum

Thank you for wanting to help! Solarum is young and changes fast, and every kind of help is welcome: a bug report, an idea, a groove that sounds wrong, a typo in the README, or a pull request.

You don't need to be a Rust expert or a musician. If you're unsure about anything, open an issue and ask.

## Ways to help

- **Report a bug** – [open a bug report](https://github.com/tj537/solarum/issues/new?template=bug_report.yml). Say what you did, what you expected and what happened; a screenshot or a short recording helps a lot.
- **Suggest an idea** – [open a feature request](https://github.com/tj537/solarum/issues/new?template=feature_request.yml). Solarum is meant to be easy for everyone, kids and beginners included, so tell us who the idea helps.
- **Tell us what feels hard** – if something confused you, that's a bug in Solarum, not in you.
- **Send a pull request** – for anything bigger than a small fix, open an issue first so we can agree on the idea before you spend time on it.

## Getting started

You need [Rust](https://rustup.rs) 1.96 or newer (stable). On Linux you also need a few libraries:

```bash
sudo apt-get install libasound2-dev libgtk-3-dev libxcb-render0-dev libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev
```

Then:

```bash
cargo run --release
```

## Before you send a pull request

CI runs these on macOS, Windows and Linux, so please run them locally first:

```bash
cargo fmt
```

```bash
cargo clippy --release --all-targets -- -D warnings
```

```bash
cargo test --release
```

Clippy must be free of warnings and every test must pass. Formatting uses `rustfmt.toml` (`max_width = 140`).

If you changed the UI or the sound, also render the screenshots and demo loops and look at (and listen to) them:

```bash
cargo test --release render_ -- --ignored
```

They land in `target/snapshots/` (PNGs) and `target/snapshots/sounds/` (WAVs). To check that nothing is too loud or too quiet:

```bash
cargo test --release print_levels -- --ignored --nocapture
```

## How the code works

[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains the modules, the audio engine and the UI. A few rules matter everywhere:

- **The audio thread never waits.** `Engine::render` runs inside the sound card's callback: no locks, no blocking, no file I/O there. The UI talks to it with `Cmd` messages and reads its state from `Shared`.
- **`Song` is `Copy`.** It uses fixed-size arrays, never `Vec` or `String`, so it can be swapped between threads cheaply. Please keep it that way.
- **Old saves must keep loading.** Every new field on `Song`, `Track` or `Step` gets `#[serde(default)]`.
- **Randomness is repeatable.** Use `rng::Rng` (seedable) so tests and exports give the same result every time.
- **Nothing may clip.** Tests assert a peak of at most 1.0.
- **English everywhere** – UI, docs, comments and commit messages.
- **egui, eframe and egui-phosphor** are pinned together (egui/eframe 0.31, egui-phosphor 0.9); upgrade them in one step.
- **Docs move with the code.** If you add or change a feature, update `README.md` (for players) and `docs/ARCHITECTURE.md` (for developers).

## Commits and pull requests

- Small, focused commits with a message that says what changes for the player (for example *"Shuffle keeps the tempo when asked"*).
- One topic per pull request. Fill in the template; screenshots or a short WAV are great for UI and sound changes.
- By contributing you agree that your work is released under the project's [MIT license](LICENSE).
- New dependencies must have a license compatible with MIT (MIT, Apache-2.0, BSD, Zlib, ISC, OFL for fonts …); add them to [THIRD-PARTY-LICENSES.txt](THIRD-PARTY-LICENSES.txt) by running `./scripts/third-party-licenses.sh` (needs `cargo install cargo-about`).

## Be kind

Everyone here follows our [Code of Conduct](CODE_OF_CONDUCT.md). Solarum is made for everyone, and so is this project.
