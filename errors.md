warning: variants `KillResult`, `Pong`, and `Error` are never constructed
--> src/main.rs:156:5
|
154 | enum ServerMessage {
| ------------- variants in this enum
155 | Metrics { data: MetricsResponse },
156 | KillResult { pid: u32, success: bool, error: Option<String> },
| ^^^^^^^^^^
157 | Pong,
| ^^^^
158 | Error { message: String },
| ^^^^^
|
= note: `#[warn(dead_code)]` (part of `#[warn(unused)]`) on by default

warning: `system-monitor-server` (bin "monitor") generated 1 warning
