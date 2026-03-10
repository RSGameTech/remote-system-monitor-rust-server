# WebSocket + Windows GPU Support Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add real-time WebSocket metrics push, process kill, selectable refresh intervals, temperature sensors, and Windows AMD/Intel GPU detection (via WMI) to the existing Rust server.

**Architecture:** The WS endpoint lives on a split public router (no auth middleware) and authenticates via `?key=` query param at upgrade time. Two tasks per connection run concurrently via `tokio::select!` — one pushes metrics on a configurable interval, the other receives client commands. A `tokio::sync::watch` channel coordinates interval changes between tasks. Windows GPU detection uses the `wmi` crate to query `Win32_VideoController`.

**Tech Stack:** axum 0.7 (ws feature), tokio (watch channel), futures-util 0.3 (StreamExt), sysinfo (Components for temperature), wmi 0.14 (Windows-only, AMD/Intel GPU)

---

### Task 1: Update Cargo.toml

**Files:**
- Modify: `Cargo.toml`

**Step 1: Add ws feature to axum, add futures-util, add wmi as Windows-only dependency**

Edit `Cargo.toml` to match:

```toml
[package]
name = "system-monitor-server"
version = "1.0.0"
edition = "2021"
description = "Remote System Monitor - lightweight Rust server"

[[bin]]
name = "monitor"
path = "src/main.rs"

[dependencies]
# Web framework
axum = { version = "0.7", features = ["json", "ws"] }
tokio = { version = "1", features = ["full"] }
tower-http = { version = "0.5", features = ["cors"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# System metrics
sysinfo = "0.30"
nvml-wrapper = "0.10"

# WebSocket streams
futures-util = "0.3"

# Misc
chrono = { version = "0.4", features = ["serde"] }
tracing = "0.1"
tracing-subscriber = "0.3"

[target.'cfg(target_os = "windows")'.dependencies]
wmi = "0.14"

[profile.release]
opt-level = 3
lto = true
codegen-units = 1
strip = true
```

**Step 2: Verify cargo can fetch dependencies**

```bash
cargo fetch
```

Expected: downloads futures-util and wmi metadata, no errors.

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add ws, futures-util, wmi dependencies"
```

---

### Task 2: Add New Types

**Files:**
- Modify: `src/main.rs` — add types after existing response models, update `MetricsResponse`

**Step 1: Add `TemperatureInfo` struct**

Add after the `NetworkInfo` struct (around line 120):

```rust
#[derive(Serialize)]
struct TemperatureInfo {
    /// Sensor label e.g. "CPU Package", "NVMe SSD", "acpitz"
    component: String,
    /// Current temperature in °C
    temperature_celsius: f32,
    /// Maximum recorded temperature in °C (if available)
    max_celsius: Option<f32>,
    /// Critical threshold in °C (if available)
    critical_celsius: Option<f32>,
}
```

**Step 2: Add WebSocket message types**

Add after `TemperatureInfo`:

```rust
/// Inbound messages from the Android client
#[derive(serde::Deserialize, Debug)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ClientMessage {
    SetInterval { ms: u64 },
    KillProcess { pid: u32 },
    Ping,
}

/// Outbound messages from the server to the Android client
#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum ServerMessage {
    Metrics { data: MetricsResponse },
    KillResult { pid: u32, success: bool, error: Option<String> },
    Pong,
    Error { message: String },
}
```

**Step 3: Update `MetricsResponse` to include temperatures**

Change the existing struct from:

```rust
#[derive(Serialize)]
struct MetricsResponse {
    timestamp: String,
    system: SystemInfo,
    cpu: CpuInfo,
    memory: MemoryInfo,
    gpu: Vec<GpuInfo>,
    disk: Vec<DiskInfo>,
    network: NetworkInfo,
}
```

To:

```rust
#[derive(Serialize)]
struct MetricsResponse {
    timestamp: String,
    system: SystemInfo,
    cpu: CpuInfo,
    memory: MemoryInfo,
    gpu: Vec<GpuInfo>,
    disk: Vec<DiskInfo>,
    network: NetworkInfo,
    temperatures: Vec<TemperatureInfo>,
}
```

**Step 4: Verify it compiles (will fail on MetricsResponse construction — expected)**

```bash
cargo build 2>&1 | head -30
```

Expected: errors about `temperatures` field missing in struct literal in `get_metrics`. That's fine — we fix that in Task 5.

---

### Task 3: Add Interval Validation

**Files:**
- Modify: `src/main.rs` — add constant and function in the Metric Helpers section

**Step 1: Add after the `uptime_string` function**

```rust
const ALLOWED_INTERVALS: &[u64] = &[250, 500, 1000, 2000, 5000];

