# Remote System Monitor - Rust Server

A lightweight Rust HTTP server that exposes real-time system metrics (CPU, memory, disk, network) as JSON. Designed to be polled by a companion Android app over local WiFi.

## Prerequisites

- [Rust](https://rustup.rs/) (1.70+ recommended)
- **Windows:** Visual Studio Build Tools with the "C++ build tools" workload (required for the MSVC linker)
- **Linux (Debian/Ubuntu):** `sudo apt install build-essential pkg-config`
- **Linux (Fedora/RHEL):** `sudo dnf groupinstall "Development Tools"` and `sudo dnf install pkg-config`
- **Linux (Arch):** `sudo pacman -S base-devel`
- **NixOS / Nix:** Use the provided `shell.nix` (see [NixOS section](#nixos--nix)) or `nix-shell -p rustc cargo gcc pkg-config`

## Setup

### 1. Clone the repository

```bash
git clone <your-repo-url>
cd remote-system-monitor-rust-server
```

### 2. Set the API key

Every request to the server requires an `X-API-Key` header. You must set the key as an environment variable before running.

**PowerShell:**

```powershell
$env:MONITOR_API_KEY = "your-secret-key-here"
```

**Bash / Linux / macOS:**

```bash
export MONITOR_API_KEY="your-secret-key-here"
```

Pick any string you like as your key. You'll use this same key in your Android app.

### 3. Build and run

**Debug mode (faster compile, slower runtime):**

```bash
cargo run
```

**Release mode (slower compile, optimized binary):**

```bash
cargo run --release
```

On startup you'll see:

```
====================================================
  Remote System Monitor - Rust Server
====================================================
  Local URL:   http://localhost:8080
  Network URL: http://192.168.x.x:8080  <- use in Android app
  API Key:     your-sec...
  Header:      X-API-Key: <your-key>
  Optimised for 2s polling over local WiFi
====================================================
```

Use the **Network URL** in your Android app. Both your PC and phone must be on the same WiFi network.

### 4. Allow through firewall

#### Windows

Windows Firewall may block incoming connections on port 8080. To allow it:

1. Open **Windows Defender Firewall** > **Advanced Settings**
2. Click **Inbound Rules** > **New Rule...**
3. Select **Port** > Next
4. Select **TCP**, enter **8080** > Next
5. Select **Allow the connection** > Next
6. Check **Private** (uncheck Domain and Public) > Next
7. Name it `System Monitor Server` > Finish

Or via PowerShell (run as Administrator):

```powershell
New-NetFirewallRule -DisplayName "System Monitor Server" -Direction Inbound -LocalPort 8080 -Protocol TCP -Action Allow -Profile Private
```

#### Linux

**ufw (Ubuntu/Debian):**

```bash
sudo ufw allow 8080/tcp
```

**firewalld (Fedora/RHEL):**

```bash
sudo firewall-cmd --add-port=8080/tcp --permanent
sudo firewall-cmd --reload
```

**iptables (any distro):**

```bash
sudo iptables -A INPUT -p tcp --dport 8080 -j ACCEPT
```

**NixOS (via configuration.nix):**

```nix
networking.firewall.allowedTCPPorts = [ 8080 ];
```

Then rebuild: `sudo nixos-rebuild switch`

## API Endpoints

All endpoints return JSON. Every request must include the `X-API-Key` header.

| Endpoint               | Description                                                              |
| ---------------------- | ------------------------------------------------------------------------ |
| `GET /`                | Server info and list of available endpoints                              |
| `GET /health`          | Health check (status, timestamp, version)                                |
| `GET /metrics`         | Full system snapshot (all metrics below combined)                        |
| `GET /metrics/cpu`     | CPU usage %, core count, frequency, per-core usage                       |
| `GET /metrics/memory`  | RAM and swap: total, used, available, usage %                            |
| `GET /metrics/gpu`     | Per-GPU: name, vendor, temperature, utilization, VRAM, fan, power, clock |
| `GET /metrics/disk`    | Per-disk: name, mountpoint, filesystem, total/used/free, usage %         |
| `GET /metrics/network` | Upload/download speed (Mbps), total sent/received, packet counts         |

## Testing the API

**curl:**

```bash
curl -H "X-API-Key: your-secret-key-here" http://localhost:8080/metrics
```

**PowerShell:**

```powershell
Invoke-RestMethod -Uri http://localhost:8080/metrics -Headers @{"X-API-Key"="your-secret-key-here"}
```

**Browser:** Won't work directly (no way to set custom headers). Use curl, Postman, or your Android app.

### Example response (`/metrics`)

```json
{
  "timestamp": "2026-03-08T12:00:00+05:30",
  "system": {
    "hostname": "MY-PC",
    "os": "Windows",
    "os_version": "11 (26100)",
    "kernel_version": "26100",
    "architecture": "x86_64",
    "uptime": "5h 30m",
    "uptime_seconds": 19800,
    "boot_time": "2026-03-08 06:30:00"
  },
  "cpu": {
    "usage_percent": 15.3,
    "core_count_logical": 16,
    "core_count_physical": 8,
    "frequency_mhz": 3600,
    "per_core_percent": [12.1, 18.5, ...]
  },
  "memory": {
    "total_gb": 15.87,
    "used_gb": 8.42,
    "available_gb": 7.45,
    "usage_percent": 53.1,
    "swap_total_gb": 4.0,
    "swap_used_gb": 0.5,
    "swap_percent": 12.5
  },
  "gpu": [
    {
      "index": 0,
      "name": "NVIDIA GeForce RTX 3080",
      "vendor": "NVIDIA",
      "temperature_celsius": 65,
      "utilization_percent": 42,
      "memory_total_mb": 10240,
      "memory_used_mb": 3584,
      "memory_usage_percent": 35.0,
      "fan_speed_percent": 55,
      "power_draw_watts": 220.5,
      "clock_speed_mhz": 1905
    }
  ],
  "disk": [
    {
      "name": "C:",
      "mountpoint": "C:\\",
      "file_system": "NTFS",
      "total_gb": 476.34,
      "used_gb": 210.12,
      "free_gb": 266.22,
      "usage_percent": 44.1,
      "is_removable": false
    }
  ],
  "network": {
    "upload_speed_mbps": 0.125,
    "download_speed_mbps": 2.34,
    "total_sent_gb": 1.234,
    "total_recv_gb": 5.678,
    "packets_sent": 123456,
    "packets_recv": 789012
  }
}
```

## Android App Integration

In your Android HTTP client, add the API key header to every request:

```kotlin
val request = Request.Builder()
    .url("http://192.168.x.x:8080/metrics")
    .addHeader("X-API-Key", "your-secret-key-here")
    .build()
```

Poll `/metrics` every 2 seconds for a real-time dashboard, or use individual endpoints (`/metrics/cpu`, etc.) if you only need specific data.

## Building a Standalone Binary

```bash
cargo build --release
```

The optimized binary will be at:

- **Windows:** `target/release/monitor.exe`
- **Linux:** `target/release/monitor`

The release profile enables LTO and strips debug symbols for a small binary size. You can copy the binary to any machine with the same OS and run it directly (no Rust installation needed on the target).

### Running as a systemd service (Linux)

To keep the server running in the background and auto-start on boot:

1. Create the service file:

```bash
sudo nano /etc/systemd/system/system-monitor.service
```

2. Paste the following (adjust paths and API key):

```ini
[Unit]
Description=Remote System Monitor Server
After=network.target

[Service]
Type=simple
Environment=MONITOR_API_KEY=your-secret-key-here
ExecStart=/path/to/monitor
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

3. Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable system-monitor
sudo systemctl start system-monitor
```

4. Check status:

```bash
sudo systemctl status system-monitor
```

### NixOS / Nix

#### Development shell

Enter a shell with all build dependencies:

```bash
nix-shell
```

This uses the `shell.nix` at the repo root. Then build and run as usual:

```bash
export MONITOR_API_KEY="your-secret-key-here"
cargo run --release
```

#### Running as a NixOS service

Add the following to your `/etc/nixos/configuration.nix` (adjust the binary path):

```nix
systemd.services.system-monitor = {
  description = "Remote System Monitor Server";
  after = [ "network.target" ];
  wantedBy = [ "multi-user.target" ];
  environment = {
    MONITOR_API_KEY = "your-secret-key-here";
  };
  serviceConfig = {
    ExecStart = "/path/to/monitor";
    Restart = "on-failure";
    RestartSec = 5;
  };
};

networking.firewall.allowedTCPPorts = [ 8080 ];
```

Then rebuild:

```bash
sudo nixos-rebuild switch
```
