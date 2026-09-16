// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod engine;
mod pdf;

use pdf::{PdfConversionResult, PdfPageInfo};
use std::fs;
use std::sync::{Mutex, Arc};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::GenericImageView;
use tauri_plugin_dialog::DialogExt;
use tauri::Manager;
use tempfile::TempDir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Track temporary directories to clean them up later
// We need to store the TempDir itself, not just the path, to prevent auto-deletion
static TEMP_DIRS: Mutex<Vec<Arc<TempDir>>> = Mutex::new(Vec::new());

/// Clean up old temporary image directories to prevent disk space leaks
#[tauri::command]
fn cleanup_temp_images() -> Result<(), String> {
    let mut dirs = TEMP_DIRS.lock().map_err(|e| format!("Failed to lock temp dirs: {}", e))?;

    println!("[Cleanup] Cleaning up {} temp directories...", dirs.len());

    // Clear the vector - this will drop the Arcs and delete the temp dirs
    dirs.clear();

    println!("[Cleanup] Cleanup complete");
    Ok(())
}

/// Clean up temp files and return count of deleted directories
#[tauri::command]
fn cleanup_temp_files() -> Result<usize, String> {
    let mut dirs = TEMP_DIRS.lock().map_err(|e| format!("Failed to lock temp dirs: {}", e))?;
    let count = dirs.len();

    println!("[GarbageCollector] Deleting {} temp directories...", count);

    dirs.clear();

    println!("[GarbageCollector] Cleanup complete");
    Ok(count)
}

// ═══════════════════════════════════════════════════════════════════════════
// PROMPT TEMPLATE MANAGEMENT
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PromptTemplate {
    name: String,
    instructions: String,
}

/// Get the path to the templates.json file
fn get_templates_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    
    fs::create_dir_all(&app_data_dir)
        .map_err(|e| format!("Failed to create app data dir: {}", e))?;
    
    Ok(app_data_dir.join("templates.json"))
}

/// Load templates from disk
fn load_templates(app: &tauri::AppHandle) -> Result<HashMap<String, PromptTemplate>, String> {
    let templates_path = get_templates_path(app)?;
    
    if !templates_path.exists() {
        return Ok(HashMap::new());
    }
    
    let content = fs::read_to_string(&templates_path)
        .map_err(|e| format!("Failed to read templates file: {}", e))?;
    
    let templates: HashMap<String, PromptTemplate> = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse templates: {}", e))?;
    
    Ok(templates)
}

/// Save templates to disk
fn save_templates_to_disk(app: &tauri::AppHandle, templates: &HashMap<String, PromptTemplate>) -> Result<(), String> {
    let templates_path = get_templates_path(app)?;
    
    let content = serde_json::to_string_pretty(templates)
        .map_err(|e| format!("Failed to serialize templates: {}", e))?;
    
    fs::write(&templates_path, content)
        .map_err(|e| format!("Failed to write templates file: {}", e))?;
    
    Ok(())
}

/// Get all saved prompt templates
#[tauri::command]
fn get_templates(app: tauri::AppHandle) -> Result<Vec<PromptTemplate>, String> {
    let templates = load_templates(&app)?;
    let mut template_vec: Vec<PromptTemplate> = templates.into_values().collect();
    
    // Sort by name for consistent ordering
    template_vec.sort_by(|a, b| a.name.cmp(&b.name));
    
    Ok(template_vec)
}

/// Save a new prompt template
#[tauri::command]
fn save_template(app: tauri::AppHandle, name: String, instructions: String) -> Result<(), String> {
    let mut templates = load_templates(&app)?;
    
    let template = PromptTemplate {
        name: name.clone(),
        instructions,
    };
    
    templates.insert(name.clone(), template);
    save_templates_to_disk(&app, &templates)?;
    
    println!("[Template] Saved template: {}", name);
    Ok(())
}