fn validate_interval(ms: u64) -> Result<u64, String> {
    if ALLOWED_INTERVALS.contains(&ms) {
        Ok(ms)
    } else {
        Err(format!(
            "Invalid interval {}ms. Allowed: 250, 500, 1000, 2000, 5000",
            ms
        ))
    }
}
```

---

### Task 4: Add Temperature Builder

**Files:**
- Modify: `src/main.rs` — add function in the Builders section

**Step 1: Add `build_temperature_info` in the Builders section, after `build_memory_info`**

```rust
fn build_temperature_info() -> Vec<TemperatureInfo> {
    let components = sysinfo::Components::new_with_refreshed_list();
    components
        .list()
        .iter()
        .map(|c| TemperatureInfo {
            component: c.label().to_string(),
            temperature_celsius: c.temperature(),
            max_celsius: Some(c.max()).filter(|&v| v > 0.0),
            critical_celsius: c.critical(),
        })
        .collect()
}
```

> Note: On Linux this reads hwmon. On Windows it reads WMI sensor data. The array may be empty if the OS doesn't expose sensors or the process lacks permissions.

---

### Task 5: Extract `collect_full_metrics` and Update `get_metrics`

**Files:**
- Modify: `src/main.rs` — extract logic from `get_metrics`, add new async function

**Step 1: Add `collect_full_metrics` function before `get_metrics`**

```rust
async fn collect_full_metrics(state: &Arc<AppState>) -> MetricsResponse {
    let mut sys = state.sys.lock().unwrap();
    sys.refresh_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::everything())
            .with_memory(sysinfo::MemoryRefreshKind::everything()),
    );

    let mut disks = state.disks.lock().unwrap();
    disks.refresh();

    let mut networks = state.networks.lock().unwrap();
    networks.refresh();

    let cpu_info = build_cpu_info(&sys);
    let memory_info = build_memory_info(&sys);
    let gpu_info = build_gpu_info(&state.nvml);
    let disk_info = build_disk_info(&disks);
    let network_info = build_network_info(&networks, &mut state.prev_net.lock().unwrap());
    let system_info = build_system_info(&sys);

    MetricsResponse {
        timestamp: Local::now().to_rfc3339(),
        system: system_info,
        cpu: cpu_info,
        memory: memory_info,
        gpu: gpu_info,
        disk: disk_info,
        network: network_info,
        temperatures: build_temperature_info(),
    }
}
```

**Step 2: Replace the body of `get_metrics` with a delegation call**

Replace the entire `get_metrics` function body with:

```rust
async fn get_metrics(State(state): State<Arc<AppState>>) -> Json<MetricsResponse> {
    Json(collect_full_metrics(&state).await)
}
```

**Step 3: Build to confirm no errors**

```bash
cargo build 2>&1 | head -40
```

Expected: clean build (no errors).

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: extract collect_full_metrics, add temperature support"
```

---

### Task 6: Add `kill_process` Helper

**Files:**
- Modify: `src/main.rs` — add in the Metric Helpers section

**Step 1: Add after `validate_interval`**

