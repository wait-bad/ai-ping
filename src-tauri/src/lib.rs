use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures_util::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

// ---------- 数据模型 ----------

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// 全局测速超时时间（秒），默认 30
    pub timeout_secs: u64,
    /// 失败重试次数，默认 1（即失败后可重试 1 次，0 为不重试）
    pub retry_count: u32,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            timeout_secs: 30,
            retry_count: 1,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub id: String,
    pub name: String,
    /// API 基础地址，例如 https://api.openai.com/v1
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PingResult {
    pub model: String,
    pub ok: bool,
    /// 成功请求发出 → 收到响应头 (ms)
    pub header_ms: Option<u64>,
    /// 成功请求发出 → 第一个可见输出字符 (ms)（与中转站后台记录的单次请求 TTFT 一致）
    pub first_char_ms: Option<u64>,
    /// 成功请求发出 → 流结束 (ms)
    pub total_ms: Option<u64>,
    /// 包含所有重试与等待在内的客户端总消耗时间 (ms)
    pub overall_ms: Option<u64>,
    /// 实际尝试次数（1 为一次成功，>1 表示经历过重试）
    pub attempts: u32,
    /// 首个可见输出片段
    pub sample: String,
    pub error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NetworkTimeInfo {
    /// 权威网络时间戳 (毫秒)
    pub network_timestamp: u64,
    /// 测得网络时间时的本地时间戳 (毫秒)
    pub local_timestamp: u64,
    /// 时间偏差 (毫秒): network_timestamp - local_timestamp (正数表示本地慢了，负数表示本地快了)
    pub offset_ms: i64,
    /// 获取网络时间的单程往返 RTT (毫秒)
    pub rtt_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LocalPingInfo {
    pub ok: bool,
    /// 本地到公共核心 DNS/网关的延迟 (ms)
    pub latency_ms: Option<u64>,
    pub target: String,
    pub error: Option<String>,
}

#[derive(Default)]
struct HttpState {
    client: std::sync::OnceLock<reqwest::Client>,
}

impl HttpState {
    fn client(&self) -> &reqwest::Client {
        self.client.get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(15))
                .tcp_nodelay(true)
                .user_agent("ai-ping/0.1")
                .build()
                .expect("reqwest client")
        })
    }
}

type EndpointsState = Mutex<Vec<Endpoint>>;
type SettingsState = Mutex<AppSettings>;

// ---------- 配置持久化 ----------

fn config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

fn endpoints_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("endpoints.json"))
}

fn settings_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(config_dir(app)?.join("settings.json"))
}

