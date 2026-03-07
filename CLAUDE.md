# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Run

```bash
cargo build              # debug build
cargo build --release    # optimized release build (LTO, stripped)
cargo run                # build + run (debug)
cargo run --release      # build + run (release)
```

Binary name is `monitor` (defined in Cargo.toml `[[bin]]`).

**Note:** Cargo.toml expects the source at `src/main.rs`, but the code currently lives in `main.rs` at the repo root. You must create `src/` and move the file there, or adjust the Cargo.toml path, before `cargo build` will work.

There are no tests, lints, or CI configured.

## Architecture

Single-file Rust HTTP server (`main.rs`) that exposes system metrics as JSON over a REST API, designed to be polled (~2s intervals) by an Android companion app over local WiFi.

**Stack:** axum 0.7 (web framework) + tokio (async runtime) + sysinfo 0.30 (metrics) + tower-http (CORS)

**Shared state:** `Arc<AppState>` holds four `Mutex`-wrapped fields — `System`, `Disks`, `Networks`, and a `NetSnapshot` for calculating network speed deltas between requests.

**API endpoints (all GET, port 8080, CORS open to all origins):**
- `/` — server info + endpoint list
- `/health` — health check
- `/metrics` — full snapshot (system, CPU, memory, disk, network)
- `/metrics/cpu`, `/metrics/memory`, `/metrics/disk`, `/metrics/network` — individual subsections

**Key behavior:**
- sysinfo requires a warm-up call on startup (first CPU sample is always 0); the `main()` function handles this with an initial refresh + 200ms sleep.
- Network upload/download speed is derived by diffing cumulative byte counters against the previous snapshot's timestamp — the first request after startup will reflect speed since boot.
- All builder functions (`build_cpu_info`, `build_memory_info`, etc.) are pure transformations from sysinfo types to serializable response structs.
- Local IP detection uses the UDP connect trick (`UdpSocket` connect to 8.8.8.8:80) to find the outbound interface address.