```rust
fn kill_process(state: &Arc<AppState>, pid: u32) -> Result<(), String> {
    // Refuse to kill system/kernel processes
    if pid < 1000 {
        return Err(format!("PID {} is protected (system process)", pid));
    }

    let mut sys = state.sys.lock().unwrap();
    sys.refresh_processes();

    let sysinfo_pid = sysinfo::Pid::from(pid as usize);
    match sys.process(sysinfo_pid) {
        Some(process) => {
            tracing::warn!(
                "Killing process: {} (PID {})",
                process.name(),
                pid
            );
            process.kill();
            Ok(())
        }
        None => Err(format!("PID {} not found", pid)),
    }
}
```

> Note: `sysinfo::Process::kill()` calls `TerminateProcess` on Windows and `SIGKILL` on Linux — no platform-specific code needed.

**Step 2: Build**

```bash
cargo build 2>&1 | head -20
```

Expected: clean build.

---

### Task 7: Add Windows WMI GPU Detection

**Files:**
- Modify: `src/main.rs` — add `collect_wmi_gpus` function and update `build_gpu_info`

**Step 1: Add `collect_wmi_gpus` after `collect_sysfs_gpus` (or after `build_gpu_info` if on Windows-first preference)**

Place this after the `#[cfg(target_os = "linux")]` sysfs helper functions block:

```rust
#[cfg(target_os = "windows")]
fn collect_wmi_gpus(start_idx: u32) -> Vec<GpuInfo> {
    use serde::Deserialize;
    use wmi::{COMLibrary, WMIConnection};

    #[derive(Deserialize)]
    #[allow(non_snake_case)]
    struct Win32VideoController {
        Name: String,
        AdapterCompatibility: Option<String>,
        AdapterRAM: Option<u32>,
    }

    let com_lib = match COMLibrary::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("WMI COMLibrary init failed: {}", e);
            return Vec::new();
        }
    };
    let wmi_con = match WMIConnection::new(com_lib) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("WMI connection failed: {}", e);
            return Vec::new();
        }
    };

    let controllers: Vec<Win32VideoController> = match wmi_con.query() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("WMI query failed: {}", e);
            return Vec::new();
        }
    };

    controllers
        .into_iter()
        .enumerate()
        .filter_map(|(i, c)| {
            let compat = c.AdapterCompatibility.as_deref().unwrap_or("").to_lowercase();
            let vendor = if compat.contains("intel") {
                "Intel"
            } else if compat.contains("amd") || compat.contains("advanced micro") {
                "AMD"
            } else {
                return None; // skip NVIDIA (handled by NVML) and unknowns
            };

            // AdapterRAM is a u32 in WMI — capped at ~4 GB for older drivers.
            // Divide bytes → MB.
            let mem_mb = c.AdapterRAM.map(|b| b as u64 / 1_048_576).unwrap_or(0);

            Some(GpuInfo {
                index: start_idx + i as u32,
                name: c.Name,
                vendor: vendor.to_string(),
                temperature_celsius: None,
                utilization_percent: None,
                memory_total_mb: mem_mb,
                memory_used_mb: 0,
                memory_usage_percent: 0.0,
                fan_speed_percent: None,
                power_draw_watts: None,
                clock_speed_mhz: None,
            })
        })
        .collect()
}
```

**Step 2: Update `build_gpu_info` to call WMI on Windows**

The current function ends with the Linux `#[cfg]` block. Add a Windows equivalent after it:

```rust
fn build_gpu_info(nvml: &Option<nvml_wrapper::Nvml>) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();

    // NVIDIA GPUs via NVML (cross-platform)
    if let Some(nvml) = nvml {
        gpus.extend(collect_nvidia_gpus(nvml));
    }

    // AMD + Intel GPUs via sysfs (Linux only)
    #[cfg(target_os = "linux")]
    {
        gpus.extend(collect_sysfs_gpus(gpus.len() as u32));
    }

    // AMD + Intel GPUs via WMI (Windows only)
    #[cfg(target_os = "windows")]
    {
        gpus.extend(collect_wmi_gpus(gpus.len() as u32));
    }

    gpus
}
```

