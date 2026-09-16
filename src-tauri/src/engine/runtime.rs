//! Built-in inference runtime: install state, llama-server process lifecycle, config.
//!
//! Layout under the OS app-data directory:
//! ```text
//! engine/
//!   engine_config.json        user choices (mode, backend, reasoning model)
//!   runtime/<tag>-<backend>/  extracted llama.cpp release
//!   models/<model id>/        GGUF files
//!   models.ini                llama-server preset written on every start
//!   llama-server.log          stdout/stderr of the last run
//! ```

use super::registry::{self, Asset, Backend, ModelSpec};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::Manager;

/// Which inference engine the app talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EngineMode {
    /// llama-server managed by the app (default for new installs).
    Builtin,
    /// A user-run Ollama at a configurable URL (legacy path, kept for remote GPU boxes).
    Ollama,
}

/// Persisted engine choices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineConfig {
    pub mode: EngineMode,
    pub backend: Backend,
    /// Id of the chosen reasoning model, see `registry::underwriter_models`.
    pub underwriter_model: String,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            mode: EngineMode::Builtin,
            backend: Backend::Gpu,
            underwriter_model: registry::underwriter_models()[0].id.to_string(),
        }
    }
}

/// Running llama-server handle. Managed as Tauri state.
#[derive(Default)]
pub struct EngineProcess {
    inner: Mutex<Option<Running>>,
}

struct Running {
    child: Child,
    port: u16,
    api_key: String,
}

/// Connection details the HTTP client needs.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: String,
}

impl EngineProcess {
    pub fn endpoint(&self) -> Option<Endpoint> {
        let guard = self.inner.lock().ok()?;
        guard.as_ref().map(|r| Endpoint {
            base_url: format!("http://127.0.0.1:{}", r.port),
            api_key: r.api_key.clone(),
        })
    }

    pub fn is_running(&self) -> bool {
        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(_) => return false,
        };
        match guard.as_mut() {
            Some(r) => match r.child.try_wait() {
                Ok(None) => true,
                // Exited on its own (crash); forget the stale handle.
                _ => {
                    *guard = None;
                    false
                }
            },
            None => false,
        }
    }

    /// Stop the router. A graceful signal first so the router shuts down the per-model
    /// child processes it spawned; a hard kill only if it does not exit in time.
    pub fn stop(&self) {
        let running = match self.inner.lock() {
            Ok(mut g) => g.take(),
            Err(_) => None,
        };
        let Some(mut r) = running else { return };
        let pid = r.child.id();
        println!("[Engine] Stopping llama-server (pid {pid})");
        graceful_terminate(pid);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = r.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = r.child.kill();
        let _ = r.child.wait();
    }
}

#[cfg(unix)]
fn graceful_terminate(pid: u32) {
    // SIGTERM lets the router reap its model subprocesses.
    let _ = Command::new("kill").args(["-TERM", &pid.to_string()]).status();
}