/// Delete a prompt template
#[tauri::command]
fn delete_template(app: tauri::AppHandle, name: String) -> Result<(), String> {
    let mut templates = load_templates(&app)?;
    
    if templates.remove(&name).is_none() {
        return Err(format!("Template '{}' not found", name));
    }
    
    save_templates_to_disk(&app, &templates)?;
    
    println!("[Template] Deleted template: {}", name);
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// ANALYSIS HISTORY MANAGEMENT
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HistoryEntry {
    id: String,
    timestamp: String,
    file_name: String,
    merchant_name: Option<String>,
    risk_score: Option<String>,
    parsed_data: serde_json::Value,
}

/// Get the path to the analysis_history.json file
fn get_history_path(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Failed to get app data dir: {}", e))?;
    
    fs::create_dir_all(&app_data_dir)
        .map_err(|e| format!("Failed to create app data dir: {}", e))?;
    
    Ok(app_data_dir.join("analysis_history.json"))
}

/// Load history from disk
fn load_history(app: &tauri::AppHandle) -> Result<Vec<HistoryEntry>, String> {
    let history_path = get_history_path(app)?;
    
    if !history_path.exists() {
        return Ok(Vec::new());
    }
    
    let content = fs::read_to_string(&history_path)
        .map_err(|e| format!("Failed to read history file: {}", e))?;
    
    let history: Vec<HistoryEntry> = serde_json::from_str(&content)
        .map_err(|e| format!("Failed to parse history: {}", e))?;
    
    Ok(history)
}

/// Save history to disk
fn save_history_to_disk(app: &tauri::AppHandle, history: &[HistoryEntry]) -> Result<(), String> {
    let history_path = get_history_path(app)?;
    
    let content = serde_json::to_string_pretty(history)
        .map_err(|e| format!("Failed to serialize history: {}", e))?;
    
    fs::write(&history_path, content)
        .map_err(|e| format!("Failed to write history file: {}", e))?;
    
    Ok(())
}

/// Get all analysis history entries (sorted by timestamp, newest first)
#[tauri::command]
fn get_history(app: tauri::AppHandle) -> Result<Vec<HistoryEntry>, String> {
    let mut history = load_history(&app)?;
    
    // Sort by timestamp descending (newest first)
    history.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    
    Ok(history)
}

/// Save a new analysis history entry
#[tauri::command]
fn save_history_entry(
    app: tauri::AppHandle,
    file_name: String,
    merchant_name: Option<String>,
    risk_score: Option<String>,
    parsed_data: serde_json::Value,
) -> Result<(), String> {
    let mut history = load_history(&app)?;
    
    // Generate unique ID from timestamp
    let timestamp = chrono::Utc::now().to_rfc3339();
    let id = format!("{}_{}", timestamp, file_name.replace(" ", "_"));
    
    let entry = HistoryEntry {
        id,
        timestamp,
        file_name: file_name.clone(),
        merchant_name,
        risk_score,
        parsed_data,
    };
    
    history.push(entry);
    save_history_to_disk(&app, &history)?;
    
    println!("[History] Saved analysis for: {}", file_name);
    Ok(())
}

/// Delete a single history entry
#[tauri::command]
fn delete_history_entry(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let mut history = load_history(&app)?;
    
    let original_len = history.len();
    history.retain(|e| e.id != id);
    
    if history.len() == original_len {
        return Err(format!("History entry '{}' not found", id));
    }
    
    save_history_to_disk(&app, &history)?;
    
    println!("[History] Deleted entry: {}", id);
    Ok(())
}

/// Clear all history entries
#[tauri::command]
fn clear_all_history(app: tauri::AppHandle) -> Result<(), String> {
    save_history_to_disk(&app, &[])?;
    
    println!("[History] Cleared all history");
    Ok(())
}

#[tauri::command]
async fn read_file_as_base64(file_path: String) -> Result<String, String> {
    let data = fs::read(&file_path)
        .map_err(|e| format!("Failed to read file: {}", e))?;
    Ok(BASE64.encode(&data))
}

/// Convert PDF to temporary JPEG images on disk
/// Returns file paths instead of base64 to avoid memory limits
#[tauri::command]
async fn convert_pdf_to_images(pdf_path: String, dpi: u32) -> Result<PdfConversionResult, String> {
    use std::process::Command;
    use image::ImageFormat;

    println!("[PDF] Converting PDF to JPEG images at {} DPI...", dpi);

    // DON'T cleanup old temp dirs here - they're needed for the preview!
    // Cleanup happens when the app closes or via explicit cleanup_temp_images() call

    // Create temp directory for page images
    let temp_dir = TempDir::new()
        .map_err(|e| format!("Failed to create temp dir: {}", e))?;
    
    // Store Arc to temp_dir to prevent it from being deleted
    let temp_dir_arc = Arc::new(temp_dir);
    if let Ok(mut dirs) = TEMP_DIRS.lock() {
        dirs.push(temp_dir_arc.clone());
        println!("[PDF] Tracking temp dir (total tracked: {})", dirs.len());
    }

    // Try pdftocairo first (part of poppler-utils)
    let output_prefix = temp_dir_arc.path().join("page");
    let output_pattern = output_prefix.to_str().unwrap();

    println!("[PDF] Running pdftocairo...");

    let result = Command::new("pdftocairo")
        .args([
            "-png",  // Convert to PNG first, then we'll compress to JPEG
            "-r", &dpi.to_string(),
            &pdf_path,
            output_pattern
        ])
        .output();

    match result {
        Ok(output) if output.status.success() => {
            let mut pages = Vec::new();
            let mut image_paths = Vec::new();
            let mut page_num = 1;

            println!("[PDF] Conversion successful, compressing to JPEG...");

            loop {
                let png_path = format!("{}-{}.png", output_pattern, page_num);
                if std::path::Path::new(&png_path).exists() {
                    println!("[PDF] Processing page {}...", page_num);

                    // Read PNG
                    let png_data = fs::read(&png_path)
                        .map_err(|e| format!("Failed to read page {}: {}", page_num, e))?;

                    let img = image::load_from_memory(&png_data)
                        .map_err(|e| format!("Failed to decode image: {}", e))?;
                    let (width, height) = img.dimensions();

                    // Convert to grayscale for smaller file size (bank statements are B&W anyway)
                    let grayscale_img = img.grayscale();

                    // Save JPEG to disk (don't keep in memory)
                    let jpeg_path = format!("{}-{}.jpg", output_pattern, page_num);
                    grayscale_img.save_with_format(&jpeg_path, ImageFormat::Jpeg)
                        .map_err(|e| format!("Failed to save page {} to JPEG: {}", page_num, e))?;

                    println!("[PDF] Page {}: JPEG saved to {}", page_num, jpeg_path);

                    pages.push(PdfPageInfo { page_number: page_num, width, height });
                    image_paths.push(jpeg_path);

                    // Clean up temp PNG immediately
                    let _ = fs::remove_file(&png_path);

                    page_num += 1;
                } else {
                    break;
                }
            }

            println!("[PDF] Converted {} pages to JPEG", pages.len());

            if pages.is_empty() {
                return Err("No pages were converted from PDF".to_string());
            }

            // Return paths as base64-encoded strings (legacy transport shape for the frontend)
            let paths_as_base64: Vec<String> = image_paths.iter()
                .map(|p| BASE64.encode(p.as_bytes()))
                .collect();

            // Generate Data URI for frontend preview from page-1.jpg
            let preview_image_data_uri = if let Some(first_page_path) = image_paths.first() {
                println!("[PDF] Generating Data URI from: {}", first_page_path);
                
                match fs::read(first_page_path) {
                    Ok(image_bytes) => {
                        let base64_string = BASE64.encode(&image_bytes);
                        let data_uri = format!("data:image/jpeg;base64,{}", base64_string);
                        println!("[PDF] Data URI generated: {} bytes", data_uri.len());
                        Some(data_uri)
                    }
                    Err(e) => {
                        println!("[PDF] Failed to read preview image: {}", e);
                        None
                    }
                }
            } else {
                println!("[PDF] No image paths available for Data URI");
                None
            };

            // Return first page path for frontend preview (absolute path, for debugging)
            let preview_path = image_paths.first().cloned();

            // Log the actual file path and check if it exists
            if let Some(ref path) = preview_path {
                println!("[PDF] Preview path: {}", path);
                println!("[PDF] Preview file exists: {}", std::path::Path::new(path).exists());

                // Get file size for debugging
                if let Ok(metadata) = std::fs::metadata(path) {
                    println!("[PDF] Preview file size: {} bytes", metadata.len());
                }
            } else {
                println!("[PDF] WARNING: No preview_path available!");
            }

            // DON'T drop temp_dir_arc - it's stored in TEMP_DIRS and will be cleaned up later
            // Keep the Arc alive by storing it in the result
            // The temp_dir_arc will be dropped when PdfConversionResult is dropped

            Ok(PdfConversionResult {
                pages,
                images: paths_as_base64,
                preview_path,
                preview_image_data_uri,
            })
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("pdftocairo failed: {}", stderr))
        }
        Err(e) => {
            Err(format!("pdftocairo not found. Please install poppler-utils: {}\n\nFor Ubuntu/Debian: sudo apt install poppler-utils\nFor Arch: sudo pacman -S poppler\nFor Fedora: sudo dnf install poppler-utils", e))
        }
    }
}

