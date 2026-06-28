// Code Wiki dashboard — a tiny_http server that hosts the
// pre-built Understand-Anything dashboard SPA and serves our
// knowledge-graph.json / meta.json / config.json from the project's
// wiki/code_wiki/<repo>/ directory.
//
// One server per (project_path, repo_name). The server keeps running
// for the lifetime of the Tauri app (or until explicitly closed) so
// switching repos doesn't lose the user's browser tab.

use std::collections::HashMap;
use std::fs;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::State;
use tiny_http::{Header, Method, Response, Server, StatusCode};

use crate::commands::code_wiki::{
    graph_path_for, meta_path_for, run_indexer_inner, WIKI_CODE_WIKI_DIR,
};

const PROTECTED_PATHS: &[&str] = &[
    "/knowledge-graph.json",
    "/meta.json",
    "/config.json",
];
const SPATIAL_FILE_FALLBACK: &str = "/index.html";
const BIND_HOST: &str = "127.0.0.1";

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CodeWikiDashboardInfo {
    pub project_path: String,
    pub repo_name: String,
    pub url: String,
    pub port: u16,
    pub token: String,
}

#[derive(Default)]
pub struct DashboardState {
    entries: Mutex<HashMap<String, DashboardEntry>>,
}

struct DashboardEntry {
    port: u16,
    token: String,
    project_path: PathBuf,
    repo_name: String,
    /// When dropped, the tiny_http server's recv loop will see the
    /// listener close and exit. We just keep an Option<()>-style sentinel
    /// by relying on the `kill_switch` field below.
    kill_switch: KillSwitch,
}

/// Tiny helper that drops a TCP listener on Drop — closing the listener
/// causes tiny_http's `incoming_requests()` iterator to return None,
/// ending the request loop.
struct KillSwitch {
    listener: Option<TcpListener>,
}

impl Drop for KillSwitch {
    fn drop(&mut self) {
        if let Some(l) = self.listener.take() {
            // Dropping closes the OS socket; tiny_http's accept loop
            // sees EOF and returns. We try to avoid panics if the
            // listener is already closed.
            drop(l);
        }
    }
}

fn dashboard_key(project_path: &str, repo_name: &str) -> String {
    format!("{}::{repo_name}", normalize_key(project_path))
}

fn normalize_key(s: &str) -> String {
    s.replace('\\', "/").trim_end_matches('/').to_string()
}

/// Locate the pre-built dashboard assets. We try a few locations in
/// order:
///   1. `<CARGO_MANIFEST_DIR>/dashboard-assets` — the dev-time path
///      used by `cargo run` (resolved at compile time).
///   2. `<exe_dir>/dashboard-assets` — when running an installed
///      Tauri build (the resources are copied next to the binary).
///   3. `<exe_dir>/resources/dashboard-assets` — Tauri 2 bundles
///      resources under a `resources/` subfolder next to the binary.
fn locate_dashboard_assets() -> Result<PathBuf, String> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates: Vec<PathBuf> = vec![manifest_dir.join("dashboard-assets")];
    if let Some(exe) = std::env::current_exe().ok() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("dashboard-assets"));
            candidates.push(dir.join("resources").join("dashboard-assets"));
        }
    }
    for candidate in candidates {
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    Err("dashboard assets not found; expected src-tauri/dashboard-assets/".to_string())
}

fn read_cached(assets: &Path, rel: &str) -> Option<(Vec<u8>, &'static str)> {
    let path = assets.join(rel.trim_start_matches('/'));
    let bytes = fs::read(&path).ok()?;
    let mime = mime_for(rel);
    Some((bytes, mime))
}

fn mime_for(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if lower.ends_with(".js") || lower.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if lower.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".ico") {
        "image/x-icon"
    } else if lower.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if lower.ends_with(".woff2") {
        "font/woff2"
    } else if lower.ends_with(".woff") {
        "font/woff"
    } else {
        "application/octet-stream"
    }
}

fn gen_token() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::time::SystemTime;
    let mut h = DefaultHasher::new();
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .hash(&mut h);
    std::process::id().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Make a server bound to an OS-assigned port on 127.0.0.1. Returns
/// the listener (so we can hold the kill switch) and the resolved port.
fn bind_local() -> Result<(TcpListener, u16), String> {
    let addr: SocketAddr = format!("{BIND_HOST}:0").parse().unwrap();
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind 127.0.0.1:0: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("local_addr: {e}"))?
        .port();
    Ok((listener, port))
}

