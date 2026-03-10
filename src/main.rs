use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::{ws::WebSocketUpgrade, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use chrono::Local;
use serde::Serialize;
use sysinfo::{CpuRefreshKind, Disks, Networks, RefreshKind, System};
use tower_http::cors::{Any, CorsLayer};
use tracing::info;

// ─── Shared State ─────────────────────────────────────────────────────────────

struct AppState {
    sys: Mutex<System>,
    disks: Mutex<Disks>,
    networks: Mutex<Networks>,
    /// Tracks previous network bytes for speed calculation
    prev_net: Mutex<NetSnapshot>,
    /// NVML handle for NVIDIA GPU monitoring (None if unavailable)
    nvml: Option<nvml_wrapper::Nvml>,
    api_key: String,
}

struct NetSnapshot {
    bytes_sent: u64,
    bytes_recv: u64,
    taken_at: Instant,
}

// ─── Response Models ──────────────────────────────────────────────────────────

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

#[derive(Serialize)]
struct SystemInfo {
    hostname: String,
    os: String,
    os_version: String,
    kernel_version: String,
    architecture: String,
    uptime: String,
    uptime_seconds: u64,
    boot_time: String,
}

#[derive(Serialize)]
struct CpuInfo {
    usage_percent: f32,
    core_count_logical: usize,
    core_count_physical: usize,
    frequency_mhz: u64,
    per_core_percent: Vec<f32>,
}

#[derive(Serialize)]
struct MemoryInfo {
    total_gb: f64,
    used_gb: f64,
    available_gb: f64,
    usage_percent: f32,
    swap_total_gb: f64,
    swap_used_gb: f64,
    swap_percent: f32,
}

#[derive(Serialize)]
struct GpuInfo {
    index: u32,
    name: String,
    vendor: String,
    temperature_celsius: Option<u32>,
    utilization_percent: Option<u32>,
    memory_total_mb: u64,
    memory_used_mb: u64,
    memory_usage_percent: f32,
    fan_speed_percent: Option<u32>,
    power_draw_watts: Option<f64>,
    clock_speed_mhz: Option<u32>,
}

#[derive(Serialize)]
struct DiskInfo {
    name: String,
    mountpoint: String,
    file_system: String,
    total_gb: f64,
    used_gb: f64,
    free_gb: f64,
    usage_percent: f32,
    is_removable: bool,
}

#[derive(Serialize)]
struct NetworkInfo {
    upload_speed_mbps: f64,
    download_speed_mbps: f64,
    total_sent_gb: f64,
    total_recv_gb: f64,
    packets_sent: u64,
    packets_recv: u64,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
    timestamp: String,
    version: &'static str,
}

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

// ─── Metric Helpers ───────────────────────────────────────────────────────────

fn bytes_to_gb(bytes: u64) -> f64 {
    bytes as f64 / 1_073_741_824.0
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn round1_f32(v: f32) -> f32 {
    (v * 10.0).round() / 10.0
}

fn uptime_string(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    format!("{}h {}m", h, m)
}

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
            if process.kill() {
                Ok(())
            } else {
                Err(format!("Failed to send kill signal to PID {}", pid))
            }
        }
        None => Err(format!("PID {} not found", pid)),
    }
}

// ─── Auth Middleware ──────────────────────────────────────────────────────────

async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    match headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
        Some(key) if key == state.api_key => next.run(request).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "missing or invalid API key" })),
        )
            .into_response(),
    }
}

// ─── Route Handlers ───────────────────────────────────────────────────────────

async fn root() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "message": "Remote System Monitor (Rust) is running",
        "version": "1.0.0",
        "endpoints": ["/metrics", "/metrics/cpu", "/metrics/memory", "/metrics/gpu", "/metrics/disk", "/metrics/network", "/health"]
    }))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy",
        timestamp: Local::now().to_rfc3339(),
        version: "1.0.0",
    })
}

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

async fn get_metrics(State(state): State<Arc<AppState>>) -> Json<MetricsResponse> {
    Json(collect_full_metrics(&state).await)
}

async fn get_cpu(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let mut sys = state.sys.lock().unwrap();
    sys.refresh_specifics(RefreshKind::new().with_cpu(CpuRefreshKind::everything()));
    Json(serde_json::json!({
        "timestamp": Local::now().to_rfc3339(),
        "cpu": build_cpu_info(&sys)
    }))
}

async fn get_memory(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let mut sys = state.sys.lock().unwrap();
    sys.refresh_memory();
    Json(serde_json::json!({
        "timestamp": Local::now().to_rfc3339(),
        "memory": build_memory_info(&sys)
    }))
}