#[cfg(windows)]
fn graceful_terminate(pid: u32) {
    // /T kills the whole tree (router + model children).
    let _ = Command::new("taskkill")
        .args(["/T", "/F", "/PID", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// ─── Paths ────────────────────────────────────────────────────────────────

pub fn engine_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Cannot resolve app data dir: {e}"))?
        .join("engine");
    std::fs::create_dir_all(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    Ok(dir)
}

fn config_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(engine_dir(app)?.join("engine_config.json"))
}

pub fn load_config(app: &tauri::AppHandle) -> EngineConfig {
    config_path(app)
        .ok()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_config(app: &tauri::AppHandle, cfg: &EngineConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    std::fs::write(config_path(app)?, json).map_err(|e| format!("Cannot save engine config: {e}"))
}

pub fn runtime_dir(app: &tauri::AppHandle, backend: Backend) -> Result<PathBuf, String> {
    let name = format!(
        "{}-{}",
        registry::LLAMA_RELEASE_TAG,
        match backend {
            Backend::Gpu => "gpu",
            Backend::Cpu => "cpu",
        }
    );
    Ok(engine_dir(app)?.join("runtime").join(name))
}

pub fn models_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(engine_dir(app)?.join("models"))
}

pub fn asset_path(app: &tauri::AppHandle, model: &ModelSpec, asset: &Asset) -> Result<PathBuf, String> {
    Ok(models_dir(app)?.join(model.id).join(&asset.file_name))
}

fn archive_path(app: &tauri::AppHandle, backend: Backend) -> Result<PathBuf, String> {
    let asset = registry::runtime_asset(backend).ok_or("Unsupported platform")?;
    Ok(engine_dir(app)?.join("runtime").join(&asset.file_name))
}

/// Path to the extracted llama-server binary, if the runtime is installed.
pub fn server_binary(app: &tauri::AppHandle, backend: Backend) -> Option<PathBuf> {
    let dir = runtime_dir(app, backend).ok()?;
    let name = if cfg!(windows) { "llama-server.exe" } else { "llama-server" };
    find_file(&dir, name, 3)
}

fn find_file(dir: &Path, name: &str, depth: u32) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file() && path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
        if path.is_dir() {
            subdirs.push(path);
        }
    }
    if depth == 0 {
        return None;
    }
    subdirs.into_iter().find_map(|d| find_file(&d, name, depth - 1))
}

// ─── Install ──────────────────────────────────────────────────────────────

/// Download and unpack the llama.cpp release for `backend`. Idempotent.
pub async fn install_runtime(app: &tauri::AppHandle, backend: Backend) -> Result<PathBuf, String> {
    if let Some(bin) = server_binary(app, backend) {
        return Ok(bin);
    }
    let asset = registry::runtime_asset(backend).ok_or_else(|| {
        format!(
            "No llama.cpp build for {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    })?;
    let archive = archive_path(app, backend)?;
    super::download::download_asset(app, &asset, &archive).await?;

    let dest = runtime_dir(app, backend)?;
    let archive2 = archive.clone();
    let dest2 = dest.clone();
    tokio::task::spawn_blocking(move || extract_archive(&archive2, &dest2))
        .await
        .map_err(|e| e.to_string())??;
    let _ = std::fs::remove_file(&archive);

    server_binary(app, backend).ok_or_else(|| {
        format!("Archive unpacked but llama-server was not found under {}", dest.display())
    })
}

fn extract_archive(archive: &Path, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).map_err(|e| e.to_string())?;
    }
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let file = std::fs::File::open(archive).map_err(|e| format!("Cannot open archive: {e}"))?;
    let name = archive.to_string_lossy();
    if name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("Bad zip: {e}"))?;
        zip.extract(dest).map_err(|e| format!("Unzip failed: {e}"))?;
    } else {
        let gz = flate2::read::GzDecoder::new(file);
        let mut tar = tar::Archive::new(gz);
        tar.unpack(dest).map_err(|e| format!("Untar failed: {e}"))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(bin) = find_file(dest, "llama-server", 3) {
            let _ = std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755));
        }
    }
    Ok(())
}

/// Download every file of `model`. Idempotent.
pub async fn install_model(app: &tauri::AppHandle, model: &ModelSpec) -> Result<(), String> {
    for asset in &model.files {
        let dest = asset_path(app, model, asset)?;
        super::download::download_asset(app, asset, &dest).await?;
    }
    Ok(())
}

pub fn model_installed(app: &tauri::AppHandle, model: &ModelSpec) -> bool {
    model.files.iter().all(|a| {
        asset_path(app, model, a)
            .map(|p| super::download::is_complete(a, &p))
            .unwrap_or(false)
    })
}

// ─── Run ──────────────────────────────────────────────────────────────────