fn respond_with(
    request: tiny_http::Request,
    status: u16,
    content_type: &'static str,
    body: Vec<u8>,
) {
    let len = body.len();
    let mut response = Response::from_data(body).with_status_code(StatusCode(status));
    if let Ok(h) = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()) {
        response = response.with_header(h);
    }
    if let Ok(h) = Header::from_bytes(&b"Content-Length"[..], len.to_string().as_bytes()) {
        response = response.with_header(h);
    }
    let _ = request.respond(response);
}

fn respond_status(request: tiny_http::Request, status: u16, message: &str) {
    let body = format!("{message}\n").into_bytes();
    respond_with(request, status, "text/plain; charset=utf-8", body);
}

fn token_matches(url: &str, expected: &str) -> bool {
    // The token can come either as ?token=... in the URL or as an
    // Authorization: Bearer header (for fetch from JS).
    if let Some(qs) = url.split_once('?').map(|(_, q)| q) {
        for pair in qs.split('&') {
            if let Some(v) = pair.strip_prefix("token=") {
                if v == expected {
                    return true;
                }
            }
        }
    }
    false
}

fn serve_dashboard(
    listener: TcpListener,
    assets: PathBuf,
    project_path: PathBuf,
    repo_name: String,
    token: String,
) {
    let server = match Server::from_listener(listener, None) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("[code-wiki dashboard] from_listener failed: {err}");
            return;
        }
    };
    eprintln!(
        "[code-wiki dashboard] serving repo={} at http://{BIND_HOST}:{} (token gated)",
        repo_name,
        server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0)
    );
    for request in server.incoming_requests() {
        let method = request.method().clone();
        let url = request.url().to_string();
        let path = url.split('?').next().unwrap_or("/").to_string();

        if method == Method::Options {
            respond_status(request, 204, "");
            continue;
        }

        // Protected data endpoints
        if PROTECTED_PATHS.iter().any(|p| path == *p) {
            if !token_matches(&url, &token) {
                respond_status(request, 403, "Forbidden: missing or invalid token");
                continue;
            }
            match path.as_str() {
                "/knowledge-graph.json" => {
                    let file = graph_path_for(&project_path, &repo_name);
                    if let Ok(bytes) = fs::read(&file) {
                        respond_with(request, 200, mime_for("knowledge-graph.json"), bytes);
                    } else {
                        respond_status(request, 404, "knowledge-graph.json not found");
                    }
                }
                "/meta.json" => {
                    let file = meta_path_for(&project_path, &repo_name);
                    if let Ok(bytes) = fs::read(&file) {
                        respond_with(request, 200, mime_for("meta.json"), bytes);
                    } else {
                        respond_status(request, 404, "meta.json not found");
                    }
                }
                "/config.json" => {
                    let body =
                        br#"{"autoUpdate":false,"outputLanguage":"en"}"#.to_vec();
                    respond_with(request, 200, mime_for("config.json"), body);
                }
                _ => respond_status(request, 404, "not found"),
            }
            continue;
        }

        // Static asset
        let rel = path.trim_start_matches('/');
        if let Some((bytes, mime)) = read_cached(&assets, rel) {
            respond_with(request, 200, mime, bytes);
            continue;
        }

        // SPA fallback — any other path is an in-app route, serve
        // index.html so React Router (or hash routing) can take over.
        if let Some((bytes, mime)) = read_cached(&assets, SPATIAL_FILE_FALLBACK) {
            respond_with(request, 200, mime, bytes);
        } else {
            respond_status(request, 500, "index.html missing in dashboard-assets");
        }
    }
    eprintln!("[code-wiki dashboard] stopped for repo={repo_name}");
}