async fn get_gpu(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "timestamp": Local::now().to_rfc3339(),
        "gpu": build_gpu_info(&state.nvml)
    }))
}

async fn get_disk(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let mut disks = state.disks.lock().unwrap();
    disks.refresh();
    Json(serde_json::json!({
        "timestamp": Local::now().to_rfc3339(),
        "disk": build_disk_info(&disks)
    }))
}

async fn get_network(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let mut networks = state.networks.lock().unwrap();
    networks.refresh();
    let net = build_network_info(&networks, &mut state.prev_net.lock().unwrap());
    Json(serde_json::json!({
        "timestamp": Local::now().to_rfc3339(),
        "network": net
    }))
}

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
    use futures_util::{SinkExt, StreamExt};
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
                    let metrics = collect_full_metrics(&state_clone).await;
                    let msg = ServerMessage::Metrics { data: metrics };
                    let json = serde_json::to_string(&msg).unwrap();

                    if sender.send(Message::Text(json)).await.is_err() {
                        break; // client disconnected
                    }
                }
                _ = interval_rx.changed() => {
                    current_ms = *interval_rx.borrow_and_update();
                    interval = tokio::time::interval(
                        std::time::Duration::from_millis(current_ms)
                    );
                    interval.tick().await; // skip immediate first tick after change
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

// ─── Builders ─────────────────────────────────────────────────────────────────

fn build_system_info(_sys: &System) -> SystemInfo {
    let uptime_secs = System::uptime();
    let boot_ts = System::boot_time();
    let boot_dt = chrono::DateTime::from_timestamp(boot_ts as i64, 0)
        .map(|dt| dt.with_timezone(&Local).format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "unknown".to_string());

    SystemInfo {
        hostname: System::host_name().unwrap_or_else(|| "unknown".to_string()),
        os: System::name().unwrap_or_else(|| "unknown".to_string()),
        os_version: System::os_version().unwrap_or_else(|| "unknown".to_string()),
        kernel_version: System::kernel_version().unwrap_or_else(|| "unknown".to_string()),
        architecture: std::env::consts::ARCH.to_string(),
        uptime: uptime_string(uptime_secs),
        uptime_seconds: uptime_secs,
        boot_time: boot_dt,
    }
}

fn build_cpu_info(sys: &System) -> CpuInfo {
    let cpus = sys.cpus();
    let total_usage = sys.global_cpu_info().cpu_usage();
    let per_core: Vec<f32> = cpus.iter().map(|c| c.cpu_usage()).collect();
    let freq = cpus.first().map(|c| c.frequency()).unwrap_or(0);
    let logical = cpus.len();
    let physical = sys.physical_core_count().unwrap_or(logical / 2);

    CpuInfo {
        usage_percent: round1_f32(total_usage),
        core_count_logical: logical,
        core_count_physical: physical,
        frequency_mhz: freq,
        per_core_percent: per_core.iter().map(|v| round1_f32(*v)).collect(),
    }
}

fn build_memory_info(sys: &System) -> MemoryInfo {
    let total = sys.total_memory();
    let used = sys.used_memory();
    let available = sys.available_memory();
    let swap_total = sys.total_swap();
    let swap_used = sys.used_swap();

    let usage_percent = if total > 0 {
        (used as f64 / total as f64 * 100.0) as f32
    } else {
        0.0
    };

    let swap_percent = if swap_total > 0 {
        (swap_used as f64 / swap_total as f64 * 100.0) as f32
    } else {
        0.0
    };

    MemoryInfo {
        total_gb: round2(bytes_to_gb(total)),
        used_gb: round2(bytes_to_gb(used)),
        available_gb: round2(bytes_to_gb(available)),
        usage_percent: round1_f32(usage_percent),
        swap_total_gb: round2(bytes_to_gb(swap_total)),
        swap_used_gb: round2(bytes_to_gb(swap_used)),
        swap_percent: round1_f32(swap_percent),
    }
}

fn build_temperature_info() -> Vec<TemperatureInfo> {
    let components = sysinfo::Components::new_with_refreshed_list();
    components
        .list()
        .iter()
        .map(|c| TemperatureInfo {
            component: c.label().to_string(),
            temperature_celsius: c.temperature(),
            max_celsius: Some(c.max()).filter(|v| v.is_finite() && *v > 0.0),
            critical_celsius: c.critical(),
        })
        .collect()
}

// ─── GPU Support ──────────────────────────────────────────────────────────────

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

fn collect_nvidia_gpus(nvml: &nvml_wrapper::Nvml) -> Vec<GpuInfo> {
    let count = match nvml.device_count() {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    (0..count)
        .filter_map(|i| {
            let dev = nvml.device_by_index(i).ok()?;
            let mem = dev.memory_info().ok();
            let (mem_total, mem_used, mem_pct) = mem
                .map(|m| {
                    let t = m.total / 1_048_576;
                    let u = m.used / 1_048_576;
                    let p = if m.total > 0 {
                        (m.used as f64 / m.total as f64 * 100.0) as f32
                    } else {
                        0.0
                    };
                    (t, u, round1_f32(p))
                })
                .unwrap_or((0, 0, 0.0));

            Some(GpuInfo {
                index: i,
                name: dev.name().unwrap_or_else(|_| "Unknown NVIDIA GPU".into()),
                vendor: "NVIDIA".into(),
                temperature_celsius: dev
                    .temperature(
                        nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu,
                    )
                    .ok(),
                utilization_percent: dev.utilization_rates().ok().map(|u| u.gpu),
                memory_total_mb: mem_total,
                memory_used_mb: mem_used,
                memory_usage_percent: mem_pct,
                fan_speed_percent: dev.fan_speed(0).ok(),
                power_draw_watts: dev
                    .power_usage()
                    .ok()
                    .map(|mw| round2(mw as f64 / 1000.0)),
                clock_speed_mhz: dev
                    .clock_info(nvml_wrapper::enum_wrappers::device::Clock::Graphics)
                    .ok(),
            })
        })
        .collect()
}

/// Scan /sys/class/drm for AMD (0x1002) and Intel (0x8086) GPUs.
#[cfg(target_os = "linux")]
fn collect_sysfs_gpus(start_idx: u32) -> Vec<GpuInfo> {
    use std::fs;
    use std::path::Path;

    let drm = Path::new("/sys/class/drm");
    if !drm.exists() {
        return Vec::new();
    }

    let mut entries: Vec<_> = fs::read_dir(drm)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            n.starts_with("card") && !n.contains('-')
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    let mut gpus = Vec::new();

    for entry in &entries {
        let card_path = entry.path();
        let dev = card_path.join("device");

        let vendor = fs::read_to_string(dev.join("vendor"))
            .unwrap_or_default()
            .trim()
            .to_string();

        let (vendor_name, default_name) = match vendor.as_str() {
            "0x1002" => ("AMD", "AMD GPU"),
            "0x8086" => ("Intel", "Intel GPU"),
            _ => continue,
        };

        let name = fs::read_to_string(dev.join("product_name"))
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default_name.into());

        let temp = sysfs_hwmon_read(&dev, "temp1_input").map(|v| (v / 1000) as u32);

        // Utilization: AMD exposes gpu_busy_percent, Intel does not via sysfs
        let util = sysfs_read_u64(&dev.join("gpu_busy_percent")).map(|v| v as u32);

        // VRAM: AMD has mem_info_vram_*; Intel iGPUs use system RAM (no dedicated VRAM)
        // For Intel, try mem_info_vram_* anyway (Intel Arc dGPUs expose it)
        let vram_total =
            sysfs_read_u64(&dev.join("mem_info_vram_total")).map(|b| b / 1_048_576);
        let vram_used =
            sysfs_read_u64(&dev.join("mem_info_vram_used")).map(|b| b / 1_048_576);

        let (mt, mu, mp) = match (vram_total, vram_used) {
            (Some(t), Some(u)) => {
                let p = if t > 0 {
                    (u as f64 / t as f64 * 100.0) as f32
                } else {
                    0.0
                };
                (t, u, round1_f32(p))
            }
            _ => (0, 0, 0.0),
        };

        let fan = sysfs_hwmon_fan_pct(&dev);
        let power = sysfs_hwmon_read(&dev, "power1_average")
            .map(|uw| round2(uw as f64 / 1_000_000.0));

        // Clock speed: AMD uses pp_dpm_sclk, Intel uses gt_cur_freq_mhz
        let clock = if vendor_name == "AMD" {
            sysfs_amd_active_clock(&dev.join("pp_dpm_sclk"))
        } else {
            // Intel: gt_cur_freq_mhz is on the card dir, not device dir
            sysfs_read_u64(&card_path.join("gt_cur_freq_mhz")).map(|v| v as u32)
        };

        gpus.push(GpuInfo {
            index: start_idx + gpus.len() as u32,
            name,
            vendor: vendor_name.into(),
            temperature_celsius: temp,
            utilization_percent: util,
            memory_total_mb: mt,
            memory_used_mb: mu,
            memory_usage_percent: mp,
            fan_speed_percent: fan,
            power_draw_watts: power,
            clock_speed_mhz: clock,
        });
    }

    gpus
}

/// Read a value from the first hwmon directory under a device path.
#[cfg(target_os = "linux")]
fn sysfs_hwmon_read(device_path: &std::path::Path, filename: &str) -> Option<u64> {
    let hwmon = device_path.join("hwmon");
    for entry in std::fs::read_dir(&hwmon).ok()?.flatten() {
        if let Some(v) = sysfs_read_u64(&entry.path().join(filename)) {
            return Some(v);
        }
    }
    None
}

/// Read fan speed as a percentage from hwmon (fan1_input / fan1_max * 100).
#[cfg(target_os = "linux")]
fn sysfs_hwmon_fan_pct(device_path: &std::path::Path) -> Option<u32> {
    let hwmon = device_path.join("hwmon");
    for entry in std::fs::read_dir(&hwmon).ok()?.flatten() {
        let p = entry.path();
        let cur = sysfs_read_u64(&p.join("fan1_input"))?;
        let max = sysfs_read_u64(&p.join("fan1_max")).filter(|&m| m > 0)?;
        return Some((cur * 100 / max) as u32);
    }
    None
}

/// Parse the active clock from AMD's pp_dpm_sclk (line with `*` suffix).
#[cfg(target_os = "linux")]
fn sysfs_amd_active_clock(path: &std::path::Path) -> Option<u32> {
    let content = std::fs::read_to_string(path).ok()?;
    for line in content.lines() {
        if line.contains('*') {
            return line
                .split_whitespace()
                .find(|s| s.ends_with("Mhz") || s.ends_with("MHz"))
                .and_then(|s| {
                    s.trim_end_matches("Mhz")
                        .trim_end_matches("MHz")
                        .parse()
                        .ok()
                });
        }
    }
    None
}

/// Read a u64 value from a sysfs file.
#[cfg(target_os = "linux")]
fn sysfs_read_u64(path: &std::path::Path) -> Option<u64> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse()
        .ok()
}

