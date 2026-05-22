# AGENTS.md

> Guidelines for AI agents working in this repository.

## Project Overview

**spank** is a macOS CLI tool that detects physical hits/slaps on Apple Silicon MacBooks via the accelerometer and plays audio responses. Rust application with embedded MP3 assets.

- **Platform**: macOS on Apple Silicon (M2+) only
- **Runtime requirement**: `sudo` (for IOKit HID accelerometer access)
- **Architecture**: Single `src/main.rs` file with embedded audio assets

## Commands

### Run

```bash
# Run after installing (requires sudo)
sudo ./spank
sudo ./spank --sexy      # escalating responses mode
sudo ./spank --halo      # Halo death sounds mode
sudo ./spank --custom /path/to/mp3s  # custom audio directory
```

### Install

```bash
cargo install --git https://github.com/taigrr/spank spank
cargo install --path .
```

There is no alternate package-manager or release-archive support. Keep installation docs limited to `cargo install`.

## Code Organization

```
spank/
├── Cargo.toml           # Rust package manifest
├── src/main.rs          # All application code (single file)
├── audio/
│   ├── pain/            # Default "ow!" responses (10 MP3s)
│   ├── sexy/            # Escalating responses (60 MP3s)
│   ├── halo/            # Halo death sounds (9 MP3s)
│   └── lizard/          # Lizard escalation sounds
└── doc/logo.png
```

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` | CLI argument parsing |
| `include_dir` | Compile-time embedded audio directories |
| `rodio` | Audio playback and MP3 decoding |
| `serde_json` | Stdio JSON command protocol |
| `ctrlc` | Ctrl-C shutdown handling |
| `libc` | `geteuid` and platform FFI types |

## Code Patterns

### Embedded Assets

Audio files are embedded at compile time using `include_dir!`:

```rust
static PAIN_AUDIO: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/audio/pain");
```

### Play Modes

Two playback strategies in `PlayMode`:
- `Random`: Random file selection (pain, halo, custom modes)
- `Escalation`: Intensity increases with slap frequency (sexy and lizard modes)

### Slap Detection Flow

1. Rust FFI wakes Apple SPU HID drivers via IOKit/CoreFoundation
2. The current CFRunLoop receives accelerometer callbacks
3. `Detector::new()` processes samples with vibration detection algorithms
4. Events trigger audio playback with configurable cooldown

### Concurrency

- `Arc<RwLock<Settings>>` protects live stdin-tunable settings
- Audio playback runs in background Rust threads
- Sensor callbacks push samples into a mutex-protected in-process queue

## Constants

Key tuning parameters in `src/main.rs`:

| Constant | Value | Purpose |
|----------|-------|---------|
| `DECAY_HALF_LIFE` | 30s | How fast escalation fades |
| `DEFAULT_COOLDOWN_MS` | 750ms | Minimum time between audio plays |
| `DEFAULT_SENSOR_POLL_INTERVAL` | 10ms | Accelerometer polling rate |
| `DEFAULT_MAX_SAMPLE_BATCH` | 200 | Max samples processed per tick |

## Gotchas

1. **Root required**: The app must run with `sudo` for IOKit HID access. The `run()` function checks `libc::geteuid() != 0`.

2. **Apple Silicon only**: Runtime support is macOS on `aarch64`/Apple Silicon. Intel Macs are not supported.

3. **No accelerometer dependency**: IOKit/CoreFoundation access is implemented directly in Rust FFI.

4. **Single file**: All code is in `src/main.rs`. When adding features, follow the existing pattern of types and functions in the same file.

5. **Mutually exclusive modes**: `--sexy`, `--halo`, `--lizard`, and `--custom`/`--custom-files` flags cannot be combined.

6. **Cargo install only**: Do not add alternate package-manager or release-archive support.

## Adding Audio

To add a new sound pack:

1. Create directory under `audio/`
2. Add MP3 files (numbered for escalation mode, any names for random)
3. Add an `include_dir!` static
4. Add flag and case in `select_sound_pack()`
5. Create `SoundPack` with appropriate `PlayMode`

## Version

Version comes from `Cargo.toml` and is exposed through Clap's generated `--version` output.
