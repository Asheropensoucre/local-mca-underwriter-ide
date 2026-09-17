//! Built-in inference engine: llama-server managed by the app, an OCR model that
//! reads pages and a text model that writes the underwriting report.
//!
//! The Tauri commands in this file are thin; the work lives in the submodules.

pub mod download;
pub mod headless;
pub mod ledger;
pub mod llama;
pub mod memory;
pub mod ocr_table;
pub mod pipeline;
pub mod registry;
pub mod runtime;

use registry::{Backend, ModelSpec};
use runtime::{EngineConfig, EngineProcess};
use serde::Serialize;
use serde_json::json;
use tauri::{Emitter, Manager};

#[derive(Debug, Serialize)]
pub struct ModelStatus {
    pub id: String,
    pub role: String,
    pub display_name: String,
    pub hf_repo: String,
    pub total_size: u64,
    pub min_ram_gb: u32,
    pub installed: bool,
    pub files: Vec<FileStatus>,
}

#[derive(Debug, Serialize)]
pub struct FileStatus {
    pub asset_id: String,
    pub file_name: String,
    pub size: u64,
    pub installed: bool,
}

#[derive(Debug, Serialize)]
pub struct Hardware {
    pub os: String,
    pub arch: String,
    pub total_ram_gb: f32,
    pub free_disk_gb: f32,
    pub cpu_threads: usize,
}

/// Everything the setup screen and Settings need in one call.
#[derive(Debug, Serialize)]
pub struct EngineStatus {
    pub config: EngineConfig,
    pub platform_supported: bool,
    pub runtime_size: u64,
    pub runtime_installed: bool,
    pub ocr: ModelStatus,
    pub underwriters: Vec<ModelStatus>,
    pub hardware: Hardware,
    pub running: bool,
    /// Runtime plus both required models are on disk.
    pub ready: bool,
    pub engine_dir: String,
    /// Memory sizing for the chosen models against the RAM free right now.
    pub memory: memory::MemoryPlan,
    /// Set when the memory watchdog stopped the engine.
    pub stopped_reason: Option<String>,
}

fn model_status(app: &tauri::AppHandle, m: &ModelSpec) -> ModelStatus {
    ModelStatus {
        id: m.id.to_string(),
        role: m.role.to_string(),
        display_name: m.display_name.to_string(),
        hf_repo: m.hf_repo.to_string(),
        total_size: m.total_size(),
        min_ram_gb: m.min_ram_gb,
        installed: runtime::model_installed(app, m),
        files: m
            .files
            .iter()
            .map(|a| FileStatus {
                asset_id: a.id.to_string(),
                file_name: a.file_name.clone(),
                size: a.size,
                installed: runtime::asset_path(app, m, a).map(|p| download::is_complete(a, &p)).unwrap_or(false),
            })
            .collect(),
    }
}

fn hardware(app: &tauri::AppHandle) -> Hardware {
    use sysinfo::{Disks, System};
    let mut sys = System::new();
    sys.refresh_memory();
    let total_ram_gb = sys.total_memory() as f32 / 1e9;

    let free_disk_gb = runtime::engine_dir(app)
        .ok()
        .and_then(|dir| {
            let disks = Disks::new_with_refreshed_list();
            // Longest mount point that is a prefix of the engine dir is the disk it lives on.
            disks
                .iter()
                .filter(|d| dir.starts_with(d.mount_point()))
                .max_by_key(|d| d.mount_point().as_os_str().len())
                .map(|d| d.available_space() as f32 / 1e9)
        })
        .unwrap_or(0.0);

    Hardware {
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        total_ram_gb,
        free_disk_gb,
        cpu_threads: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
    }
}

#[tauri::command]
pub fn engine_status(app: tauri::AppHandle) -> Result<EngineStatus, String> {
    let config = runtime::load_config(&app);
    let ocr = model_status(&app, &registry::ocr_model());
    let underwriters: Vec<ModelStatus> = registry::underwriter_models().iter().map(|m| model_status(&app, m)).collect();
    let runtime_asset = registry::runtime_asset(config.backend);
    let runtime_installed = runtime::server_binary(&app, config.backend).is_some();
    let chosen_installed = underwriters.iter().any(|m| m.id == config.underwriter_model && m.installed);
    let state = app.state::<EngineProcess>();
    let running = state.is_running();
    let chosen = registry::underwriter_model(&config.underwriter_model).unwrap_or_else(|| registry::underwriter_models()[0].clone());
    let memory = memory::plan(&registry::ocr_model(), &chosen);
    let stopped_reason = state.stopped_reason.lock().ok().and_then(|g| g.clone());
    Ok(EngineStatus {
        memory,
        stopped_reason,
        platform_supported: runtime_asset.is_some(),
        runtime_size: runtime_asset.map(|a| a.size).unwrap_or(0),
        runtime_installed,
        ready: runtime_installed && ocr.installed && chosen_installed,
        ocr,
        underwriters,
        hardware: hardware(&app),
        running,
        engine_dir: runtime::engine_dir(&app)?.to_string_lossy().to_string(),
        config,
    })
}

#[tauri::command]
pub fn engine_save_config(app: tauri::AppHandle, config: EngineConfig) -> Result<(), String> {
    registry::underwriter_model(&config.underwriter_model)
        .ok_or_else(|| format!("Unknown reasoning model {}", config.underwriter_model))?;
    runtime::save_config(&app, &config)
}