**Step 3: Build on Linux (WMI code is gated, should compile cleanly)**

```bash
cargo build 2>&1 | head -20
```

Expected: clean build. The `#[cfg(target_os = "windows")]` block is skipped on Linux.

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: add Windows AMD/Intel GPU detection via WMI"
```

---

### Task 8: Add WebSocket Handlers

**Files:**
- Modify: `src/main.rs` — add two functions in the Route Handlers section

**Step 1: Add `ws_handler` and `handle_ws` after the `get_network` handler**

```rust
async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Response {
    // Auth: validate ?key= query param at upgrade time
    match params.get("key").map(|k| k.as_str()) {
        Some(key) if key == state.api_key => {}
        _ => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "missing or invalid API key" })),
            )
                .into_response();
        }
    }

    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(socket: axum::extract::ws::WebSocket, state: Arc<AppState>) {
    use axum::extract::ws::Message;
    use futures_util::StreamExt;
    use tokio::sync::watch;

    let (mut sender, mut receiver) = socket.split();

    // Shared interval between recv task and send loop (default 2s)
    let (interval_tx, mut interval_rx) = watch::channel(2000u64);

    // ── Task A: push metrics on interval ────────────────────────────────────
    let state_clone = state.clone();
    let mut send_task = tokio::spawn(async move {
        let mut current_ms = *interval_rx.borrow();
        let mut interval = tokio::time::interval(
            std::time::Duration::from_millis(current_ms)
        );

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    // Rebuild interval if client changed it
                    if interval_rx.has_changed().unwrap_or(false) {
                        current_ms = *interval_rx.borrow_and_update();
                        interval = tokio::time::interval(
                            std::time::Duration::from_millis(current_ms)
                        );
                        interval.tick().await; // skip immediate first tick
                    }

                    let metrics = collect_full_metrics(&state_clone).await;
                    let msg = ServerMessage::Metrics { data: metrics };
                    let json = serde_json::to_string(&msg).unwrap();

                    if sender.send(Message::Text(json)).await.is_err() {
                        break; // client disconnected
                    }
                }
            }
        }
    });

    // ── Task B: receive commands from Android ────────────────────────────────
    let state_clone2 = state.clone();
    let mut recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            match msg {
                Message::Text(text) => {
                    match serde_json::from_str::<ClientMessage>(&text) {
                        Ok(ClientMessage::SetInterval { ms }) => {
                            match validate_interval(ms) {
                                Ok(valid_ms) => {
                                    let _ = interval_tx.send(valid_ms);
                                    tracing::info!("Client set interval to {}ms", valid_ms);
                                }
                                Err(e) => {
                                    tracing::warn!("Invalid interval: {}", e);
                                }
                            }
                        }
                        Ok(ClientMessage::KillProcess { pid }) => {
                            let result = kill_process(&state_clone2, pid);
                            tracing::warn!(
                                "Kill request for PID {}: {}",
                                pid,
                                if result.is_ok() { "success" } else { "failed" }
                            );
                        }
                        Ok(ClientMessage::Ping) => {
                            tracing::debug!("Ping received");
                        }
                        Err(e) => {
                            tracing::warn!("Unparseable WS message: {}", e);
                        }
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    // If either task exits, abort the other
    tokio::select! {
        _ = &mut send_task => recv_task.abort(),
        _ = &mut recv_task => send_task.abort(),
    }
}
```

**Step 2: Build**

```bash
cargo build 2>&1 | head -40
```

Expected: errors about missing import `WebSocketUpgrade`. Fix in Task 9.

---

### Task 9: Update Imports, Router, and Startup Banner

**Files:**
- Modify: `src/main.rs` — imports at top, router in `main()`, banner in `main()`

**Step 1: Update the axum import block at the top of the file**

Replace:

```rust
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
```

With:

```rust
use axum::{
    extract::{ws::WebSocketUpgrade, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use futures_util::StreamExt;
```

**Step 2: Split the router in `main()` to exempt `/ws` from auth middleware**

Replace:

```rust
let app = Router::new()
    .route("/", get(root))
    .route("/health", get(health))
    .route("/metrics", get(get_metrics))
    .route("/metrics/cpu", get(get_cpu))
    .route("/metrics/memory", get(get_memory))
    .route("/metrics/gpu", get(get_gpu))
    .route("/metrics/disk", get(get_disk))
    .route("/metrics/network", get(get_network))
    .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
    .layer(cors)
    .with_state(state);
```

With:

```rust
let public = Router::new()
    .route("/ws", get(ws_handler));

let protected = Router::new()
    .route("/", get(root))
    .route("/health", get(health))
    .route("/metrics", get(get_metrics))
    .route("/metrics/cpu", get(get_cpu))
    .route("/metrics/memory", get(get_memory))
    .route("/metrics/gpu", get(get_gpu))
    .route("/metrics/disk", get(get_disk))
    .route("/metrics/network", get(get_network))
    .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));

let app = public
    .merge(protected)
    .layer(cors)
    .with_state(state);
```

**Step 3: Add WebSocket URL to the startup banner**

Find the banner `println!` block and add a WS URL line after the Network URL line:

```rust
println!(
    "  WebSocket:   ws://{}:{}  ← use in Android app",
    local_ip, port
);
```

Place it directly after:
```rust
println!(
    "  Network URL: http://{}:{}  ← use in Android app",
    local_ip, port
);
```

Also update the banner note about polling to reflect WebSocket:

```rust
println!("  WebSocket refresh: 250ms / 500ms / 1s / 2s (default) / 5s");
```

Replace the existing:
```rust
println!("  Optimised for 2s polling over local WiFi");
```

**Step 4: Build — expect clean compile**

```bash
cargo build 2>&1
```

Expected: clean build, zero errors.

If there's a `StreamExt` unused import warning (because `.split()` and `.next()` are used inside the async closure where it's re-imported via `use futures_util::StreamExt`), remove the top-level import and keep only the inner one inside `handle_ws`. Either approach compiles.

**Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat: add WebSocket endpoint with real-time metrics push and process kill"
```

---

### Task 10: Final Build Verification

**Step 1: Clean build**

```bash
cargo build --release 2>&1
```

Expected: clean release build. Binary at `target/release/monitor`.

**Step 2: Smoke test — run server and connect via wscat or websocat**

```bash
MONITOR_API_KEY=testkey cargo run
```

In a second terminal (requires `websocat` or `wscat`):

```bash
# Install websocat if needed: cargo install websocat
websocat "ws://127.0.0.1:8080/ws?key=testkey"
```

Expected: JSON frames arriving every 2 seconds with `"event":"metrics"` and a `temperatures` array.

**Step 3: Test interval change**

While connected via websocat, send:

```json
{"action":"set_interval","ms":500}
```

Expected: frames now arrive every ~500ms.

**Step 4: Test auth rejection**

```bash
websocat "ws://127.0.0.1:8080/ws?key=wrongkey"
```

Expected: HTTP 401 response, connection refused.

**Step 5: Test invalid interval**

```json
{"action":"set_interval","ms":999}
```

Expected: server logs `WARN Invalid interval: Invalid interval 999ms. Allowed: 250, 500, 1000, 2000, 5000`, no crash.

**Step 6: Final commit**

```bash
git add -p  # review any unstaged changes
git commit -m "chore: release build verified, WebSocket implementation complete"
```

---

## Windows Build Note

To cross-compile or build natively on Windows:

```powershell
cargo build --release
```

The `wmi` crate links against Windows COM automatically via `winapi`. No extra setup needed beyond a standard Rust Windows toolchain (`x86_64-pc-windows-msvc`). The `#[cfg(target_os = "linux")]` sysfs code is skipped entirely.

On Windows, `sysinfo::Components` may return an empty list unless the process has sufficient privileges — this is expected behavior, not a bug.
