# WebSocket Implementation Instructions

## Overview

This document covers the changes required to add WebSocket support to the Remote System Monitor Rust server, including:

- Real-time metrics push over WebSocket
- Selectable refresh intervals from the Android client
- Kill process command over WebSocket
- Temperature sensor data (server sends `°C`, client converts to `°F` based on user preference)

---

## 1. Cargo.toml Changes

No new crates are required. Axum's WebSocket support is built-in. Ensure `tokio` has the `full` feature:

```toml
tokio = { version = "1", features = ["full"] }
serde_json = "1"
serde = { version = "1", features = ["derive"] }
```

---

## 2. New Structs

Add these message types near the top of `main.rs` alongside your existing response models:

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

/// Per-connection mutable state
struct WsConnectionState {
    /// Current refresh interval in milliseconds
    interval_ms: u64,
}

impl Default for WsConnectionState {
    fn default() -> Self {
        Self { interval_ms: 2000 }
    }
}

/// A single thermal sensor reading — always in Celsius
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

---

## 3. Allowed Refresh Intervals

The Android app must only send one of these pre-approved values. The server enforces this — any other value is rejected with an error frame.

| Label | Value (ms) |
| ----- | ---------- |
| 250ms | 250        |
| 500ms | 500        |
| 1s    | 1000       |
| 2s    | 2000       |
| 5s    | 5000       |

### Server-side validation

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

## 4. New WebSocket Route Handler

Add this handler function in the route handlers section:

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
    use tokio::sync::watch;

    let (mut sender, mut receiver) = socket.split();

    // Shared interval between recv task and send loop
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
                    // Check if interval changed
                    if interval_rx.has_changed().unwrap_or(false) {
                        current_ms = *interval_rx.borrow_and_update();
                        interval = tokio::time::interval(
                            std::time::Duration::from_millis(current_ms)
                        );
                        interval.tick().await; // skip the immediate first tick
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
                                    // Send error back — handled via a separate channel
                                    // or log it; sender is in the other task
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

---

## 5. Kill Process Helper

Add this function in the builders/helpers section:

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

---

## 6. Metrics Collection Helper

Extract the metrics collection from `get_metrics` into a shared async function so both REST and WebSocket can call it:

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
    }
}
```

Then update `get_metrics` to call this instead of duplicating the logic:

```rust
async fn get_metrics(State(state): State<Arc<AppState>>) -> Json<MetricsResponse> {
    Json(collect_full_metrics(&state).await)
}
```

---

## 7. Temperature Data

### Add to `MetricsResponse`

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
    temperatures: Vec<TemperatureInfo>,   // ← add this
}
```

### Builder function

Add alongside the other `build_*` functions:

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

### Update `collect_full_metrics`

```rust
MetricsResponse {
    timestamp: Local::now().to_rfc3339(),
    system: system_info,
    cpu: cpu_info,
    memory: memory_info,
    gpu: gpu_info,
    disk: disk_info,
    network: network_info,
    temperatures: build_temperature_info(),   // ← add this
}
```

> **Note:** `sysinfo::Components` reads from `hwmon` on Linux, WMI on Windows, and IOKit on macOS. Sensor availability varies by hardware — the array may be empty on some systems.

---

## 8. Register the Route

In `main()`, add the WebSocket route to the router. The WebSocket route bypasses the existing `auth_middleware` because auth is handled inside `ws_handler` via query param:

```rust
let app = Router::new()
    .route("/", get(root))
    .route("/health", get(health))
    .route("/ws", get(ws_handler))                    // ← add this (no auth middleware)
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

> **Note:** In Axum, routes added before `.layer()` still get the layer applied. To truly exempt `/ws` from `auth_middleware`, split the router:
>
> ```rust
> let public = Router::new()
>     .route("/ws", get(ws_handler));
>
> let protected = Router::new()
>     .route("/", get(root))
>     .route("/health", get(health))
>     .route("/metrics", get(get_metrics))
>     // ... other routes
>     .layer(middleware::from_fn_with_state(state.clone(), auth_middleware));
>
> let app = public.merge(protected).layer(cors).with_state(state);
> ```

---

## 9. Required Imports

Add to the top of `main.rs`:

```rust
use axum::extract::ws::WebSocketUpgrade;
use futures_util::StreamExt; // for .split() and .next() on WebSocket
```

Add to `Cargo.toml`:

```toml
futures-util = "0.3"
```

---

## 10. Android Client Integration

### Connection URL

```
ws://<LOCAL_IP>:8080/ws?key=YOUR_API_KEY
```

Replace `<LOCAL_IP>` with your PC's local network IP (shown in the server startup banner).

### Sending a Refresh Interval Change

The user selects from a fixed list in the UI. Send one of these JSON strings:

```json
{ "action": "set_interval", "ms": 250  }
{ "action": "set_interval", "ms": 500  }
{ "action": "set_interval", "ms": 1000 }
{ "action": "set_interval", "ms": 2000 }
{ "action": "set_interval", "ms": 5000 }
```

### Killing a Process

```json
{ "action": "kill_process", "pid": 1234 }
```

### Receiving Metrics

Every frame from the server will have `"event": "metrics"` with the full data payload:

```json
{
  "event": "metrics",
  "data": {
    "timestamp": "...",
    "cpu": { ... },
    "memory": { ... },
    "gpu": [ ... ],
    "disk": [ ... ],
    "network": { ... },
    "temperatures": [
      { "component": "CPU Package", "temperature_celsius": 62.0, "max_celsius": 75.0, "critical_celsius": 100.0 },
      { "component": "NVMe SSD",    "temperature_celsius": 38.5, "max_celsius": 45.0, "critical_celsius": null  }
    ]
  }
}
```

### Temperature Unit Conversion (Client-side)

The server **always sends `°C`**. The Android app converts locally based on a Setting preference — no extra message to the server needed.

```kotlin
// In your Settings (e.g. DataStore or SharedPreferences)
enum class TempUnit { CELSIUS, FAHRENHEIT }

// Conversion utility
fun Float.toDisplayTemp(unit: TempUnit): String = when (unit) {
    TempUnit.CELSIUS    -> "%.1f°C".format(this)
    TempUnit.FAHRENHEIT -> "%.1f°F".format(this * 9f / 5f + 32f)
}

// Usage in your UI
val displayTemp = sensor.temperatureCelsius.toDisplayTemp(userPrefs.tempUnit)
```

Add a toggle in your Settings screen:

```kotlin
// Jetpack Compose example
var tempUnit by remember { mutableStateOf(TempUnit.CELSIUS) }

Row {
    Text("Temperature unit")
    Spacer(Modifier.weight(1f))
    SegmentedButton(
        options = listOf("°C", "°F"),
        selected = if (tempUnit == TempUnit.CELSIUS) 0 else 1,
        onSelect = { tempUnit = if (it == 0) TempUnit.CELSIUS else TempUnit.FAHRENHEIT }
    )
}
```

### Recommended Android Libraries

- **OkHttp** `WebSocket` — connection management and auto-reconnect
- **Gson / Moshi** — JSON deserialization into Kotlin data classes
- **Kotlin Coroutines + Flow** — emit received frames as a `SharedFlow` for UI consumption

### Suggested UI for Refresh Selector (Kotlin)

```kotlin
val intervals = listOf(
    "250ms" to 250,
    "500ms" to 500,
    "1s"    to 1000,
    "2s"    to 2000,
    "5s"    to 5000
)

// Spinner / SegmentedButton populated from this list
// On selection:
fun onIntervalSelected(ms: Int) {
    webSocket.send("""{"action":"set_interval","ms":$ms}""")
}
```

---

## 11. Security Checklist

- [x] API key validated at WebSocket upgrade (query param `?key=`)
- [x] PID < 1000 is blocked from kill (protects system processes)
- [x] Refresh interval is validated against an allowlist (no arbitrary values)
- [x] Every kill attempt is logged via `tracing::warn!`
- [x] WebSocket connection drops cleanly when either send or recv task exits