/// Export JSON data to file using native save dialog
#[tauri::command]
async fn export_json(
    app: tauri::AppHandle,
    data: serde_json::Value,
    default_path: String,
) -> Result<String, String> {
    let content = serde_json::to_string_pretty(&data)
        .map_err(|e| format!("Failed to serialize JSON: {}", e))?;

    // Show save dialog
    let file_path = app.dialog()
        .file()
        .set_file_name(&default_path)
        .add_filter("JSON Files", &["json"])
        .blocking_save_file();

    if let Some(path) = file_path {
        // Convert FilePath to PathBuf
        let path_buf = path.into_path()
            .map_err(|_| "Failed to convert file path".to_string())?;
        
        fs::write(&path_buf, content)
            .map_err(|e| format!("Failed to write file: {}", e))?;
        println!("[Export] JSON saved to: {:?}", path_buf);
        Ok(path_buf.to_string_lossy().to_string())
    } else {
        Ok(String::new()) // User canceled
    }
}

/// Export CSV content to file using native save dialog
#[tauri::command]
async fn export_csv(
    app: tauri::AppHandle,
    content: String,
    default_path: String,
) -> Result<String, String> {
    // Show save dialog
    let file_path = app.dialog()
        .file()
        .set_file_name(&default_path)
        .add_filter("CSV Files", &["csv"])
        .blocking_save_file();

    if let Some(path) = file_path {
        // Convert FilePath to PathBuf
        let path_buf = path.into_path()
            .map_err(|_| "Failed to convert file path".to_string())?;
        
        fs::write(&path_buf, content)
            .map_err(|e| format!("Failed to write file: {}", e))?;
        println!("[Export] CSV saved to: {:?}", path_buf);
        Ok(path_buf.to_string_lossy().to_string())
    } else {
        Ok(String::new()) // User canceled
    }
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            read_file_as_base64,
            convert_pdf_to_images,
            cleanup_temp_images,
            cleanup_temp_files,
            export_json,
            export_csv,
            get_templates,
            save_template,
            delete_template,
            get_history,
            save_history_entry,
            delete_history_entry,
            clear_all_history,
            engine::engine_status,
            engine::engine_save_config,
            engine::engine_install,
            engine::engine_start,
            engine::engine_stop,
            engine::engine_use_cpu_backend,
            engine::engine_devices,
            engine::engine_analyze,
            engine::engine_chat
        ])
        .manage(engine::runtime::EngineProcess::default())
        .setup(|app| {
            // `--headless-analyze` runs the pipeline from a terminal for testing.
            if let Some(args) = engine::headless::parse_args() {
                engine::headless::run(app.handle(), args);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Always take llama-server down with the app; it is a child we own.
            if let tauri::RunEvent::Exit = event {
                app.state::<engine::runtime::EngineProcess>().stop();
            }
        });
}