fn load_endpoints(app: &AppHandle) -> Vec<Endpoint> {
    endpoints_path(app)
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_endpoints(app: &AppHandle, list: &[Endpoint]) -> Result<(), String> {
    let path = endpoints_path(app)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(
        &path,
        serde_json::to_string_pretty(list).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

fn load_settings(app: &AppHandle) -> AppSettings {
    settings_path(app)
        .ok()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_settings(app: &AppHandle, settings: &AppSettings) -> Result<(), String> {
    let path = settings_path(app)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(
        &path,
        serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

// ---------- URL 拼接（对完整地址和 base 地址都宽容） ----------

fn chat_url(base: &str) -> String {
    let t = base.trim().trim_end_matches('/');
    if t.ends_with("/chat/completions") {
        t.to_string()
    } else if t.ends_with("/v1") || t.contains("/v1/") {
        format!("{t}/chat/completions")
    } else {
        format!("{t}/v1/chat/completions")
    }
}

fn clip_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn json_str_field(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

/// 非标准 provider 兜底：从原始 data 行里提取内容
fn loose_content(data: &str) -> Option<String> {
    let idx = data.find("\"content\"")
        .or_else(|| data.find("\"reasoning_content\""))
        .or_else(|| data.find("\"thought\""))
        .or_else(|| data.find("\"arguments\""))?;
    let after = &data[idx..];
    let colon = after.find(':')?;
    let rest = after[colon + 1..].trim_start();
    if !rest.starts_with('"') {
        return None;
    }
    let mut out = String::new();
    let mut chars = rest[1..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => {
                if let Some(e) = chars.next() {
                    out.push(match e {
                        'n' => '\n',
                        't' => '\t',
                        'r' => '\r',
                        other => other,
                    });
                }
            }
            other => out.push(other),
        }
        if out.chars().count() >= 40 {
            break;
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// 从 delta 或 message 对象中提取首个非空内容（涵盖 content、reasoning_content、thought、tool_calls 等各种模型形态）
fn extract_delta_content(delta: &serde_json::Value) -> Option<String> {
    if let Some(s) = json_str_field(delta, "content") {
        if !s.is_empty() {
            return Some(s);
        }
    }
    if let Some(s) = json_str_field(delta, "reasoning_content") {
        if !s.is_empty() {
            return Some(s);
        }
    }
    if let Some(s) = json_str_field(delta, "reasoning") {
        if !s.is_empty() {
            return Some(s);
        }
    }
    if let Some(s) = json_str_field(delta, "thought") {
        if !s.is_empty() {
            return Some(s);
        }
    }
    if let Some(tool_calls) = delta.get("tool_calls").and_then(|v| v.as_array()) {
        if let Some(first_tc) = tool_calls.first() {
            if let Some(func) = first_tc.get("function") {
                if let Some(name) = json_str_field(func, "name") {
                    if !name.is_empty() {
                        return Some(format!("[fn:{name}]"));
                    }
                }
                if let Some(args) = json_str_field(func, "arguments") {
                    if !args.is_empty() {
                        return Some(args);
                    }
                }
            }
        }
    }
    None
}

/// 解析单行 SSE data 内容
fn extract_chunk_text(data: &str) -> Option<String> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
        if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
            if let Some(first_choice) = choices.first() {
                if let Some(delta) = first_choice.get("delta") {
                    if let Some(t) = extract_delta_content(delta) {
                        return Some(t);
                    }
                }
                if let Some(msg) = first_choice.get("message") {
                    if let Some(t) = extract_delta_content(msg) {
                        return Some(t);
                    }
                }
                if let Some(text) = json_str_field(first_choice, "text") {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
    }
    loose_content(data)
}

// ---------- 单次首字计时 ----------

async fn ping_attempt(
    client: &reqwest::Client,
    ep: &Endpoint,
    model: &str,
    timeout_secs: u64,
) -> PingResult {
    let mut result = PingResult {
        model: model.to_string(),
        ok: false,
        header_ms: None,
        first_char_ms: None,
        total_ms: None,
        overall_ms: None,
        attempts: 1,
        sample: String::new(),
        error: None,
    };

    let body = serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": "Hi" }],
        "stream": true,
        "max_tokens": 16,
    });

    let timeout_duration = Duration::from_secs(if timeout_secs == 0 { 30 } else { timeout_secs });

    // 从单次请求发出开始精准计时（涵盖该次请求的 DNS、TCP/TLS 握手及响应 TTFT）
    let start = Instant::now();

    let send_req = client
        .post(chat_url(&ep.base_url))
        .header("Content-Type", "application/json")
        .header("Accept", "text/event-stream")
        .bearer_auth(ep.api_key.trim())
        .json(&body)
        .send();

    let resp = match tokio::time::timeout(timeout_duration, send_req).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            result.error = Some(format!("连接失败：{e}"));
            return result;
        }
        Err(_) => {
            result.error = Some(format!("请求连接超时（>{timeout_secs}s）"));
            return result;
        }
    };

    result.header_ms = Some(start.elapsed().as_millis() as u64);
    let status = resp.status();

    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        result.error = Some(format!("HTTP {status}·{}", clip_chars(text.trim(), 260)));
        return result;
    }

    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut line_start = 0usize;
    let mut first_char_ms: Option<u64> = None;
    let mut sample = String::new();
    let mut stream_err: Option<String> = None;

    let read_stream = async {
        while let Some(chunk) = stream.next().await {
            let now = start.elapsed().as_millis() as u64;
            match chunk {
                Ok(b) => buf.extend_from_slice(&b),
                Err(e) => {
                    return Err(format!("流中断：{e}"));
                }
            }
            // 按行扫描 SSE（CRLF 兼容：trim 掉 \r）
            while let Some(nl) = buf[line_start..].iter().position(|&c| c == b'\n') {
                let line =
                    String::from_utf8_lossy(&buf[line_start..line_start + nl]).trim().to_string();
                line_start += nl + 1;
                let Some(data) = line.strip_prefix("data:") else {
                    continue;
                };
                let data = data.trim();
                if data.is_empty() {
                    continue;
                }
                if data == "[DONE]" {
                    return Ok(());
                }

                if let Ok(v) = serde_json::from_str::<serde_json::Value>(data) {
                    if let Some(err_obj) = v.get("error") {
                        let err_msg = json_str_field(err_obj, "message")
                            .unwrap_or_else(|| data.to_string());
                        return Err(format!("接口错误·{}", clip_chars(&err_msg, 260)));
                    }
                }

                if let Some(t) = extract_chunk_text(data) {
                    if !t.is_empty() {
                        first_char_ms = Some(now);
                        sample = clip_chars(&t, 40);
                        return Ok(());
                    }
                }
            }
        }
        Ok(())
    };

    let remaining_timeout = timeout_duration.saturating_sub(start.elapsed());
    match tokio::time::timeout(remaining_timeout, read_stream).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            stream_err = Some(e);
        }
        Err(_) => {
            stream_err = Some(format!("等待首字超时（>{timeout_secs}s）"));
        }
    }

    result.total_ms = Some(start.elapsed().as_millis() as u64);
    match first_char_ms {
        Some(ms) => {
            result.ok = true;
            result.first_char_ms = Some(ms);
            result.sample = sample;
        }
        None => {
            result.error = Some(stream_err.unwrap_or_else(|| "未收到可见输出".into()));
        }
    }
    result
}

// ---------- 核心：带重试与全生命周期追踪的首字计时 ----------

pub async fn ping_one(
    client: &reqwest::Client,
    ep: &Endpoint,
    model: &str,
    timeout_secs: u64,
    retry_count: u32,
) -> PingResult {
    let overall_start = Instant::now();
    let mut last_res = ping_attempt(client, ep, model, timeout_secs).await;
    last_res.attempts = 1;
    last_res.overall_ms = Some(overall_start.elapsed().as_millis() as u64);

    if last_res.ok || retry_count == 0 {
        return last_res;
    }

    let mut attempt = 0;
    while attempt < retry_count && !last_res.ok {
        attempt += 1;
        // 短暂退避 200ms
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut res = ping_attempt(client, ep, model, timeout_secs).await;
        res.attempts = attempt + 1;
        res.overall_ms = Some(overall_start.elapsed().as_millis() as u64);
        if res.ok {
            return res;
        }
        let prev_err = last_res.error.clone().unwrap_or_default();
        let cur_err = res.error.clone().unwrap_or_default();
        last_res = res;
        last_res.error = Some(format!("重试{attempt}次仍失败：{cur_err} (初次: {prev_err})"));
    }
    last_res.overall_ms = Some(overall_start.elapsed().as_millis() as u64);
    last_res
}

// ---------- 网络时间与本地时间探测 ----------

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[tauri::command]
async fn get_network_time(app: AppHandle) -> Result<NetworkTimeInfo, String> {
    let client = app.state::<HttpState>().client().clone();
    let t0 = Instant::now();
    let local_t0 = now_millis();

    let resp = client
        .head("https://www.cloudflare.com/cdn-cgi/trace")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("获取网络时间失败: {e}"))?;

    let rtt = t0.elapsed().as_millis() as u64;
    let local_mid = local_t0 + rtt / 2;

    if let Some(date_val) = resp.headers().get("date") {
        if let Ok(date_str) = date_val.to_str() {
            if let Ok(parsed) = httpdate::parse_http_date(date_str) {
                let net_ms = parsed
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(local_mid);
                let offset = (net_ms as i64) - (local_mid as i64);
                return Ok(NetworkTimeInfo {
                    network_timestamp: net_ms,
                    local_timestamp: local_mid,
                    offset_ms: offset,
                    rtt_ms: rtt,
                });
            }
        }
    }

    Ok(NetworkTimeInfo {
        network_timestamp: local_mid,
        local_timestamp: local_mid,
        offset_ms: 0,
        rtt_ms: rtt,
    })
}

// ---------- 本地网络延迟测试 ----------

#[tauri::command]
async fn test_local_network_ping(app: AppHandle) -> LocalPingInfo {
    let client = app.state::<HttpState>().client().clone();
    let target = "https://1.1.1.1/cdn-cgi/trace";
    let start = Instant::now();
    let res = client
        .get(target)
        .timeout(Duration::from_secs(4))
        .send()
        .await;

    match res {
        Ok(r) => {
            let lat = start.elapsed().as_millis() as u64;
            LocalPingInfo {
                ok: r.status().is_success(),
                latency_ms: Some(lat),
                target: "1.1.1.1 (Cloudflare)".into(),
                error: None,
            }
        }
        Err(e) => LocalPingInfo {
            ok: false,
            latency_ms: None,
            target: "1.1.1.1 (Cloudflare)".into(),
            error: Some(format!("测速失败: {e}")),
        },
    }
}

// ---------- Tauri 命令 ----------

#[tauri::command]
fn list_endpoints(state: State<'_, EndpointsState>) -> Vec<Endpoint> {
    state.lock().clone()
}

#[tauri::command]
fn save_endpoints_cmd(
    app: AppHandle,
    state: State<'_, EndpointsState>,
    list: Vec<Endpoint>,
) -> Result<(), String> {
    save_endpoints(&app, &list)?;
    *state.lock() = list;
    Ok(())
}

#[tauri::command]
fn get_settings(state: State<'_, SettingsState>) -> AppSettings {
    state.lock().clone()
}

#[tauri::command]
fn save_settings_cmd(
    app: AppHandle,
    state: State<'_, SettingsState>,
    settings: AppSettings,
) -> Result<(), String> {
    save_settings(&app, &settings)?;
    *state.lock() = settings;
    Ok(())
}

/// 并发测一个接口下的一批模型，每完成一个推送 `ping://result`
#[tauri::command]
async fn ping_models(
    app: AppHandle,
    endpoint: Endpoint,
    models: Vec<String>,
) -> Result<Vec<PingResult>, String> {
    let client = app.state::<HttpState>().client().clone();
    let settings = app.state::<SettingsState>().lock().clone();
    let timeout_secs = settings.timeout_secs;
    let retry_count = settings.retry_count;

    let mut set = tokio::task::JoinSet::new();
    for m in models {
        let c = client.clone();
        let e = endpoint.clone();
        let a = app.clone();
        let t = timeout_secs;
        let r_cnt = retry_count;
        set.spawn(async move {
            let r = ping_one(&c, &e, &m, t, r_cnt).await;
            let _ = a.emit("ping://result", &r);
            r
        });
    }
    let mut out = Vec::new();
    while let Some(r) = set.join_next().await {
        if let Ok(r) = r {
            out.push(r);
        }
    }
    Ok(out)
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            app.manage(HttpState::default());
            app.manage(Mutex::new(load_endpoints(&handle)));
            app.manage(Mutex::new(load_settings(&handle)));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_endpoints,
            save_endpoints_cmd,
            get_settings,
            save_settings_cmd,
            ping_models,
            get_network_time,
            test_local_network_ping
        ])
        .run(tauri::generate_context!())
        .expect("error while running AI Ping");
}