#[cfg(target_os = "windows")]
fn collect_wmi_gpus(start_idx: u32) -> Vec<GpuInfo> {
    use serde::Deserialize;
    use wmi::{COMLibrary, WMIConnection};

    #[derive(Deserialize)]
    #[allow(non_snake_case)]
    struct Win32VideoController {
        Name: String,
        AdapterCompatibility: Option<String>,
        AdapterRAM: Option<u64>,
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

    let mut result: Vec<GpuInfo> = controllers
        .into_iter()
        .filter_map(|c| {
            let compat = c.AdapterCompatibility.as_deref().unwrap_or("").to_lowercase();
            let vendor = if compat.contains("intel") {
                "Intel"
            } else if compat.contains("amd") || compat.contains("advanced micro") {
                "AMD"
            } else {
                return None; // skip NVIDIA (handled by NVML) and unknowns
            };

            let mem_mb = c.AdapterRAM.map(|b| b / 1_048_576).unwrap_or(0);

            Some(GpuInfo {
                index: 0, // assigned below
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
        .collect();

    for (i, gpu) in result.iter_mut().enumerate() {
        gpu.index = start_idx + i as u32;
    }
    result
}

// ─── Disk & Network Builders ─────────────────────────────────────────────────

fn build_disk_info(disks: &Disks) -> Vec<DiskInfo> {
    disks
        .list()
        .iter()
        .map(|d| {
            let total = d.total_space();
            let free = d.available_space();
            let used = total.saturating_sub(free);
            let usage_percent = if total > 0 {
                (used as f64 / total as f64 * 100.0) as f32
            } else {
                0.0
            };

            DiskInfo {
                name: d.name().to_string_lossy().to_string(),
                mountpoint: d.mount_point().to_string_lossy().to_string(),
                file_system: d.file_system().to_string_lossy().to_string(),
                total_gb: round2(bytes_to_gb(total)),
                used_gb: round2(bytes_to_gb(used)),
                free_gb: round2(bytes_to_gb(free)),
                usage_percent: round1_f32(usage_percent),
                is_removable: d.is_removable(),
            }
        })
        .collect()
}

fn build_network_info(networks: &Networks, prev: &mut NetSnapshot) -> NetworkInfo {
    let (total_sent, total_recv, packets_sent, packets_recv) = networks
        .list()
        .iter()
        .fold((0u64, 0u64, 0u64, 0u64), |acc, (_, data)| {
            (
                acc.0 + data.total_transmitted(),
                acc.1 + data.total_received(),
                acc.2 + data.total_packets_transmitted(),
                acc.3 + data.total_packets_received(),
            )
        });

    let elapsed = prev.taken_at.elapsed().as_secs_f64();
    let sent_diff = total_sent.saturating_sub(prev.bytes_sent);
    let recv_diff = total_recv.saturating_sub(prev.bytes_recv);

    let upload_mbps = if elapsed > 0.0 {
        (sent_diff as f64 / elapsed) / 1_048_576.0
    } else {
        0.0
    };
    let download_mbps = if elapsed > 0.0 {
        (recv_diff as f64 / elapsed) / 1_048_576.0
    } else {
        0.0
    };

    // Update snapshot
    prev.bytes_sent = total_sent;
    prev.bytes_recv = total_recv;
    prev.taken_at = Instant::now();

    NetworkInfo {
        upload_speed_mbps: round3(upload_mbps),
        download_speed_mbps: round3(download_mbps),
        total_sent_gb: round3(bytes_to_gb(total_sent)),
        total_recv_gb: round3(bytes_to_gb(total_recv)),
        packets_sent,
        packets_recv,
    }
}

// ─── Main ─────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_target(false)
        .compact()
        .init();

    let api_key = std::env::var("MONITOR_API_KEY")
        .expect("MONITOR_API_KEY env variable must be set");

    // Warm up sysinfo — first CPU read is always 0 without a prior sample
    let mut sys = System::new_all();
    sys.refresh_all();
    tokio::time::sleep(Duration::from_millis(200)).await;
    sys.refresh_cpu_usage();

    let networks = Networks::new_with_refreshed_list();
    let (init_sent, init_recv) = networks
        .list()
        .iter()
        .fold((0u64, 0u64), |acc, (_, d)| {
            (acc.0 + d.total_transmitted(), acc.1 + d.total_received())
        });

    // Initialize NVML for NVIDIA GPU monitoring
    let nvml = match nvml_wrapper::Nvml::init() {
        Ok(n) => {
            let count = n.device_count().unwrap_or(0);
            info!("NVML initialized — {} NVIDIA GPU(s) detected", count);
            Some(n)
        }
        Err(_) => {
            info!("NVML unavailable — no NVIDIA GPU support");
            None
        }
    };

    // Check for AMD/Intel GPUs on Linux
    #[cfg(target_os = "linux")]
    {
        let sysfs_gpus = collect_sysfs_gpus(0);
        for gpu in &sysfs_gpus {
            info!("{} GPU detected via sysfs: {}", gpu.vendor, gpu.name);
        }
    }

    let state = Arc::new(AppState {
        sys: Mutex::new(sys),
        disks: Mutex::new(Disks::new_with_refreshed_list()),
        networks: Mutex::new(networks),
        prev_net: Mutex::new(NetSnapshot {
            bytes_sent: init_sent,
            bytes_recv: init_recv,
            taken_at: Instant::now(),
        }),
        nvml,
        api_key,
    });

    // Print startup banner with local IP
    let local_ip = get_local_ip();
    let port = 8080u16;

    let gpu_status = {
        let gpus = build_gpu_info(&state.nvml);
        if gpus.is_empty() {
            "none detected".to_string()
        } else {
            gpus.iter()
                .map(|g| format!("{} ({})", g.name, g.vendor))
                .collect::<Vec<_>>()
                .join(", ")
        }
    };

    println!("\n{}", "=".repeat(52));
    println!("  Remote System Monitor — Rust Server");
    println!("{}", "=".repeat(52));
    println!("  Local URL:   http://localhost:{}", port);
    println!(
        "  Network URL: http://{}:{}  ← use in Android app",
        local_ip, port
    );
    println!(
        "  WebSocket:   ws://{}:{}/ws  ← real-time feed",
        local_ip, port
    );
    println!(
        "  API Key:     {}...",
        &state.api_key[..8.min(state.api_key.len())]
    );
    println!("  Header:      X-API-Key: <your-key>");
    println!("  GPU:         {}", gpu_status);
    println!("  WebSocket refresh: 250ms / 500ms / 1s / 2s (default) / 5s");
    println!("{}\n", "=".repeat(52));

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

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

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    info!("Listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

fn get_local_ip() -> String {
    // Attempt UDP connect trick to find outbound interface IP
    use std::net::UdpSocket;
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| {
            s.connect("8.8.8.8:80")?;
            s.local_addr()
        })
        .map(|addr| addr.ip().to_string())
        .unwrap_or_else(|_| "127.0.0.1".to_string())
}
