use anyhow::{Context, Result};
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;

pub fn run() -> Result<()> {
    let addr = "127.0.0.1:8787";
    let listener =
        TcpListener::bind(addr).with_context(|| format!("failed to bind web UI on {addr}"))?;
    eprintln!("EasyWiFi Web UI listening on http://{addr}");

    // Best-effort browser launch.
    let _ = std::process::Command::new("xdg-open")
        .arg(format!("http://{addr}"))
        .spawn();

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                thread::spawn(move || {
                    let _ = handle_client(stream);
                });
            }
            Err(err) => {
                eprintln!("web ui accept error: {err}");
            }
        }
    }
    Ok(())
}

fn handle_client(mut stream: TcpStream) -> Result<()> {
    let mut buf = [0_u8; 8192];
    let read = stream.read(&mut buf).context("failed reading request")?;
    if read == 0 {
        return Ok(());
    }
    let req = String::from_utf8_lossy(&buf[..read]);
    let mut lines = req.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET");
    let path = parts.next().unwrap_or("/");

    if method == "GET" && path == "/api/health" {
        return respond_json(&mut stream, "{\"status\":\"ok\",\"ui\":\"web\"}");
    }

    if method != "GET" {
        return respond_status(
            &mut stream,
            405,
            "Method Not Allowed",
            "text/plain",
            b"method not allowed",
        );
    }

    let file_path = map_path(path);
    if !file_path.exists() {
        return respond_status(&mut stream, 404, "Not Found", "text/plain", b"not found");
    }
    let bytes =
        fs::read(&file_path).with_context(|| format!("failed reading {}", file_path.display()))?;
    let mime = mime_for(&file_path);
    respond_status(&mut stream, 200, "OK", mime, &bytes)
}

fn map_path(request_path: &str) -> PathBuf {
    let base = Path::new("lovableUI");
    match request_path {
        "/" => base.join("index.html"),
        "/index.css" => base.join("index.css"),
        "/app.js" => base.join("app.js"),
        "/favicon.ico" => base.join("favicon.ico"),
        "/placeholder.svg" => base.join("placeholder.svg"),
        _ => base.join("index.html"),
    }
}

fn mime_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "application/javascript; charset=utf-8",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn respond_json(stream: &mut TcpStream, body: &str) -> Result<()> {
    respond_status(
        stream,
        200,
        "OK",
        "application/json; charset=utf-8",
        body.as_bytes(),
    )
}

fn respond_status(
    stream: &mut TcpStream,
    code: u16,
    reason: &str,
    content_type: &str,
    body: &[u8],
) -> Result<()> {
    let header = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(header.as_bytes())
        .context("failed writing header")?;
    stream.write_all(body).context("failed writing body")?;
    Ok(())
}
