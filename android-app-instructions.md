# Android Companion App — Server Stats Screen Guide

Guide focused on the **server stats detail screen** — the screen shown when a user taps a server from the list. Covers data models, network layer, polling, and full UI layout with Material 3 Expressive.

---

## Your Existing App Flow

```
Main Screen                          Stats Detail Screen
┌──────────────────────┐             ┌──────────────────────┐
│                      │             │  ← MY-PC             │
│  ┌────────────────┐  │   tap       │                      │
│  │ MY-PC          │──│────────────→│  [System Info Card]  │
│  │ 192.168.1.100  │  │             │  [CPU Card]          │
│  └────────────────┘  │             │  [Memory Card]       │
│                      │             │  [GPU Cards]         │
│  ┌────────────────┐  │             │  [Disk Cards]        │
│  │ WORK-PC        │  │             │  [Network Card]      │
│  │ 192.168.1.101  │  │             │  Status: Connected   │
│  └────────────────┘  │             │  Last update: 2s ago │
│                      │             └──────────────────────┘
│          [+ Add] FAB │
└──────────────────────┘
```

- **Min SDK:** 29 (Android 10)
- **Target SDK:** 36 (Android 16)
- **UI:** Jetpack Compose + Material 3 Expressive
- **Already done:** Extended FAB "Add" → dialog → validation → server list
- **Needed:** Stats detail screen shown when tapping a server item

---

## Table of Contents