/// Write the preset file llama-server reads: one section per role.
fn write_presets(app: &tauri::AppHandle, ocr: &ModelSpec, underwriter: &ModelSpec) -> Result<PathBuf, String> {
    let mut ini = String::new();
    for m in [ocr, underwriter] {
        ini.push_str(&format!("[{}]\n", m.role));
        ini.push_str(&format!("model = {}\n", asset_path(app, m, m.main_file())?.display()));
        if let Some(mm) = m.mmproj_file() {
            ini.push_str(&format!("mmproj = {}\n", asset_path(app, m, mm)?.display()));
        }
        ini.push_str(&format!("ctx-size = {}\n\n", m.ctx_size));
    }
    let path = engine_dir(app)?.join("models.ini");
    std::fs::write(&path, ini).map_err(|e| format!("Cannot write presets: {e}"))?;
    Ok(path)
}

fn free_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
    Ok(listener.local_addr().map_err(|e| e.to_string())?.port())
}

fn random_key() -> String {
    // Enough entropy for a loopback-only bearer token.
    let mut bytes = [0u8; 24];
    getrandom::getrandom(&mut bytes).expect("os randomness");
    format!("mcaw_{}", bytes.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

/// Start llama-server in router mode with both models registered. Returns once
/// `/health` answers. Model weights load lazily on the first request for each role.
pub async fn start(app: &tauri::AppHandle, cfg: &EngineConfig) -> Result<Endpoint, String> {
    let state = app.state::<EngineProcess>();
    if state.is_running() {
        return state.endpoint().ok_or("engine state inconsistent".into());
    }

    let bin = server_binary(app, cfg.backend).ok_or("Runtime is not installed")?;
    let ocr = registry::ocr_model();
    let underwriter = registry::underwriter_model(&cfg.underwriter_model)
        .ok_or_else(|| format!("Unknown reasoning model {}", cfg.underwriter_model))?;
    if !model_installed(app, &ocr) {
        return Err("OCR model is not installed".into());
    }
    if !model_installed(app, &underwriter) {
        return Err(format!("{} is not installed", underwriter.display_name));
    }

    let presets = write_presets(app, &ocr, &underwriter)?;
    let port = free_port()?;
    let api_key = random_key();
    let log_path = engine_dir(app)?.join("llama-server.log");
    let log = std::fs::File::create(&log_path).map_err(|e| format!("Cannot create log: {e}"))?;
    let log_err = log.try_clone().map_err(|e| e.to_string())?;

    let mut cmd = Command::new(&bin);
    cmd.args([
        "--models-preset", &presets.to_string_lossy(),
        "--host", "127.0.0.1",
        "--port", &port.to_string(),
        // Both roles stay resident when memory allows; the router evicts LRU otherwise.
        "--models-max", "2",
        "-ngl", "auto",
        "--jinja",
        "--no-webui",
        "--api-key", &api_key,
    ])
    // Keep the router from discovering unrelated GGUFs in the user's llama.cpp cache.
    .env("LLAMA_CACHE", engine_dir(app)?.join("cache"))
    .current_dir(bin.parent().unwrap_or(Path::new(".")))
    .stdin(Stdio::null())
    .stdout(Stdio::from(log))
    .stderr(Stdio::from(log_err));

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn().map_err(|e| format!("Cannot start llama-server: {e}"))?;
    println!("[Engine] llama-server pid {} on port {port} ({:?})", child.id(), cfg.backend);

    let base_url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            let tail = std::fs::read_to_string(&log_path).unwrap_or_default();
            let tail: String = tail.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
            return Err(format!("llama-server exited early ({status}).\n{tail}"));
        }
        if let Ok(resp) = client.get(format!("{base_url}/health")).send().await {
            if resp.status().is_success() {
                break;
            }
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return Err("llama-server did not become healthy within 30 seconds".into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let endpoint = Endpoint { base_url, api_key: api_key.clone() };
    if let Ok(mut g) = state.inner.lock() {
        *g = Some(Running { child, port, api_key });
    }
    Ok(endpoint)
}
