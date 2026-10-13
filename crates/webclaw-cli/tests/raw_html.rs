use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;

fn proxy(status: &'static str, body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for socket in listener.incoming() {
            let Ok(mut socket) = socket else { return };
            let mut buffer = [0; 4096];
            let _ = socket.read(&mut buffer);
            let reply = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = socket.write_all(reply.as_bytes());
        }
    });
    format!("http://{addr}")
}

fn raw_html(status: &'static str, body: &'static str) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_webclaw"))
        .args([
            "http://example.com/page",
            "--raw-html",
            "--render",
            "never",
            "-p",
        ])
        .arg(proxy(status, body))
        .env_remove("WEBCLAW_API_KEY")
        .output()
        .unwrap()
}

#[test]
#[ignore = "requires local TCP sockets and public DNS"]
fn an_error_page_prints_its_body_and_exits_1() {
    let out = raw_html("404 Not Found", "<p>gone</p>");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stdout).contains("<p>gone</p>"));
    assert!(String::from_utf8_lossy(&out.stderr).contains("HTTP 404"));
}

#[test]
#[ignore = "requires local TCP sockets and public DNS"]
fn a_successful_page_exits_0() {
    let out = raw_html("200 OK", "<p>here</p>");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&out.stdout).contains("<p>here</p>"));
}