1. [API Data Models](#1-api-data-models)
2. [Network Layer](#2-network-layer)
3. [Repository](#3-repository)
4. [ViewModel for Stats Screen](#4-viewmodel-for-stats-screen)
5. [Stats Screen — Full Layout](#5-stats-screen--full-layout)
6. [Reusable Components](#6-reusable-components)
7. [Section Detail Screens](#7-section-detail-screens)
8. [Polling Mechanism](#8-polling-mechanism)
9. [Error Handling](#9-error-handling)
10. [Wiring It Up — Navigation](#10-wiring-it-up--navigation)
11. [Server Behavior Notes](#11-server-behavior-notes)
12. [Full JSON Schema Reference](#12-full-json-schema-reference)

---

## 1. API Data Models

Kotlin data classes mapping **exactly** to the server's JSON. Use `@SerializedName` for snake_case fields.

### Full Metrics Response (`GET /metrics`)

```kotlin
// data/model/MetricsResponse.kt

import com.google.gson.annotations.SerializedName

data class MetricsResponse(
    val timestamp: String,
    val system: SystemInfo,
    val cpu: CpuInfo,
    val memory: MemoryInfo,
    val gpu: List<GpuInfo>,
    val disk: List<DiskInfo>,
    val network: NetworkInfo
)

data class SystemInfo(
    val hostname: String,
    val os: String,
    @SerializedName("os_version")     val osVersion: String,
    @SerializedName("kernel_version") val kernelVersion: String,
    val architecture: String,
    val uptime: String,               // human-readable, e.g. "5h 30m"
    @SerializedName("uptime_seconds") val uptimeSeconds: Long,
    @SerializedName("boot_time")      val bootTime: String  // "2026-03-08 06:30:00"
)

data class CpuInfo(
    @SerializedName("usage_percent")       val usagePercent: Float,      // 0.0 – 100.0
    @SerializedName("core_count_logical")  val coreCountLogical: Int,
    @SerializedName("core_count_physical") val coreCountPhysical: Int,
    @SerializedName("frequency_mhz")       val frequencyMhz: Long,
    @SerializedName("per_core_percent")    val perCorePercent: List<Float>  // one entry per logical core
)

data class MemoryInfo(
    @SerializedName("total_gb")      val totalGb: Double,
    @SerializedName("used_gb")       val usedGb: Double,
    @SerializedName("available_gb")  val availableGb: Double,
    @SerializedName("usage_percent") val usagePercent: Float,       // 0.0 – 100.0
    @SerializedName("swap_total_gb") val swapTotalGb: Double,
    @SerializedName("swap_used_gb")  val swapUsedGb: Double,
    @SerializedName("swap_percent")  val swapPercent: Float         // 0.0 – 100.0
)

data class GpuInfo(
    val index: Int,
    val name: String,
    val vendor: String,                                                // "NVIDIA", "AMD", or "Intel"
    @SerializedName("temperature_celsius")  val temperatureCelsius: Int?,
    @SerializedName("utilization_percent")  val utilizationPercent: Int?,
    @SerializedName("memory_total_mb")      val memoryTotalMb: Long,
    @SerializedName("memory_used_mb")       val memoryUsedMb: Long,
    @SerializedName("memory_usage_percent") val memoryUsagePercent: Float,  // 0.0 – 100.0
    @SerializedName("fan_speed_percent")    val fanSpeedPercent: Int?,
    @SerializedName("power_draw_watts")     val powerDrawWatts: Double?,
    @SerializedName("clock_speed_mhz")      val clockSpeedMhz: Int?
)

data class DiskInfo(
    val name: String,                // e.g. "sda1" or "C:" — can be empty on some Linux mounts
    val mountpoint: String,          // e.g. "/" or "C:\\"
    @SerializedName("file_system")   val fileSystem: String,  // e.g. "ext4", "NTFS", "btrfs"
    @SerializedName("total_gb")      val totalGb: Double,
    @SerializedName("used_gb")       val usedGb: Double,
    @SerializedName("free_gb")       val freeGb: Double,
    @SerializedName("usage_percent") val usagePercent: Float,
    @SerializedName("is_removable")  val isRemovable: Boolean
)

data class NetworkInfo(
    @SerializedName("upload_speed_mbps")   val uploadSpeedMbps: Double,   // MB/s (bytes, not bits)
    @SerializedName("download_speed_mbps") val downloadSpeedMbps: Double,
    @SerializedName("total_sent_gb")       val totalSentGb: Double,
    @SerializedName("total_recv_gb")       val totalRecvGb: Double,
    @SerializedName("packets_sent")        val packetsSent: Long,
    @SerializedName("packets_recv")        val packetsRecv: Long
)
```

### Sub-Endpoint Wrappers (optional — if you poll individual endpoints)

```kotlin
// data/model/SubResponses.kt

data class CpuResponse(val timestamp: String, val cpu: CpuInfo)
data class MemoryResponse(val timestamp: String, val memory: MemoryInfo)
data class GpuResponse(val timestamp: String, val gpu: List<GpuInfo>)
data class DiskResponse(val timestamp: String, val disk: List<DiskInfo>)
data class NetworkResponse(val timestamp: String, val network: NetworkInfo)
```

### Health & Root

```kotlin
data class HealthResponse(
    val status: String,      // "healthy"
    val timestamp: String,
    val version: String      // "1.0.0"
)

data class RootResponse(
    val status: String,      // "ok"
    val message: String,
    val version: String,
    val endpoints: List<String>
)
```

### Error (401 Unauthorized)

```kotlin
data class ErrorResponse(val error: String)
// Server returns: {"error": "missing or invalid API key"}
```

---

## 2. Network Layer

### Retrofit API Interface

```kotlin
// data/remote/MonitorApi.kt

import retrofit2.Response
import retrofit2.http.GET

interface MonitorApi {
    @GET("/")        suspend fun getRoot(): Response<RootResponse>
    @GET("/health")  suspend fun getHealth(): Response<HealthResponse>
    @GET("/metrics") suspend fun getMetrics(): Response<MetricsResponse>

    // Individual endpoints (use if you only need one section)
    @GET("/metrics/cpu")     suspend fun getCpu(): Response<CpuResponse>
    @GET("/metrics/memory")  suspend fun getMemory(): Response<MemoryResponse>
    @GET("/metrics/gpu")     suspend fun getGpu(): Response<GpuResponse>
    @GET("/metrics/disk")    suspend fun getDisk(): Response<DiskResponse>
    @GET("/metrics/network") suspend fun getNetwork(): Response<NetworkResponse>
}
```

### API Client Factory

Since your app supports **multiple servers** in a list, you need a client per server (each has a different URL + API key). Don't use a singleton — create one per server connection.

```kotlin
// data/remote/ApiClientFactory.kt

import okhttp3.Interceptor
import okhttp3.OkHttpClient
import okhttp3.logging.HttpLoggingInterceptor
import retrofit2.Retrofit
import retrofit2.converter.gson.GsonConverterFactory
import java.util.concurrent.TimeUnit

object ApiClientFactory {

    /**
     * Creates a MonitorApi for a specific server.
     *
     * @param baseUrl  Full URL, e.g. "http://192.168.1.100:8080"
     * @param apiKey   The MONITOR_API_KEY value configured on that server
     */
    fun create(baseUrl: String, apiKey: String): MonitorApi {
        val authInterceptor = Interceptor { chain ->
            val request = chain.request().newBuilder()
                .addHeader("X-API-Key", apiKey)
                .build()
            chain.proceed(request)
        }

        val logging = HttpLoggingInterceptor().apply {
            level = HttpLoggingInterceptor.Level.NONE  // set to BODY for debugging
        }

        val client = OkHttpClient.Builder()
            .addInterceptor(authInterceptor)
            .addInterceptor(logging)
            .connectTimeout(5, TimeUnit.SECONDS)
            .readTimeout(5, TimeUnit.SECONDS)
            .build()

        val normalizedUrl = if (baseUrl.endsWith("/")) baseUrl else "$baseUrl/"

        return Retrofit.Builder()
            .baseUrl(normalizedUrl)
            .client(client)
            .addConverterFactory(GsonConverterFactory.create())
            .build()
            .create(MonitorApi::class.java)
    }
}
```

---

## 3. Repository

```kotlin
// data/repository/MonitorRepository.kt

class MonitorRepository(private val api: MonitorApi) {

    suspend fun fetchMetrics(): Result<MetricsResponse> = runCatching {
        val response = api.getMetrics()
        when {
            response.isSuccessful -> response.body() ?: throw Exception("Empty response")
            response.code() == 401 -> throw AuthException("Invalid API key")
            else -> throw Exception("HTTP ${response.code()}: ${response.message()}")
        }
    }

    suspend fun fetchHealth(): Result<HealthResponse> = runCatching {
        val response = api.getHealth()
        if (response.isSuccessful) response.body() ?: throw Exception("Empty response")
        else throw Exception("HTTP ${response.code()}")
    }
}

class AuthException(message: String) : Exception(message)
```

---

## 4. ViewModel for Stats Screen

### UI State

```kotlin
// viewmodel/ServerStatsState.kt

sealed class ConnectionState {
    data object Disconnected : ConnectionState()
    data object Connecting : ConnectionState()
    data object Connected : ConnectionState()
    data class Error(val message: String) : ConnectionState()
}

data class ServerStatsState(
    val connectionState: ConnectionState = ConnectionState.Connecting,
    val metrics: MetricsResponse? = null,
    val lastUpdated: String = "",
    val isPolling: Boolean = false
)
```

### ViewModel

This ViewModel receives the server URL + API key when the user taps a server in the list. It starts polling immediately and stops when the screen is left.

```kotlin
// viewmodel/ServerStatsViewModel.kt

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

class ServerStatsViewModel : ViewModel() {

    private val _state = MutableStateFlow(ServerStatsState())
    val state = _state.asStateFlow()

    private var pollingJob: Job? = null
    private var repository: MonitorRepository? = null

    companion object {
        const val POLL_INTERVAL_MS = 2000L
    }

    /**
     * Call this when the stats screen opens with a specific server's details.
     */
    fun startMonitoring(serverUrl: String, apiKey: String) {
        if (pollingJob?.isActive == true) return  // already monitoring

        val api = ApiClientFactory.create(serverUrl, apiKey)
        repository = MonitorRepository(api)

        _state.update { it.copy(connectionState = ConnectionState.Connecting, isPolling = true) }

        pollingJob = viewModelScope.launch {
            while (isActive) {
                repository?.fetchMetrics()
                    ?.onSuccess { metrics ->
                        _state.update {
                            it.copy(
                                connectionState = ConnectionState.Connected,
                                metrics = metrics,
                                lastUpdated = metrics.timestamp
                            )
                        }
                    }
                    ?.onFailure { error ->
                        val msg = when (error) {
                            is AuthException -> "Invalid API key"
                            is java.net.ConnectException -> "Cannot reach server"
                            is java.net.SocketTimeoutException -> "Connection timed out"
                            else -> error.message ?: "Unknown error"
                        }
                        _state.update {
                            it.copy(connectionState = ConnectionState.Error(msg))
                        }
                    }

                delay(POLL_INTERVAL_MS)
            }
        }
    }

    fun stopMonitoring() {
        pollingJob?.cancel()
        pollingJob = null
        _state.update { it.copy(isPolling = false) }
    }

    override fun onCleared() {
        stopMonitoring()
        super.onCleared()
    }
}
```

---

## 5. Stats Screen — Full Layout

This is the main screen shown when the user taps a server from the list. It displays all metrics in a scrollable column with sections for system info, CPU, memory, disks, and network.

### Screen Structure

```
┌─────────────────────────────────────────────┐
│  ← Server Name          [status indicator]  │  TopAppBar
├─────────────────────────────────────────────┤
│                                             │
│  ┌─────────────────────────────────────┐    │
│  │  MY-PC                              │    │  System Info Card
│  │  Linux 6.19.6 (x86_64)              │    │
│  │  Uptime: 5h 30m                     │    │
│  │  Boot: 2026-03-08 06:30:00          │    │
│  └─────────────────────────────────────┘    │
│                                             │
│  ┌─────────────────────────────────────┐    │
│  │  CPU                        15.3%   │    │  CPU Card
│  │  ████████░░░░░░░░░░░░░░░░░░░░░░░░   │    │  (tappable → detail)
│  │  8C/16T @ 3600 MHz                  │    │
│  └─────────────────────────────────────┘    │
│                                             │
│  ┌─────────────────────────────────────┐    │
│  │  Memory                     53.1%   │    │  Memory Card
│  │  ████████████████░░░░░░░░░░░░░░░░   │    │  (tappable → detail)
│  │  8.42 / 15.87 GB                    │    │
│  │  ┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄     │    │
│  │  Swap: 0.5 / 4.0 GB (12.5%)         │    │
│  └─────────────────────────────────────┘    │
│                                             │
│  GPU                                        │
│  ┌─────────────────────────────────────┐    │
│  │  RTX 3080 (NVIDIA)          42%     │    │  GPU Card (one per GPU)
│  │  ████████████░░░░░░░░░░░░░░░░░░░    │    │  (tappable → detail)
│  │  VRAM: 3584 / 10240 MB  65°C        │    │
│  │  Fan: 55%  Power: 220.5W            │    │
│  └─────────────────────────────────────┘    │
│                                             │
│  Disks                                      │
│  ┌─────────────────────────────────────┐    │
│  │  sda1  (/)                  44.1%   │    │  Disk Card (one per disk)
│  │  █████████████░░░░░░░░░░░░░░░░░░░   │    │
│  │  210.12 / 476.34 GB  (ext4)         │    │
│  └─────────────────────────────────────┘    │
│                                             │
│  ┌─────────────────────────────────────┐    │
│  │  Network                            │    │  Network Card
│  │    ↑ 0.125 MB/s   ↓ 2.340 MB/s      │    │  (tappable → detail)
│  │  Sent: 1.234 GB   Recv: 5.678 GB    │    │
│  │  Pkts: 123,456 ↑  789,012 ↓         │    │
│  └─────────────────────────────────────┘    │
│                                             │
│  Last updated: 2s ago                       │
│                                             │
└─────────────────────────────────────────────┘
```

### Full Composable

```kotlin
// ui/screens/ServerStatsScreen.kt

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ServerStatsScreen(
    serverName: String,
    serverUrl: String,
    apiKey: String,
    onBack: () -> Unit,
    viewModel: ServerStatsViewModel = viewModel()
) {
    val state by viewModel.state.collectAsStateWithLifecycle()

    // Start polling when screen enters composition
    LaunchedEffect(serverUrl, apiKey) {
        viewModel.startMonitoring(serverUrl, apiKey)
    }

    // Stop polling when leaving screen
    DisposableEffect(Unit) {
        onDispose { viewModel.stopMonitoring() }
    }

    // Pause/resume on lifecycle
    val lifecycleOwner = LocalLifecycleOwner.current
    DisposableEffect(lifecycleOwner) {
        val observer = LifecycleEventObserver { _, event ->
            when (event) {
                Lifecycle.Event.ON_STOP -> viewModel.stopMonitoring()
                Lifecycle.Event.ON_START -> viewModel.startMonitoring(serverUrl, apiKey)
                else -> {}
            }
        }
        lifecycleOwner.lifecycle.addObserver(observer)
        onDispose { lifecycleOwner.lifecycle.removeObserver(observer) }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(serverName) },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                },
                actions = {
                    ConnectionStatusDot(state.connectionState)
                }
            )
        }
    ) { padding ->
        when {
            // Loading state — first connection attempt
            state.metrics == null && state.connectionState is ConnectionState.Connecting -> {
                Box(
                    Modifier.fillMaxSize().padding(padding),
                    contentAlignment = Alignment.Center
                ) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        CircularProgressIndicator()
                        Spacer(Modifier.height(16.dp))
                        Text("Connecting to $serverUrl...")
                    }
                }
            }

            // Error state with no data yet
            state.metrics == null && state.connectionState is ConnectionState.Error -> {
                Box(
                    Modifier.fillMaxSize().padding(padding),
                    contentAlignment = Alignment.Center
                ) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Icon(
                            Icons.Filled.Warning,
                            contentDescription = null,
                            tint = MaterialTheme.colorScheme.error,
                            modifier = Modifier.size(48.dp)
                        )
                        Spacer(Modifier.height(16.dp))
                        Text(
                            (state.connectionState as ConnectionState.Error).message,
                            style = MaterialTheme.typography.bodyLarge
                        )
                    }
                }
            }

            // Data available — show metrics
            state.metrics != null -> {
                StatsContent(
                    metrics = state.metrics!!,
                    connectionState = state.connectionState,
                    modifier = Modifier.padding(padding)
                )
            }
        }
    }
}

@Composable
private fun StatsContent(
    metrics: MetricsResponse,
    connectionState: ConnectionState,
    modifier: Modifier = Modifier
) {
    LazyColumn(
        modifier = modifier.fillMaxSize(),
        contentPadding = PaddingValues(16.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp)
    ) {
        // Connection error banner (shown when data is stale but we had data before)
        if (connectionState is ConnectionState.Error) {
            item {
                ConnectionErrorBanner(connectionState.message)
            }
        }

        // System info
        item { SystemInfoCard(metrics.system) }

        // CPU
        item { CpuOverviewCard(metrics.cpu) }

        // Memory
        item { MemoryOverviewCard(metrics.memory) }

        // GPUs (only show section if GPUs exist)
        if (metrics.gpu.isNotEmpty()) {
            item {
                Text("GPU", style = MaterialTheme.typography.titleMedium)
            }
            items(metrics.gpu, key = { it.index }) { gpu ->
                GpuOverviewCard(gpu)
            }
        }

        // Disks header
        item {
            Text("Disks", style = MaterialTheme.typography.titleMedium)
        }
        items(metrics.disk, key = { it.mountpoint }) { disk ->
            DiskCard(disk)
        }

        // Network
        item { NetworkOverviewCard(metrics.network) }

        // Timestamp footer
        item {
            Text(
                "Last updated: ${metrics.timestamp}",
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.fillMaxWidth(),
                textAlign = TextAlign.Center
            )
        }
    }
}
```

---

## 6. Reusable Components

### Connection Status Dot (TopAppBar action)

```kotlin
// ui/components/ConnectionStatusDot.kt

@Composable
fun ConnectionStatusDot(state: ConnectionState) {
    val color = when (state) {
        is ConnectionState.Connected -> Color(0xFF4CAF50)   // green
        is ConnectionState.Connecting -> Color(0xFFFFC107)  // amber
        is ConnectionState.Error -> Color(0xFFF44336)       // red
        is ConnectionState.Disconnected -> Color(0xFF9E9E9E) // grey
    }

    Box(
        modifier = Modifier
            .padding(end = 16.dp)
            .size(12.dp)
            .clip(CircleShape)
            .background(color)
    )
}
```

### Connection Error Banner

```kotlin
@Composable
fun ConnectionErrorBanner(message: String) {
    Surface(
        color = MaterialTheme.colorScheme.errorContainer,
        shape = MaterialTheme.shapes.small,
        modifier = Modifier.fillMaxWidth()
    ) {
        Row(
            Modifier.padding(12.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            Icon(
                Icons.Filled.Warning,
                contentDescription = null,
                tint = MaterialTheme.colorScheme.onErrorContainer,
                modifier = Modifier.size(20.dp)
            )
            Text(
                message,
                color = MaterialTheme.colorScheme.onErrorContainer,
                style = MaterialTheme.typography.bodySmall
            )
        }
    }
}
```

### System Info Card

```kotlin
// ui/components/SystemInfoCard.kt

@Composable
fun SystemInfoCard(system: SystemInfo) {
    ElevatedCard(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            Text(system.hostname, style = MaterialTheme.typography.headlineSmall)
            Spacer(Modifier.height(4.dp))
            InfoRow("OS", "${system.os} ${system.osVersion}")
            InfoRow("Kernel", system.kernelVersion)
            InfoRow("Arch", system.architecture)
            InfoRow("Uptime", system.uptime)
            InfoRow("Boot time", system.bootTime)
        }
    }
}

@Composable
private fun InfoRow(label: String, value: String) {
    Row(
        Modifier.fillMaxWidth().padding(vertical = 2.dp),
        horizontalArrangement = Arrangement.SpaceBetween
    ) {
        Text(label, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.bodySmall)
    }
}
```

### CPU Overview Card

```kotlin
// ui/components/CpuOverviewCard.kt

@Composable
fun CpuOverviewCard(cpu: CpuInfo) {
    ElevatedCard(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            // Header row
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text("CPU", style = MaterialTheme.typography.titleMedium)
                Text(
                    "${cpu.usagePercent}%",
                    style = MaterialTheme.typography.headlineSmall,
                    color = usageColor(cpu.usagePercent)
                )
            }

            Spacer(Modifier.height(8.dp))

            // Overall progress bar
            LinearProgressIndicator(
                progress = { cpu.usagePercent / 100f },
                modifier = Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(4.dp)),
                color = usageColor(cpu.usagePercent)
            )

            Spacer(Modifier.height(8.dp))

            // Subtitle
            Text(
                "${cpu.coreCountPhysical}C / ${cpu.coreCountLogical}T @ ${cpu.frequencyMhz} MHz",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant
            )

            Spacer(Modifier.height(12.dp))

            // Per-core mini bars (compact grid)
            Text("Per Core", style = MaterialTheme.typography.labelMedium)
            Spacer(Modifier.height(4.dp))

            // Show cores in a 2-column grid
            val chunked = cpu.perCorePercent.chunked(2)
            chunked.forEachIndexed { rowIdx, pair ->
                Row(
                    Modifier.fillMaxWidth().padding(vertical = 2.dp),
                    horizontalArrangement = Arrangement.spacedBy(12.dp)
                ) {
                    pair.forEachIndexed { colIdx, usage ->
                        val coreIdx = rowIdx * 2 + colIdx
                        Row(
                            Modifier.weight(1f),
                            verticalAlignment = Alignment.CenterVertically
                        ) {
                            Text(
                                "$coreIdx",
                                modifier = Modifier.width(24.dp),
                                style = MaterialTheme.typography.labelSmall
                            )
                            LinearProgressIndicator(
                                progress = { usage / 100f },
                                modifier = Modifier.weight(1f).height(6.dp).clip(RoundedCornerShape(3.dp)),
                                color = usageColor(usage)
                            )
                            Text(
                                "${usage.toInt()}%",
                                modifier = Modifier.width(36.dp).padding(start = 4.dp),
                                style = MaterialTheme.typography.labelSmall,
                                textAlign = TextAlign.End
                            )
                        }
                    }
                    // If odd number of cores, fill the empty space
                    if (pair.size == 1) {
                        Spacer(Modifier.weight(1f))
                    }
                }
            }
        }
    }
}
```

### Memory Overview Card

```kotlin
// ui/components/MemoryOverviewCard.kt

@Composable
fun MemoryOverviewCard(memory: MemoryInfo) {
    ElevatedCard(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            // Header
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Text("Memory", style = MaterialTheme.typography.titleMedium)
                Text(
                    "${memory.usagePercent}%",
                    style = MaterialTheme.typography.headlineSmall,
                    color = usageColor(memory.usagePercent)
                )
            }

            Spacer(Modifier.height(8.dp))

            // RAM progress bar
            LinearProgressIndicator(
                progress = { memory.usagePercent / 100f },
                modifier = Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(4.dp)),
                color = usageColor(memory.usagePercent)
            )

            Spacer(Modifier.height(4.dp))

            Text(
                "${memory.usedGb} GB used / ${memory.totalGb} GB total (${memory.availableGb} GB available)",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant
            )

            // Swap section (only show if swap exists)
            if (memory.swapTotalGb > 0) {
                HorizontalDivider(Modifier.padding(vertical = 8.dp))

                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween
                ) {
                    Text("Swap", style = MaterialTheme.typography.labelMedium)
                    Text("${memory.swapPercent}%", style = MaterialTheme.typography.labelMedium)
                }

                Spacer(Modifier.height(4.dp))

                LinearProgressIndicator(
                    progress = { memory.swapPercent / 100f },
                    modifier = Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)),
                    color = usageColor(memory.swapPercent)
                )

                Spacer(Modifier.height(2.dp))

                Text(
                    "${memory.swapUsedGb} GB / ${memory.swapTotalGb} GB",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant
                )
            }
        }
    }
}
```

### GPU Overview Card

```kotlin
// ui/components/GpuOverviewCard.kt

@Composable
fun GpuOverviewCard(gpu: GpuInfo) {
    ElevatedCard(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            // Header
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Column(Modifier.weight(1f)) {
                    Text(gpu.name, style = MaterialTheme.typography.titleSmall)
                    Text(gpu.vendor, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                gpu.utilizationPercent?.let { util ->
                    Text(
                        "${util}%",
                        style = MaterialTheme.typography.headlineSmall,
                        color = usageColor(util.toFloat())
                    )
                }
            }

            // Utilization bar
            gpu.utilizationPercent?.let { util ->
                Spacer(Modifier.height(8.dp))
                LinearProgressIndicator(
                    progress = { util / 100f },
                    modifier = Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(4.dp)),
                    color = usageColor(util.toFloat())
                )
            }

            Spacer(Modifier.height(8.dp))

            // VRAM bar
            if (gpu.memoryTotalMb > 0) {
                Row(
                    Modifier.fillMaxWidth(),
                    horizontalArrangement = Arrangement.SpaceBetween
                ) {
                    Text("VRAM", style = MaterialTheme.typography.labelMedium)
                    Text("${gpu.memoryUsagePercent}%", style = MaterialTheme.typography.labelMedium)
                }
                Spacer(Modifier.height(4.dp))
                LinearProgressIndicator(
                    progress = { gpu.memoryUsagePercent / 100f },
                    modifier = Modifier.fillMaxWidth().height(4.dp).clip(RoundedCornerShape(2.dp)),
                    color = usageColor(gpu.memoryUsagePercent)
                )
                Spacer(Modifier.height(2.dp))
                Text(
                    "${gpu.memoryUsedMb} MB / ${gpu.memoryTotalMb} MB",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant
                )
            }

            HorizontalDivider(Modifier.padding(vertical = 8.dp))

            // Stats row: Temperature, Fan, Power, Clock
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceEvenly
            ) {
                gpu.temperatureCelsius?.let { temp ->
                    GpuStatItem("Temp", "${temp}\u00B0C")
                }
                gpu.fanSpeedPercent?.let { fan ->
                    GpuStatItem("Fan", "${fan}%")
                }
                gpu.powerDrawWatts?.let { power ->
                    GpuStatItem("Power", "${power}W")
                }
                gpu.clockSpeedMhz?.let { clock ->
                    GpuStatItem("Clock", "${clock} MHz")
                }
            }
        }
    }
}

@Composable
private fun GpuStatItem(label: String, value: String) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Text(label, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(value, style = MaterialTheme.typography.bodyMedium)
    }
}
```

### Disk Card

```kotlin
// ui/components/DiskCard.kt

@Composable
fun DiskCard(disk: DiskInfo) {
    ElevatedCard(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically
            ) {
                Column(Modifier.weight(1f)) {
                    Text(
                        disk.name.ifEmpty { disk.mountpoint },
                        style = MaterialTheme.typography.titleSmall
                    )
                    Text(
                        buildString {
                            append(disk.fileSystem)
                            if (disk.name.isNotEmpty()) append(" — ${disk.mountpoint}")
                            if (disk.isRemovable) append(" (removable)")
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant
                    )
                }
                Text(
                    "${disk.usagePercent}%",
                    style = MaterialTheme.typography.titleMedium,
                    color = usageColor(disk.usagePercent)
                )
            }

            Spacer(Modifier.height(8.dp))

            LinearProgressIndicator(
                progress = { disk.usagePercent / 100f },
                modifier = Modifier.fillMaxWidth().height(6.dp).clip(RoundedCornerShape(3.dp)),
                color = usageColor(disk.usagePercent)
            )

            Spacer(Modifier.height(4.dp))

            Text(
                "${disk.usedGb} GB used / ${disk.totalGb} GB total (${disk.freeGb} GB free)",
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant
            )
        }
    }
}
```

### Network Overview Card

```kotlin
// ui/components/NetworkOverviewCard.kt

@Composable
fun NetworkOverviewCard(network: NetworkInfo) {
    ElevatedCard(modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp)) {
            Text("Network", style = MaterialTheme.typography.titleMedium)

            Spacer(Modifier.height(12.dp))

            // Speed row
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceEvenly
            ) {
                SpeedColumn(
                    label = "Upload",
                    speedMbps = network.uploadSpeedMbps,
                    arrow = "\u2191"  // ↑
                )
                SpeedColumn(
                    label = "Download",
                    speedMbps = network.downloadSpeedMbps,
                    arrow = "\u2193"  // ↓
                )
            }

            HorizontalDivider(Modifier.padding(vertical = 12.dp))

            // Totals row
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween
            ) {
                Column {
                    Text("Total Sent", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text("${network.totalSentGb} GB", style = MaterialTheme.typography.bodyMedium)
                }
                Column(horizontalAlignment = Alignment.End) {
                    Text("Total Received", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Text("${network.totalRecvGb} GB", style = MaterialTheme.typography.bodyMedium)
                }
            }

            Spacer(Modifier.height(8.dp))

            // Packet counts
            Row(
                Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween
            ) {
                Text(
                    "Packets: ${formatNumber(network.packetsSent)} sent",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant
                )
                Text(
                    "${formatNumber(network.packetsRecv)} received",
                    style = MaterialTheme.typography.labelSmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant
                )
            }
        }
    }
}

@Composable
private fun SpeedColumn(label: String, speedMbps: Double, arrow: String) {
    Column(horizontalAlignment = Alignment.CenterHorizontally) {
        Text(label, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.height(4.dp))
        Text(
            "$arrow ${formatSpeed(speedMbps)}",
            style = MaterialTheme.typography.titleLarge
        )
    }
}
```

### Shared Utility Functions

```kotlin
// ui/components/Utils.kt

@Composable
fun usageColor(percent: Float): Color = when {
    percent > 90f -> MaterialTheme.colorScheme.error
    percent > 70f -> MaterialTheme.colorScheme.tertiary
    else -> MaterialTheme.colorScheme.primary
}

fun formatSpeed(mbps: Double): String = when {
    mbps < 0.001 -> "0 B/s"
    mbps < 1.0   -> String.format("%.0f KB/s", mbps * 1024)
    else         -> String.format("%.2f MB/s", mbps)
}

fun formatNumber(n: Long): String {
    return java.text.NumberFormat.getIntegerInstance().format(n)
}
```

---

## 7. Section Detail Screens

Optional — if you want tapping a card to show a full-screen detail view.

### CPU Detail (full per-core list)

```kotlin
// ui/screens/CpuDetailScreen.kt

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun CpuDetailScreen(cpu: CpuInfo, onBack: () -> Unit) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("CPU Details") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                }
            )
        }
    ) { padding ->
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(padding),
            contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp)
        ) {
            // Overview
            item {
                ElevatedCard(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(16.dp)) {
                        Text("Overall Usage", style = MaterialTheme.typography.titleMedium)
                        Text("${cpu.usagePercent}%", style = MaterialTheme.typography.displaySmall, color = usageColor(cpu.usagePercent))
                        Spacer(Modifier.height(8.dp))
                        LinearProgressIndicator(
                            progress = { cpu.usagePercent / 100f },
                            modifier = Modifier.fillMaxWidth().height(12.dp).clip(RoundedCornerShape(6.dp)),
                            color = usageColor(cpu.usagePercent)
                        )
                        Spacer(Modifier.height(8.dp))
                        InfoRow("Physical Cores", "${cpu.coreCountPhysical}")
                        InfoRow("Logical Cores", "${cpu.coreCountLogical}")
                        InfoRow("Frequency", "${cpu.frequencyMhz} MHz")
                    }
                }
            }

            // Per-core header
            item {
                Text("Per-Core Usage", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 8.dp))
            }

            // Individual core rows
            itemsIndexed(cpu.perCorePercent) { index, usage ->
                ElevatedCard(Modifier.fillMaxWidth()) {
                    Row(
                        Modifier.padding(horizontal = 16.dp, vertical = 12.dp).fillMaxWidth(),
                        verticalAlignment = Alignment.CenterVertically
                    ) {
                        Text(
                            "Core $index",
                            modifier = Modifier.width(64.dp),
                            style = MaterialTheme.typography.bodyMedium
                        )
                        LinearProgressIndicator(
                            progress = { usage / 100f },
                            modifier = Modifier.weight(1f).height(10.dp).clip(RoundedCornerShape(5.dp)),
                            color = usageColor(usage)
                        )
                        Text(
                            "${usage}%",
                            modifier = Modifier.width(56.dp).padding(start = 8.dp),
                            style = MaterialTheme.typography.bodyMedium,
                            textAlign = TextAlign.End,
                            color = usageColor(usage)
                        )
                    }
                }
            }
        }
    }
}
```

### Memory Detail

```kotlin
// ui/screens/MemoryDetailScreen.kt

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MemoryDetailScreen(memory: MemoryInfo, onBack: () -> Unit) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Memory Details") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                }
            )
        }
    ) { padding ->
        Column(
            Modifier.fillMaxSize().padding(padding).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp)
        ) {
            // RAM card
            ElevatedCard(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp)) {
                    Text("RAM", style = MaterialTheme.typography.titleMedium)
                    Spacer(Modifier.height(8.dp))
                    Text("${memory.usagePercent}%", style = MaterialTheme.typography.displaySmall, color = usageColor(memory.usagePercent))
                    Spacer(Modifier.height(8.dp))
                    LinearProgressIndicator(
                        progress = { memory.usagePercent / 100f },
                        modifier = Modifier.fillMaxWidth().height(12.dp).clip(RoundedCornerShape(6.dp)),
                        color = usageColor(memory.usagePercent)
                    )
                    Spacer(Modifier.height(12.dp))
                    InfoRow("Total", "${memory.totalGb} GB")
                    InfoRow("Used", "${memory.usedGb} GB")
                    InfoRow("Available", "${memory.availableGb} GB")
                }
            }

            // Swap card
            ElevatedCard(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp)) {
                    Text("Swap", style = MaterialTheme.typography.titleMedium)
                    Spacer(Modifier.height(8.dp))

                    if (memory.swapTotalGb > 0) {
                        Text("${memory.swapPercent}%", style = MaterialTheme.typography.displaySmall, color = usageColor(memory.swapPercent))
                        Spacer(Modifier.height(8.dp))
                        LinearProgressIndicator(
                            progress = { memory.swapPercent / 100f },
                            modifier = Modifier.fillMaxWidth().height(12.dp).clip(RoundedCornerShape(6.dp)),
                            color = usageColor(memory.swapPercent)
                        )
                        Spacer(Modifier.height(12.dp))
                        InfoRow("Total", "${memory.swapTotalGb} GB")
                        InfoRow("Used", "${memory.swapUsedGb} GB")
                    } else {
                        Text(
                            "No swap configured",
                            style = MaterialTheme.typography.bodyMedium,
                            color = MaterialTheme.colorScheme.onSurfaceVariant
                        )
                    }
                }
            }
        }
    }
}
```

### GPU Detail

```kotlin
// ui/screens/GpuDetailScreen.kt

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun GpuDetailScreen(gpus: List<GpuInfo>, onBack: () -> Unit) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("GPU Details") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                }
            )
        }
    ) { padding ->
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(padding),
            contentPadding = PaddingValues(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp)
        ) {
            items(gpus, key = { it.index }) { gpu ->
                ElevatedCard(Modifier.fillMaxWidth()) {
                    Column(Modifier.padding(16.dp)) {
                        // GPU name + vendor
                        Text(gpu.name, style = MaterialTheme.typography.titleMedium)
                        Text("${gpu.vendor} — GPU #${gpu.index}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)

                        Spacer(Modifier.height(12.dp))

                        // Utilization
                        gpu.utilizationPercent?.let { util ->
                            Text("Utilization", style = MaterialTheme.typography.labelMedium)
                            Text("${util}%", style = MaterialTheme.typography.displaySmall, color = usageColor(util.toFloat()))
                            Spacer(Modifier.height(4.dp))
                            LinearProgressIndicator(
                                progress = { util / 100f },
                                modifier = Modifier.fillMaxWidth().height(12.dp).clip(RoundedCornerShape(6.dp)),
                                color = usageColor(util.toFloat())
                            )
                            Spacer(Modifier.height(12.dp))
                        }

                        // VRAM
                        if (gpu.memoryTotalMb > 0) {
                            Text("VRAM", style = MaterialTheme.typography.labelMedium)
                            Text("${gpu.memoryUsagePercent}%", style = MaterialTheme.typography.headlineSmall, color = usageColor(gpu.memoryUsagePercent))
                            Spacer(Modifier.height(4.dp))
                            LinearProgressIndicator(
                                progress = { gpu.memoryUsagePercent / 100f },
                                modifier = Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(4.dp)),
                                color = usageColor(gpu.memoryUsagePercent)
                            )
                            Spacer(Modifier.height(4.dp))
                            InfoRow("Total", "${gpu.memoryTotalMb} MB")
                            InfoRow("Used", "${gpu.memoryUsedMb} MB")
                            InfoRow("Free", "${gpu.memoryTotalMb - gpu.memoryUsedMb} MB")
                            Spacer(Modifier.height(12.dp))
                        }

                        // Stats grid
                        HorizontalDivider(Modifier.padding(vertical = 4.dp))
                        gpu.temperatureCelsius?.let { InfoRow("Temperature", "${it}\u00B0C") }
                        gpu.fanSpeedPercent?.let { InfoRow("Fan Speed", "${it}%") }
                        gpu.powerDrawWatts?.let { InfoRow("Power Draw", "${it} W") }
                        gpu.clockSpeedMhz?.let { InfoRow("Clock Speed", "${it} MHz") }
                    }
                }
            }
        }
    }
}
```

### Network Detail

```kotlin
// ui/screens/NetworkDetailScreen.kt

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NetworkDetailScreen(network: NetworkInfo, onBack: () -> Unit) {
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Network Details") },
                navigationIcon = {
                    IconButton(onClick = onBack) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = "Back")
                    }
                }
            )
        }
    ) { padding ->
        Column(
            Modifier.fillMaxSize().padding(padding).padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp)
        ) {
            // Speed card
            ElevatedCard(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp)) {
                    Text("Current Speed", style = MaterialTheme.typography.titleMedium)
                    Spacer(Modifier.height(16.dp))
                    Row(
                        Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.SpaceEvenly
                    ) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            Text("Upload", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            Text("\u2191 ${formatSpeed(network.uploadSpeedMbps)}", style = MaterialTheme.typography.headlineMedium)
                        }
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            Text("Download", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            Text("\u2193 ${formatSpeed(network.downloadSpeedMbps)}", style = MaterialTheme.typography.headlineMedium)
                        }
                    }
                }
            }

            // Totals card
            ElevatedCard(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp)) {
                    Text("Total Data Transfer", style = MaterialTheme.typography.titleMedium)
                    Spacer(Modifier.height(12.dp))
                    InfoRow("Sent", "${network.totalSentGb} GB")
                    InfoRow("Received", "${network.totalRecvGb} GB")
                }
            }

            // Packets card
            ElevatedCard(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(16.dp)) {
                    Text("Packets", style = MaterialTheme.typography.titleMedium)
                    Spacer(Modifier.height(12.dp))
                    InfoRow("Sent", formatNumber(network.packetsSent))
                    InfoRow("Received", formatNumber(network.packetsRecv))
                }
            }
        }
    }
}
```

---

## 8. Polling Mechanism

### Summary

| Aspect          | Value                                                                           |
| --------------- | ------------------------------------------------------------------------------- |
| **Endpoint**    | `GET /metrics` (full snapshot, ~1 KB payload)                                   |
| **Interval**    | 2000ms — matches server design                                                  |
| **Lifecycle**   | Pause on `ON_STOP`, resume on `ON_START`                                        |
| **Timeout**     | 5s connect + 5s read — fail fast                                                |
| **On error**    | Keep polling — next attempt in 2s is auto-retry. Show stale data + error banner |
| **Auth header** | `X-API-Key: <value>` on every request                                           |

### Why poll `/metrics` and not individual endpoints?

The full `/metrics` response is small (~1 KB) and returns everything in one request. Making 4 separate calls (`/cpu`, `/memory`, `/disk`, `/network`) would be 4x the overhead for negligible savings. Use individual endpoints only if you add a view that monitors just one metric.

### Network speed accuracy

The server calculates upload/download speed as `(bytes_now - bytes_previous) / elapsed_seconds`. With 2s polling, the speed is an average over the last 2 seconds. The **first** response after server start shows speed since boot — it stabilizes on the 2nd poll.

---

## 9. Error Handling

| Scenario                   | HTTP                                           | What user sees                            |
| -------------------------- | ---------------------------------------------- | ----------------------------------------- |
| Server unreachable         | Connection refused / timeout                   | Error screen: "Cannot reach server"       |
| Wrong API key              | `401` `{"error":"missing or invalid API key"}` | Error screen: "Invalid API key"           |
| Server stopped mid-session | Connection reset                               | Error banner over stale data + auto-retry |
| WiFi disconnected          | No route to host                               | Error banner: "No network connection"     |
| First poll CPU = 0%        | Normal (sysinfo warm-up)                       | Shows 0% briefly, corrects on next poll   |

The ViewModel already maps exception types to user-friendly messages (see section 4). The stats screen shows:

- **Full error screen** if no data has been received yet
- **Error banner + stale data** if connection drops after data was loaded (so the user can still see the last known state)

---

## 10. Wiring It Up — Navigation

In your existing navigation, when the user taps a server item in the list, navigate to the stats screen passing the server's details:

```kotlin
// In your NavHost or navigation setup

composable(
    route = "server_stats/{serverName}/{serverUrl}/{apiKey}",
    arguments = listOf(
        navArgument("serverName") { type = NavType.StringType },
        navArgument("serverUrl") { type = NavType.StringType },
        navArgument("apiKey") { type = NavType.StringType }
    )
) { backStackEntry ->
    ServerStatsScreen(
        serverName = backStackEntry.arguments?.getString("serverName") ?: "",
        serverUrl = backStackEntry.arguments?.getString("serverUrl") ?: "",
        apiKey = backStackEntry.arguments?.getString("apiKey") ?: "",
        onBack = { navController.popBackStack() }
    )
}

// When user taps a server item in the list:
val encodedUrl = URLEncoder.encode(server.url, "UTF-8")
val encodedKey = URLEncoder.encode(server.apiKey, "UTF-8")
navController.navigate("server_stats/${server.name}/$encodedUrl/$encodedKey")
```

**Alternative:** If you're using a shared ViewModel or passing the server object differently (e.g. via `savedStateHandle`, a shared data holder, or type-safe navigation), adapt accordingly. The key point is that `ServerStatsScreen` needs `serverUrl` and `apiKey` to start polling.

---

## 11. Server Behavior Notes

Things to be aware of when building the UI:

| Behavior                                           | Impact on Android UI                                                                                                                                                                            |
| -------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **CPU first-read is 0%**                           | sysinfo needs a warm-up sample. The server handles this on startup, but the very first poll _after server restart_ may still show slightly off values. Corrects itself on the next 2s poll.     |
| **Network speed = delta between polls**            | Speed is `(current_bytes - previous_bytes) / elapsed_time`. First request after server start reflects cumulative speed since boot. Normalizes on 2nd poll.                                      |
| **`disk.name` can be empty**                       | On Linux, some mount points (like tmpfs) have empty names. Use `mountpoint` as fallback display text: `disk.name.ifEmpty { disk.mountpoint }`                                                   |
| **`uptime` is human-readable**                     | The server sends both `"uptime": "5h 30m"` and `"uptime_seconds": 19800`. Use the string for display, seconds for calculations.                                                                 |
| **Speed units are MB/s** (megabytes, not megabits) | The field is named `_mbps` but the value is computed as `bytes / 1_048_576` — these are **megabytes per second**, not megabits. Display as "MB/s".                                              |
| **CORS is fully open**                             | The server allows all origins/methods/headers. No CORS issues from Android.                                                                                                                     |
| **All disks are returned**                         | Including virtual filesystems on Linux (tmpfs, devtmpfs, etc.). You may want to filter out disks with `total_gb == 0` or specific filesystem types.                                             |
| **`gpu` is always an array**                       | Empty `[]` if no GPU detected (e.g., headless server, integrated-only). Multiple entries for multi-GPU. Always check `gpu.isNotEmpty()` before showing the GPU section.                         |
| **GPU fields are nullable**                        | `temperature_celsius`, `utilization_percent`, `fan_speed_percent`, `power_draw_watts`, `clock_speed_mhz` can be `null` if the GPU/driver doesn't expose that metric. Use `?.let {}` in Compose. |
| **NVIDIA vs AMD vs Intel GPU support**             | NVIDIA GPUs use NVML (full metrics). AMD/Intel GPUs use Linux sysfs (fewer fields). `vendor` is `"NVIDIA"`, `"AMD"`, or `"Intel"`. Intel iGPUs typically only report clock speed.               |
| **GPU VRAM is in MB, not GB**                      | Unlike RAM/disk which use GB, GPU memory uses `memory_total_mb` / `memory_used_mb` in **megabytes**. Intel iGPUs share system RAM so VRAM fields will be 0.                                     |

---

## 12. Full JSON Schema Reference

### `GET /metrics` — Full Snapshot (this is what you poll)

```json
{
  "timestamp": "2026-03-08T12:00:00.000+05:30",
  "system": {
    "hostname": "MY-PC",
    "os": "Linux",
    "os_version": "6.19.6",
    "kernel_version": "6.19.6-arch1-1",
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
    "per_core_percent": [12.1, 18.5, 5.0, 22.3, 8.7, 15.2, 3.1, 45.0, 10.0, 20.1, 6.5, 14.3, 9.8, 11.2, 7.4, 25.6]
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
      "name": "sda1",
      "mountpoint": "/",
      "file_system": "ext4",
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

### `GET /health` — Validation endpoint (use in Add Server dialog)

```json
{
  "status": "healthy",
  "timestamp": "2026-03-08T12:00:00.000+05:30",
  "version": "1.0.0"
}
```

### `401 Unauthorized` — Wrong/missing API key

```json
{ "error": "missing or invalid API key" }
```

### Type Mapping: Server → Android

| Rust Type   | JSON          | Kotlin    |
| ----------- | ------------- | --------- |
| `String`    | `string`      | `String`  |
| `f32`       | `number`      | `Float`   |
| `f64`       | `number`      | `Double`  |
| `u64`       | `number`      | `Long`    |
| `u32`       | `number`      | `Int`     |
| `usize`     | `number`      | `Int`     |
| `bool`      | `boolean`     | `Boolean` |
| `Option<T>` | `T` or `null` | `T?`      |
| `Vec<T>`    | `array`       | `List<T>` |