/// Download whatever is missing for the current config: runtime, OCR model, reasoning
/// model. Progress arrives as `engine-download-progress` events; this resolves when
/// everything is on disk.
#[tauri::command]
pub async fn engine_install(app: tauri::AppHandle) -> Result<(), String> {
    let cfg = runtime::load_config(&app);
    runtime::install_runtime(&app, cfg.backend).await?;
    runtime::install_model(&app, &registry::ocr_model()).await?;
    let uw = registry::underwriter_model(&cfg.underwriter_model).ok_or("Unknown reasoning model")?;
    runtime::install_model(&app, &uw).await?;
    Ok(())
}

/// Start llama-server. Returns the local base URL (for display only; the API key
/// never leaves the Rust side).
#[tauri::command]
pub async fn engine_start(app: tauri::AppHandle) -> Result<String, String> {
    let cfg = runtime::load_config(&app);
    let ep = runtime::start(&app, &cfg).await?;
    Ok(ep.base_url)
}

#[tauri::command]
pub fn engine_stop(app: tauri::AppHandle) {
    app.state::<EngineProcess>().stop();
}

/// Switch the runtime to the CPU build (after a GPU start failure) and restart.
#[tauri::command]
pub async fn engine_use_cpu_backend(app: tauri::AppHandle) -> Result<String, String> {
    let mut cfg = runtime::load_config(&app);
    cfg.backend = Backend::Cpu;
    runtime::save_config(&app, &cfg)?;
    app.state::<EngineProcess>().stop();
    runtime::install_runtime(&app, Backend::Cpu).await?;
    let ep = runtime::start(&app, &cfg).await?;
    Ok(ep.base_url)
}

/// Output of `llama-server --list-devices` for the Settings screen.
#[tauri::command]
pub fn engine_devices(app: tauri::AppHandle) -> Result<String, String> {
    let cfg = runtime::load_config(&app);
    let bin = runtime::server_binary(&app, cfg.backend).ok_or("Runtime is not installed")?;
    let out = std::process::Command::new(&bin)
        .arg("--list-devices")
        .current_dir(bin.parent().unwrap())
        .output()
        .map_err(|e| e.to_string())?;
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok(text
        .lines()
        .skip_while(|l| !l.contains("Available devices"))
        .collect::<Vec<_>>()
        .join("\n"))
}

/// If the memory watchdog stopped the engine, its reason is the error the user should
/// see, not the broken connection that followed.
fn watchdog_reason(app: &tauri::AppHandle) -> Option<String> {
    app.state::<EngineProcess>().stopped_reason.lock().ok().and_then(|g| g.clone())
}

async fn ensure_running(app: &tauri::AppHandle) -> Result<runtime::Endpoint, String> {
    let state = app.state::<EngineProcess>();
    if let Some(ep) = state.endpoint().filter(|_| state.is_running()) {
        // Models are resident; the job's working set must still fit with headroom.
        memory::check_before_job(state.ocr_parallel())?;
        return Ok(ep);
    }
    if let Some(reason) = state.stopped_reason.lock().ok().and_then(|g| g.clone()) {
        // The watchdog stopped it: only restart when memory has recovered.
        let plan = memory::plan(&registry::ocr_model(), &registry::underwriter_model(&runtime::load_config(app).underwriter_model).unwrap_or_else(registry::ocr_model));
        if !plan.fits {
            return Err(format!("{reason}\n{}", plan.message));
        }
    }
    let cfg = runtime::load_config(app);
    runtime::start(app, &cfg).await
}

/// Analyze one or more statements of the same merchant as a single job.
/// Emits the same `analysis-progress` / `analysis-complete` events the UI already
/// listens to and returns the report JSON text.
#[tauri::command]
pub async fn engine_analyze(
    app: tauri::AppHandle,
    pdf_paths: Vec<String>,
    custom_instructions: String,
    temperature: f32,
    max_tokens: i32,
) -> Result<String, String> {
    if pdf_paths.is_empty() {
        return Err("No files to analyze".into());
    }
    let ep = ensure_running(&app).await?;

    let mut total_pages = 0;
    for p in &pdf_paths {
        total_pages += pipeline::page_count(p)?;
    }
    let _ = app.emit("analysis-progress", json!({
        "type": "start", "total_pages": total_pages,
        "message": format!("Reading {total_pages} pages across {} file(s)", pdf_paths.len())
    }));

    let started = std::time::Instant::now();
    let pages = pipeline::read_pages(&app, Some(&ep), &pdf_paths, total_pages).await.map_err(|e| watchdog_reason(&app).unwrap_or(e))?;
    let ocr_pages = pages.iter().filter(|p| p.method == "ocr").count();
    println!("[Engine] {} pages read in {:.1}s ({ocr_pages} via OCR)", pages.len(), started.elapsed().as_secs_f32());

    let _ = app.emit("analysis-progress", json!({
        "type": "aggregating", "total_pages": total_pages,
        "message": "Underwriting: the reasoning model is writing the report"
    }));
    let result = pipeline::underwrite(&app, &ep, &pages, &custom_instructions, temperature, max_tokens).await.map_err(|e| watchdog_reason(&app).unwrap_or(e))?;
    println!("[Engine] job done in {:.1}s", started.elapsed().as_secs_f32());

    let _ = app.emit("analysis-complete", json!({
        "type": "complete", "result": result, "total_pages": total_pages,
        "seconds": started.elapsed().as_secs_f32(), "ocr_pages": ocr_pages,
        "message": "Analysis complete"
    }));
    Ok(result)
}

#[tauri::command]
pub async fn engine_chat(app: tauri::AppHandle, prompt: String, temperature: f32, max_tokens: i32) -> Result<String, String> {
    let ep = ensure_running(&app).await?;
    pipeline::chat(&app, &ep, &prompt, temperature, max_tokens).await
}
