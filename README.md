# Rusty Tuner

A desktop guitar tuner built with Rust and `egui`.

## Features
- Real-time pitch detection
- Visual tuning gauge and strobe effect
- Support for multiple tunings (Standard, Drop D, etc.)
- Adjustable A4 reference frequency (420 Hz - 460 Hz)
- Auto string detection or manual selection
- Built-in tone generator for reference
- Volume control and VU meter

## Getting Started

### Prerequisites

You'll need a working Rust toolchain. On Linux, you may also need some development headers for your audio backend (like ALSA) and display.

For example, on Ubuntu/Debian:
```bash
sudo apt install libasound2-dev libx11-dev
```

### Running

Just use Cargo to build and run the app:

```bash
cargo run --release
```

## Controls

Several keyboard shortcuts handle the basic controls:

- **Space**: Toggle the reference tone for the currently selected string
- **1-6**: Select a specific string manually
- **0 or A**: Switch back to auto string detection mode
- **W**: Toggle the small waveform display overlay
- **D**: Toggle the calibration overlay, used for modifying UI zones
