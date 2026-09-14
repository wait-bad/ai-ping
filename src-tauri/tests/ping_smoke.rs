// 冒烟测试：本地起假 SSE 服务器，验证真实解析 + 计时链路
use std::io::{Read, Write};
use std::net::TcpListener;

use ai_ping_lib::{ping_one, Endpoint};

fn spawn_sse_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            // 读完请求头（到 \r\n\r\n）即可，不解析内容
            let mut buf = [0u8; 4096];
            let mut got = Vec::new();
            loop {
                match s.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        got.extend_from_slice(&buf[..n]);
                        if got.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let body = [
                "data: {\"id\":1,\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
                "data: {\"id\":2,\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"}}]}\n\n",
                "data: [DONE]\n\n",
            ].join("");
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n{body}"
            );
            let _ = s.write_all(resp.as_bytes());
            let _ = s.flush();
            let _ = s.shutdown(std::net::Shutdown::Write);
            let mut sink = Vec::new();
            let _ = s.read_to_end(&mut sink);
        }
    });
    port
}

fn spawn_fail_then_success_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let cur = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if cur == 0 {
                // 第一次返回 500
                let _ = s.write_all(b"HTTP/1.1 500 Internal Error\r\nContent-Length: 5\r\nConnection: close\r\n\r\nerror");
            } else {
                // 第二次返回 SSE
                let body = "data: {\"choices\":[{\"delta\":{\"content\":\"retry_ok\"}}]}\n\ndata: [DONE]\n\n";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n{body}"
                );
                let _ = s.write_all(resp.as_bytes());
            }
            let _ = s.flush();
        }
    });
    port
}

#[tokio::test]
async fn first_char_timing_over_real_sse() {
    let port = spawn_sse_server();
    let ep = Endpoint {
        id: "t".into(),
        name: "local".into(),
        base_url: format!("http://127.0.0.1:{port}/v1"),
        api_key: "sk-test".into(),
        models: vec!["m1".into()],
    };
    let client = reqwest::Client::new();
    let r = ping_one(&client, &ep, "m1", 30, 1).await;
    assert!(r.first_char_ms.is_some() && r.first_char_ms.unwrap() <= r.total_ms.unwrap());
    assert_eq!(r.sample, "ok");
    assert_eq!(r.attempts, 1);
    assert!(r.overall_ms.is_some());
}

#[tokio::test]
async fn retry_success_captures_attempts_and_overall_ms() {
    let port = spawn_fail_then_success_server();
    let ep = Endpoint {
        id: "retry_ep".into(),
        name: "retry_local".into(),
        base_url: format!("http://127.0.0.1:{port}/v1"),
        api_key: "sk-test".into(),
        models: vec!["m_retry".into()],
    };
    let client = reqwest::Client::new();
    let r = ping_one(&client, &ep, "m_retry", 30, 2).await;
    assert!(r.ok);
    assert_eq!(r.attempts, 2);
    assert_eq!(r.sample, "retry_ok");
    assert!(r.overall_ms.unwrap() >= 200); // 经历了退避
}

#[tokio::test]
async fn http_error_is_reported_with_retry() {
    // 404 的服务器：任何路径都回 404
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf);
            let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot-found");
            let _ = s.flush();
        }
    });
    let ep = Endpoint {
        id: "t".into(),
        name: "local".into(),
        base_url: format!("http://127.0.0.1:{port}"),
        api_key: String::new(),
        models: vec!["m".into()],
    };
    let r = ping_one(&reqwest::Client::new(), &ep, "m", 30, 2).await;
    assert!(!r.ok);
    assert_eq!(r.attempts, 3); // 初始1次 + 重试2次
    let err = r.error.unwrap();
    assert!(err.contains("404"));
    assert!(err.contains("重试2次仍失败"));
}