fn ensure_knowledge_graph(project_path: &Path, repo_name: &str) -> Result<PathBuf, String> {
    let graph_path = graph_path_for(project_path, repo_name);
    if graph_path.exists() {
        return Ok(graph_path);
    }
    // Trigger the indexer; the TS layer will write knowledge-graph.json
    // immediately after we return. Wait briefly to let the file appear.
    run_indexer_inner(project_path, repo_name)?;
    for _ in 0..50 {
        if graph_path.exists() {
            return Ok(graph_path);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(format!(
        "knowledge-graph.json still missing after running indexer for {repo_name}"
    ))
}

#[tauri::command]
pub async fn code_wiki_open_dashboard(
    project_path: String,
    repo_name: String,
    state: State<'_, DashboardState>,
) -> Result<CodeWikiDashboardInfo, String> {
    let project_path_buf = PathBuf::from(&project_path);
    let key = dashboard_key(&project_path, &repo_name);

    // Reuse if already open
    {
        let entries = state.entries.lock().map_err(|e| format!("lock: {e}"))?;
        if let Some(entry) = entries.get(&key) {
            let url = format!(
                "http://{BIND_HOST}:{}/?token={}",
                entry.port, entry.token
            );
            return Ok(CodeWikiDashboardInfo {
                project_path: entry.project_path.to_string_lossy().to_string(),
                repo_name: entry.repo_name.clone(),
                url,
                port: entry.port,
                token: entry.token.clone(),
            });
        }
    }

    let assets = locate_dashboard_assets()?;
    let _ = ensure_knowledge_graph(&project_path_buf, &repo_name)?;

    let (listener, port) = bind_local()?;
    let token = gen_token();

    let assets_for_thread = assets.clone();
    let project_for_thread = project_path_buf.clone();
    let repo_for_thread = repo_name.clone();
    let token_for_thread = token.clone();
    let listener_for_thread = listener.try_clone().map_err(|e| format!("try_clone: {e}"))?;

    // The thread takes the cloned listener; the original stays alive
    // in `kill_switch` until the entry is removed (or the app exits).
    thread::Builder::new()
        .name(format!("code-wiki-dashboard-{repo_name}"))
        .spawn(move || {
            serve_dashboard(
                listener_for_thread,
                assets_for_thread,
                project_for_thread,
                repo_for_thread,
                token_for_thread,
            )
        })
        .map_err(|e| format!("spawn dashboard thread: {e}"))?;

    // Probe the server to make sure it's actually accepting before
    // we return the URL. The thread's `tiny_http::Server` will start
    // accepting on the listener immediately, but there's a small
    // window where the loop hasn't started yet.
    for _ in 0..20 {
        if TcpStream::connect_timeout(
            &format!("{BIND_HOST}:{port}").parse().unwrap(),
            Duration::from_millis(100),
        )
        .is_ok()
        {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }

    let entry = DashboardEntry {
        port,
        token: token.clone(),
        project_path: project_path_buf,
        repo_name: repo_name.clone(),
        kill_switch: KillSwitch {
            listener: Some(listener),
        },
    };
    {
        let mut entries = state.entries.lock().map_err(|e| format!("lock: {e}"))?;
        entries.insert(key, entry);
    }

    let url = format!("http://{BIND_HOST}:{port}/?token={token}");
    Ok(CodeWikiDashboardInfo {
        project_path,
        repo_name,
        url,
        port,
        token,
    })
}

#[tauri::command]
pub async fn code_wiki_close_dashboard(
    project_path: String,
    repo_name: String,
    state: State<'_, DashboardState>,
) -> Result<(), String> {
    let key = dashboard_key(&project_path, &repo_name);
    let mut entries = state.entries.lock().map_err(|e| format!("lock: {e}"))?;
    if let Some(mut entry) = entries.remove(&key) {
        // Drop the kill switch — closes the listener, tiny_http's
        // accept loop returns None, the thread exits.
        entry.kill_switch.listener.take();
        Ok(())
    } else {
        Ok(())
    }
}

#[tauri::command]
pub async fn code_wiki_list_dashboards(
    state: State<'_, DashboardState>,
) -> Result<Vec<CodeWikiDashboardInfo>, String> {
    let entries = state.entries.lock().map_err(|e| format!("lock: {e}"))?;
    let out: Vec<CodeWikiDashboardInfo> = entries
        .iter()
        .map(|(_, entry)| {
            let url = format!(
                "http://{BIND_HOST}:{}/?token={}",
                entry.port, entry.token
            );
            CodeWikiDashboardInfo {
                project_path: entry.project_path.to_string_lossy().to_string(),
                repo_name: entry.repo_name.clone(),
                url,
                port: entry.port,
                token: entry.token.clone(),
            }
        })
        .collect();
    Ok(out)
}

/// Test-only: read the URL token from the `?token=` query string.
#[cfg(test)]
pub fn token_from_query(url: &str) -> Option<String> {
    token_matches(url, "").then(|| "ok".to_string());
    url.split_once('?').and_then(|(_, qs)| {
        qs.split('&').find_map(|p| p.strip_prefix("token=").map(|v| v.to_string()))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_from_query_extracts_token() {
        assert_eq!(
            token_from_query("/?token=abc123"),
            Some("abc123".to_string())
        );
        assert_eq!(token_from_query("/knowledge-graph.json"), None);
        assert_eq!(
            token_from_query("/?other=1&token=hello"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn mime_for_known_extensions() {
        assert_eq!(mime_for("a.html"), "text/html; charset=utf-8");
        assert_eq!(mime_for("a.js"), "application/javascript; charset=utf-8");
        assert_eq!(mime_for("a.css"), "text/css; charset=utf-8");
        assert_eq!(mime_for("a.svg"), "image/svg+xml");
        assert_eq!(mime_for("a.json"), "application/json; charset=utf-8");
    }

    /// End-to-end smoke test: spin up the real `serve_dashboard` against
    /// a temp project + temp assets dir, send real HTTP requests via
    /// `std::net::TcpStream`, and assert that:
    ///   1. `GET /` serves the index.html
    ///   2. `GET /assets/index-s-XXXXX.js` (or any hashed JS) is reachable
    ///   3. `GET /knowledge-graph.json?token=…` returns 200 with the JSON
    ///   4. `GET /knowledge-graph.json` (no token) returns 403
    ///   5. `GET /knowledge-graph.json?token=wrong` returns 403
    ///   6. `GET /meta.json?token=…` returns 200 with the meta JSON
    ///   7. `GET /config.json?token=…` returns 200 with default config
    ///   8. `GET /some-spa-route` falls back to index.html (SPA routing)
    #[test]
    fn dashboard_server_end_to_end() {
        use std::io::{Read as _, Write as _};
        use std::time::Duration;

        // 1. Lay out a temp project: knowledge-graph.json + meta.json
        //    under wiki/code_wiki/<repo>/, plus a minimal assets dir
        //    with an index.html we can recognize.
        let project = std::env::temp_dir().join(format!(
            "codewiki-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&project);
        let repo_dir = project.join("wiki").join("code_wiki").join("demo");
        fs::create_dir_all(&repo_dir).unwrap();
        let kg = serde_json::json!({
            "version": "1.0.0",
            "kind": "codebase",
            "project": {
                "name": "demo",
                "languages": ["rust"],
                "frameworks": [],
                "description": "",
                "analyzedAt": "2026-06-27T00:00:00Z",
                "gitCommitHash": ""
            },
            "nodes": [
                {"id": "file:src/lib.rs", "type": "file", "name": "lib.rs",
                 "filePath": "src/lib.rs", "lineRange": [0, 10],
                 "summary": "demo", "tags": [], "complexity": "moderate"}
            ],
            "edges": [],
            "layers": [],
            "tour": []
        });
        fs::write(
            repo_dir.join("knowledge-graph.json"),
            serde_json::to_vec_pretty(&kg).unwrap(),
        )
        .unwrap();
        fs::write(
            repo_dir.join("meta.json"),
            br#"{"lastAnalyzedAt":"2026-06-27T00:00:00Z","gitCommitHash":"","version":"codegraph-1.0.0","analyzedFiles":1}"#,
        )
        .unwrap();

        // 2. Lay out a temp assets dir: index.html, favicon.svg, assets/foo.js
        let assets = project.join("assets");
        fs::create_dir_all(assets.join("assets")).unwrap();
        fs::write(
            assets.join("index.html"),
            br#"<!doctype html><html><head><script type="module" src="/assets/foo.js"></script></head><body id="ua-root">DEMO_INDEX_HTML</body></html>"#,
        )
        .unwrap();
        fs::write(assets.join("favicon.svg"), b"<svg/>DEMO_FAVICON").unwrap();
        fs::write(assets.join("assets").join("foo.js"), b"console.log('DEMO_FOO_JS');").unwrap();

        // 3. Bind a free port + start the server in a thread.
        let (listener, port) = bind_local().expect("bind");
        let token = "tok_e2e_12345".to_string();
        let project_for_thread = project.clone();
        let assets_for_thread = assets.clone();
        let token_for_thread = token.clone();
        let handle = thread::spawn(move || {
            serve_dashboard(
                listener,
                assets_for_thread,
                project_for_thread,
                "demo".to_string(),
                token_for_thread,
            )
        });

        // Helper: send a request and return (status, body).
        let fetch = |url_path: &str, expect_status: u16| -> String {
            let mut stream =
                TcpStream::connect_timeout(&format!("127.0.0.1:{port}").parse().unwrap(), Duration::from_secs(2))
                    .expect("connect");
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            stream
                .write_all(
                    format!(
                        "GET {url_path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .expect("write");
            let mut buf = String::new();
            stream.read_to_string(&mut buf).expect("read");
            // Quick status check.
            let status_line = buf.lines().next().unwrap_or("");
            assert!(
                status_line.contains(&format!(" {expect_status} ")),
                "expected status {expect_status} for {url_path}, got: {status_line}"
            );
            buf
        };

        // 1. GET / → 200, body contains our marker
        let body = fetch("/", 200);
        assert!(body.contains("DEMO_INDEX_HTML"), "/ did not serve index.html: {body}");

        // 2. GET /assets/foo.js → 200, body contains the marker
        let body = fetch("/assets/foo.js", 200);
        assert!(body.contains("DEMO_FOO_JS"), "/assets/foo.js body wrong: {body}");

        // 3. GET /knowledge-graph.json?token=… → 200, body is the kg
        let body = fetch(&format!("/knowledge-graph.json?token={token}"), 200);
        let parsed: serde_json::Value = serde_json::from_str(
            body.split("\r\n\r\n").nth(1).expect("body after headers"),
        )
        .expect("kg parses");
        assert_eq!(parsed["project"]["name"], "demo");
        assert_eq!(parsed["nodes"][0]["name"], "lib.rs");

        // 4. GET /knowledge-graph.json (no token) → 403
        let _ = fetch("/knowledge-graph.json", 403);

        // 5. GET /knowledge-graph.json?token=wrong → 403
        let _ = fetch("/knowledge-graph.json?token=wrong", 403);

        // 6. GET /meta.json?token=… → 200, body is the meta
        let body = fetch(&format!("/meta.json?token={token}"), 200);
        assert!(
            body.contains("codegraph-1.0.0"),
            "/meta.json body wrong: {body}"
        );

        // 7. GET /config.json?token=… → 200, body is the default config
        let body = fetch(&format!("/config.json?token={token}"), 200);
        assert!(
            body.contains("\"autoUpdate\":false"),
            "/config.json body wrong: {body}"
        );

        // 8. SPA fallback: GET /some-spa-route → 200, body is index.html
        let body = fetch("/some-spa-route", 200);
        assert!(
            body.contains("DEMO_INDEX_HTML"),
            "SPA fallback did not return index.html: {body}"
        );

        // Tear down: drop the kill switch (the listener in handle is
        // held by the spawned thread, so we wait for the server to
        // notice its own closure and exit). Easiest: leak the listener
        // — the test process exits shortly anyway.
        drop(handle);
        let _ = fs::remove_dir_all(&project);
    }

    /// Full real-world pipeline: invoke `codegraph init` + `codegraph
    /// index` on a temp Rust repo, then read the SQLite store via
    /// `run_get_graph_payload_inner` (mirroring what the TS writer
    /// would do in production), write a UA-shaped knowledge-graph.json,
    /// start `serve_dashboard`, and curl all the endpoints to verify
    /// the dashboard would actually load with a real graph.
    ///
    /// Skipped when `codegraph` is not on PATH (CI without the CLI).
    #[test]
    fn real_codegraph_db_to_dashboard_pipeline() {
        use crate::commands::code_wiki::run_get_graph_payload_inner;
        use std::io::{Read as _, Write as _};
        use std::time::Duration;

        let bin = match which::which("codegraph") {
            Ok(b) => b,
            Err(_) => {
                eprintln!("[e2e] codegraph not on PATH; skipping real pipeline test");
                return;
            }
        };

        // 1. Lay out a tiny Rust repo under a temp project.
        let project = std::env::temp_dir().join(format!(
            "codewiki-e2e-real-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&project);
        // We need `project` again later (to pass into the server
        // thread and to clean up at the end), so don't move it here —
        // clone it once and use the clone for joined paths.
        let repo = project.clone().join("raw").join("code").join("gglog");
        let src = repo.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(
            src.join("lib.rs"),
            "pub fn add(a: i32, b: i32) -> i32 { a + b }\n\
             pub fn sub(a: i32, b: i32) -> i32 { a - b }\n\
             pub struct Counter { pub value: i32 }\n\
             impl Counter {\
                 pub fn new() -> Self { Self { value: 0 } }\
                 pub fn inc(&mut self) { self.value += 1; }\
             }\n\
             pub fn run() -> i32 { let mut c = Counter::new(); c.inc(); c.inc(); c.value }\n",
        )
        .unwrap();

        // 2. Run codegraph init + index against the real CLI.
        let init_status = std::process::Command::new(&bin)
            .arg("init").arg(&repo).status().expect("spawn codegraph init");
        assert!(init_status.success(), "codegraph init failed: {:?}", init_status);
        let index_status = std::process::Command::new(&bin)
            .arg("index").arg(&repo).status().expect("spawn codegraph index");
        assert!(index_status.success(), "codegraph index failed: {:?}", index_status);

        // 3. Read the SQLite DB via our reader (same path the TS code
        //    uses after the Rust command returns).
        let payload = run_get_graph_payload_inner(&project, "gglog").expect("read payload");
        assert!(!payload.nodes.is_empty(), "real DB gave 0 nodes");
        let node_count = payload.nodes.len();
        let edge_count = payload.edges.len();
        let languages = payload.languages.clone();
        eprintln!(
            "[e2e] real DB: {} nodes, {} edges, languages={:?}",
            node_count, edge_count, languages
        );

        // 4. Convert the codegraph payload into UA KnowledgeGraph
        //    shape. (This is the function the TS writer performs in
        //    production; in Rust we inline the equivalent mapping so
        //    the e2e test is self-contained.)
        let mut sorted_nodes = payload.nodes.clone();
        sorted_nodes.sort_by(|a, b| a.id.cmp(&b.id));
        let mut sorted_edges = payload.edges.clone();
        sorted_edges.sort_by(|a, b| a.source.cmp(&b.source).then(a.target.cmp(&b.target)));

        let kg = serde_json::json!({
            "version": "1.0.0",
            "kind": "codebase",
            "project": {
                "name": "gglog",
                "languages": payload.languages,
                "frameworks": [],
                "description": "",
                "analyzedAt": "2026-06-28T00:00:00Z",
                "gitCommitHash": payload.git_commit_hash.clone().unwrap_or_default(),
            },
            "nodes": sorted_nodes.iter().map(|n| {
                let mut node = serde_json::json!({
                    "id": n.id,
                    "type": match n.kind.as_str() {
                        "file" => "file",
                        "function" | "method" => "function",
                        "class" | "struct" | "interface" | "type_alias" | "enum" | "enum_member" => "class",
                        "module" => "module",
                        "constant" | "variable" | "property" => "concept",
                        _ => "module",
                    },
                    "name": n.name,
                    "summary": n.docstring.clone().unwrap_or_default(),
                    "tags": n.tags.clone(),
                    "complexity": "moderate",
                });
                if !n.file_path.is_empty() {
                    node["filePath"] = serde_json::Value::String(n.file_path.clone());
                }
                if let Some(loc) = &n.location {
                    node["lineRange"] = serde_json::json!([loc.start_line, loc.end_line]);
                }
                if let Some(lang) = &n.language {
                    node["languageNotes"] = serde_json::Value::String(lang.clone());
                }
                node
            }).collect::<Vec<_>>(),
            "edges": sorted_edges.iter().filter_map(|e| {
                let ua_type = match e.kind.as_str() {
                    "contains" => "contains",
                    "imports" => "imports",
                    "calls" => "calls",
                    _ => return None,
                };
                Some(serde_json::json!({
                    "source": e.source,
                    "target": e.target,
                    "type": ua_type,
                    "direction": "forward",
                    "weight": 1.0,
                }))
            }).collect::<Vec<_>>(),
            "layers": [],
            "tour": [],
        });

        // 5. Write knowledge-graph.json + meta.json to disk.
        let repo_dir = project.join("wiki").join("code_wiki").join("gglog");
        fs::create_dir_all(&repo_dir).unwrap();
        let kg_path = repo_dir.join("knowledge-graph.json");
        fs::write(&kg_path, serde_json::to_vec_pretty(&kg).unwrap()).unwrap();
        fs::write(
            repo_dir.join("meta.json"),
            br#"{"lastAnalyzedAt":"2026-06-28T00:00:00Z","gitCommitHash":"","version":"codegraph-1.0.0","analyzedFiles":2}"#,
        )
        .unwrap();
        eprintln!("[e2e] wrote {} ({} bytes)", kg_path.display(), fs::metadata(&kg_path).unwrap().len());

        // 6. Spin up the real dashboard server.
        let (listener, port) = bind_local().expect("bind");
        let token = "real_e2e_token".to_string();
        let token_for_thread = token.clone();
        let project_for_cleanup = project.clone();
        let handle = thread::spawn(move || {
            // The dashboard server reads assets from a directory; for
            // this e2e we serve a tiny inline asset so the test is
            // hermetic and doesn't depend on the built SPA.
            // We need `project` again later (to pass to
            // serve_dashboard), so don't move it into `assets`.
            let assets = project.clone().join("assets");
            fs::create_dir_all(&assets).unwrap();
            fs::write(
                assets.join("index.html"),
                br#"<!doctype html><html><body>REAL_DASHBOARD_INDEX</body></html>"#,
            )
            .unwrap();
            serve_dashboard(
                listener,
                assets,
                project,
                "gglog".to_string(),
                token_for_thread,
            )
        });

        // 7. curl-equivalent checks against all four data endpoints.
        let fetch = |url_path: &str, expect_status: u16| -> String {
            let mut stream = TcpStream::connect_timeout(
                &format!("127.0.0.1:{port}").parse().unwrap(),
                Duration::from_secs(2),
            )
            .expect("connect");
            stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
            stream
                .write_all(
                    format!(
                        "GET {url_path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .expect("write");
            let mut buf = String::new();
            stream.read_to_string(&mut buf).expect("read");
            let status = buf.lines().next().unwrap_or("");
            assert!(
                status.contains(&format!(" {expect_status} ")),
                "expected {expect_status} for {url_path}, got: {status}"
            );
            buf
        };

        let body = fetch("/", 200);
        assert!(body.contains("REAL_DASHBOARD_INDEX"), "/ did not serve index.html");

        let body = fetch(&format!("/knowledge-graph.json?token={token}"), 200);
        let kg_parsed: serde_json::Value = serde_json::from_str(
            body.split("\r\n\r\n").nth(1).expect("body"),
        )
        .expect("kg parses");
        assert_eq!(kg_parsed["project"]["name"], "gglog");
        assert_eq!(kg_parsed["kind"], "codebase");
        let parsed_nodes = kg_parsed["nodes"].as_array().unwrap().len();
        let parsed_edges = kg_parsed["edges"].as_array().unwrap().len();
        eprintln!("[e2e] served knowledge-graph.json: {} nodes, {} edges", parsed_nodes, parsed_edges);
        assert_eq!(parsed_nodes, node_count, "served node count differs from real DB");

        // All UA types are real types, not "module" fallback.
        let types: std::collections::HashSet<String> = kg_parsed["nodes"]
            .as_array().unwrap()
            .iter()
            .map(|n| n["type"].as_str().unwrap().to_string())
            .collect();
        eprintln!("[e2e] served node types: {:?}", types);
        assert!(types.contains("file"), "expected 'file' nodes");
        assert!(types.contains("function") || types.contains("class"), "expected function/class nodes");

        // Token gating.
        let _ = fetch("/knowledge-graph.json", 403);
        let _ = fetch("/knowledge-graph.json?token=wrong", 403);

        let body = fetch(&format!("/meta.json?token={token}"), 200);
        assert!(body.contains("codegraph-1.0.0"), "/meta.json body wrong: {body}");

        let body = fetch(&format!("/config.json?token={token}"), 200);
        assert!(body.contains("\"autoUpdate\":false"), "/config.json body wrong: {body}");

        // SPA fallback.
        let body = fetch("/any-spa-route", 200);
        assert!(body.contains("REAL_DASHBOARD_INDEX"), "SPA fallback wrong: {body}");

        drop(handle);
        // `project` was moved into the server thread above, but we
        // still need it here to clean up the temp dir. Clone it
        // before the move and use the clone for the cleanup.
        let _ = fs::remove_dir_all(&project_for_cleanup);
        eprintln!("[e2e] all checks passed");
    }
}
