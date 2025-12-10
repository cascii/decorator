# decorator
lightweight tauri frame for cascii generated art

## Setup

### Prerequisites
- [Node.js](https://nodejs.org/) (v18 or later)
- [Rust](https://www.rust-lang.org/tools/install) (latest stable)
- System dependencies for Tauri (see [Tauri prerequisites](https://tauri.app/v1/guides/getting-started/prerequisites))

### Installation

1. Install dependencies:
```bash
cargo tauri dev
```

2. Run in development mode:
```bash
cargo tauri dev
```

## Project Structure

- `src/` - Frontend code (HTML, CSS, JavaScript)
- `src-tauri/` - Rust backend code
- `src-tauri/src/main.rs` - Main Rust entry point
- `src-tauri/tauri.conf.json` - Tauri configuration

## Notes

- Add app icons to `src-tauri/icons/` directory (32x32, 128x128, 256x256, 512x512 PNG files)
- The app runs on `http://localhost:1420` in development mode
