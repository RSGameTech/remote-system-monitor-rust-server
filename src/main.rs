use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{
    extract::State,
    response::Json,
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
    disk: Vec<DiskInfo>,
    network: NetworkInfo,
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

fn uptime_string(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    format!("{}h {}m", h, m)
}

// ─── Route Handlers ───────────────────────────────────────────────────────────

async fn root() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "message": "Remote System Monitor (Rust) is running",
        "version": "1.0.0",
        "endpoints": ["/metrics", "/metrics/cpu", "/metrics/memory", "/metrics/disk", "/metrics/network", "/health"]
    }))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy",
        timestamp: Local::now().to_rfc3339(),
        version: "1.0.0",
    })
}

async fn get_metrics(State(state): State<Arc<AppState>>) -> Json<MetricsResponse> {
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
    let disk_info = build_disk_info(&disks);
    let network_info = build_network_info(&networks, &mut state.prev_net.lock().unwrap());
    let system_info = build_system_info(&sys);

    Json(MetricsResponse {
        timestamp: Local::now().to_rfc3339(),
        system: system_info,
        cpu: cpu_info,
        memory: memory_info,
        disk: disk_info,
        network: network_info,
    })
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
        usage_percent: (total_usage * 10.0).round() / 10.0,
        core_count_logical: logical,
        core_count_physical: physical,
        frequency_mhz: freq,
        per_core_percent: per_core.iter().map(|v| (v * 10.0).round() / 10.0).collect(),
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
        usage_percent: (usage_percent * 10.0).round() / 10.0,
        swap_total_gb: round2(bytes_to_gb(swap_total)),
        swap_used_gb: round2(bytes_to_gb(swap_used)),
        swap_percent: (swap_percent * 10.0).round() / 10.0,
    }
}

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
                usage_percent: (usage_percent * 10.0).round() / 10.0,
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

    let state = Arc::new(AppState {
        sys: Mutex::new(sys),
        disks: Mutex::new(Disks::new_with_refreshed_list()),
        networks: Mutex::new(networks),
        prev_net: Mutex::new(NetSnapshot {
            bytes_sent: init_sent,
            bytes_recv: init_recv,
            taken_at: Instant::now(),
        }),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        .route("/", get(root))
        .route("/health", get(health))
        .route("/metrics", get(get_metrics))
        .route("/metrics/cpu", get(get_cpu))
        .route("/metrics/memory", get(get_memory))
        .route("/metrics/disk", get(get_disk))
        .route("/metrics/network", get(get_network))
        .layer(cors)
        .with_state(state);

    // Print startup banner with local IP
    let local_ip = get_local_ip();
    let port = 8080u16;

    println!("\n{}", "=".repeat(52));
    println!("  Remote System Monitor — Rust Server");
    println!("{}", "=".repeat(52));
    println!("  Local URL:   http://localhost:{}", port);
    println!("  Network URL: http://{}:{}  ← use in Android app", local_ip, port);
    println!("  Optimised for 2s polling over local WiFi");
    println!("{}\n", "=".repeat(52));

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
