// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ollama;

use ollama::{OllamaChatRequest, OllamaMessage, OllamaOptions, OllamaModelsResponse, PdfConversionResult, PdfPageInfo};
use std::fs;
use std::sync::{Mutex, Arc};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::GenericImageView;
use tauri_plugin_dialog::DialogExt;
use tauri::Emitter;
use tauri::Manager;
use tempfile::TempDir;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Track temporary directories to clean them up later
// We need to store the TempDir itself, not just the path, to prevent auto-deletion
static TEMP_DIRS: Mutex<Vec<Arc<TempDir>>> = Mutex::new(Vec::new());

// Regex for stripping think tags from Qwen3-VL responses
lazy_static::lazy_static! {
    static ref THINK_TAG_REGEX: regex::Regex = regex::Regex::new(r"(?s)<think>.*?</think>").unwrap();
}

/// Strip <think>...</think> tags from model responses (Qwen3-VL thinking models)
fn strip_think_tags(response: &str) -> String {
    THINK_TAG_REGEX.replace_all(response, "").trim().to_string()
}

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
async fn check_ollama_connection() -> Result<bool, String> {
    let client = reqwest::Client::new();
    match client.get("http://localhost:11434/api/tags").send().await {
        Ok(response) => {
            let status = response.status().is_success();
            println!("[Health] Ollama health check: {}", if status { "OK" } else { "FAILED" });
            Ok(status)
        },
        Err(e) => {
            println!("[Health] Ollama connection failed: {}", e);
            Ok(false)
        }
    }
}

#[tauri::command]
async fn get_ollama_models() -> Result<Vec<ollama::OllamaModel>, String> {
    let client = reqwest::Client::new();
    let response = client
        .get("http://localhost:11434/api/tags")
        .send()
        .await
        .map_err(|e| format!("Failed to connect to Ollama: {}", e))?;
    
    let models: OllamaModelsResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse models: {}", e))?;
    
    Ok(models.models)
}

#[tauri::command]
async fn send_to_ollama(
    model: String,
    prompt: String,
    image_path: Option<String>,
    temperature: f32,
    max_tokens: i32,
) -> Result<String, String> {
    let client = reqwest::Client::new();

    // Read and encode image if provided
    let images = if let Some(path) = image_path {
        let image_data = fs::read(&path)
            .map_err(|e| format!("Failed to read image: {}", e))?;
        let base64_image = BASE64.encode(&image_data);
        Some(vec![base64_image])
    } else {
        None
    };

    let request = OllamaChatRequest {
        model: model.clone(),
        messages: vec![OllamaMessage {
            role: "user".to_string(),
            content: prompt,
            images,
        }],
        stream: false,
        options: Some(OllamaOptions {
            temperature: Some(temperature),
            num_predict: Some(max_tokens),
            num_ctx: Some(8192),
        }),
        format: None,
    };

    let response = client
        .post("http://localhost:11434/api/chat")
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("Failed to send request: {}", e))?;

    let result: ollama::OllamaChatResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse response: {}", e))?;

    // Strip think tags from qwen3 thinking models
    let cleaned_response = strip_think_tags(&result.message.content);

    Ok(cleaned_response)
}

#[tauri::command]
async fn read_file_as_base64(file_path: String) -> Result<String, String> {
    let data = fs::read(&file_path)
        .map_err(|e| format!("Failed to read file: {}", e))?;
    Ok(BASE64.encode(&data))
}

#[tauri::command]
async fn test_ollama_model(model: String) -> Result<String, String> {
    println!("[Test] Testing Ollama model: {}", model);

    // Increase timeout for thinking models (qwen3-vl can take 30+ seconds to think)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let request = OllamaChatRequest {
        model: model.clone(),
        messages: vec![OllamaMessage {
            role: "user".to_string(),
            content: "Say hello in one word".to_string(),
            images: None,
        }],
        stream: false,
        options: Some(OllamaOptions {
            temperature: Some(0.1),
            num_predict: Some(10),
            num_ctx: Some(4096),
        }),
        format: None,
    };

    let response = client
        .post("http://localhost:11434/api/chat")
        .json(&request)
        .send()
        .await
        .map_err(|e| {
            println!("[Test] Request failed: {}", e);
            format!("Failed to send request: {}. Is Ollama running?", e)
        })?;

    let result: ollama::OllamaChatResponse = response
        .json()
        .await
        .map_err(|e| {
            println!("[Test] Parse failed: {}", e);
            format!("Failed to parse response: {}", e)
        })?;

    // Strip think tags from qwen3 thinking models
    let cleaned_response = strip_think_tags(&result.message.content);

    println!("[Test] Success: {}", cleaned_response);
    Ok(cleaned_response)
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

            // Return paths as base64-encoded strings for Ollama
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

/// Analyze a single page with Ollama
async fn analyze_single_page(
    client: &reqwest::Client,
    model: &str,
    prompt: &str,
    base64_image: &str,
    temperature: f32,
    max_tokens: i32,
    page_num: usize,
    total_pages: usize,
) -> Result<String, String> {
    let page_prompt = if total_pages > 1 {
        format!("Page {} of {}. {}\n\nExtract data from this page only. Be thorough.",
            page_num, total_pages, prompt)
    } else {
        prompt.to_string()
    };

    // For thinking models like Qwen3-VL, don't send format: "json" as it can conflict
    let format_json = if !model.to_lowercase().contains("qwen3") {
        Some("json".to_string())
    } else {
        None
    };

    let request = OllamaChatRequest {
        model: model.to_string(),
        messages: vec![OllamaMessage {
            role: "user".to_string(),
            content: page_prompt,
            images: Some(vec![base64_image.to_string()]),
        }],
        stream: false,
        options: Some(OllamaOptions {
            temperature: Some(temperature),
            num_predict: Some(max_tokens),
            num_ctx: Some(8192), // 8K context for individual page analysis
        }),
        format: format_json,
    };

    println!("[Multi-page] Analyzing page {}/{}...", page_num, total_pages);

    let response = client
        .post("http://localhost:11434/api/chat")
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("Page {} request failed: {}", page_num, e))?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!("Page {} - Ollama returned status {}: {}", page_num, status, error_text));
    }

    let result: ollama::OllamaChatResponse = response
        .json()
        .await
        .map_err(|e| format!("Page {} - Failed to parse response: {}", page_num, e))?;

    // Strip think tags from Qwen3-VL responses
    let cleaned_response = strip_think_tags(&result.message.content);

    Ok(cleaned_response)
}

/// Aggregate results from multiple pages into final analysis
/// Strict JSON merger - combines page results without echoing the original prompt
async fn aggregate_page_results(
    client: &reqwest::Client,
    model: &str,
    _original_prompt: &str,
    page_results: &[String],
    temperature: f32,
    max_tokens: i32,
) -> Result<String, String> {
    println!("[Multi-page] Aggregating {} page results...", page_results.len());

    let combined_context = page_results
        .iter()
        .enumerate()
        .map(|(i, r)| format!("=== PAGE {} ANALYSIS ===\n{}", i + 1, r))
        .collect::<Vec<_>>()
        .join("\n\n");

    // We stop passing the original prompt here to prevent echo/duplication.
    // We only tell the AI to merge the JSON it already generated.
    let aggregate_prompt = format!(
        r#"You are a strict JSON data merger. I am providing you with the JSON outputs from multiple pages of a document.

CRITICAL RULES:
1. Combine all the page data into ONE single, flat, valid JSON object.
2. If multiple pages mention the same merchant name, only output it once.
3. Combine all items from the "positions" arrays into a single "positions" array.
4. DO NOT output the words "=== PAGE ANALYSIS ===". Strip out all page dividers.
5. DO NOT echo my prompts or ask follow-up questions.
6. Output ONLY valid JSON brackets. No markdown, no conversational text.

PAGE ANALYSES TO COMBINE:
{}

MERGED JSON ONLY:"#,
        combined_context
    );

    // For thinking models like Qwen3-VL, don't send format: "json" as it can conflict
    let format_json = if !model.to_lowercase().contains("qwen3") {
        Some("json".to_string())
    } else {
        None
    };

    let request = OllamaChatRequest {
        model: model.to_string(),
        messages: vec![OllamaMessage {
            role: "user".to_string(),
            content: aggregate_prompt,
            images: None, // No images needed for aggregation - text only
        }],
        stream: false,
        options: Some(OllamaOptions {
            temperature: Some(temperature),
            num_predict: Some(max_tokens),
            num_ctx: Some(16384), // 16K context for large multi-page aggregation
        }),
        format: format_json,
    };

    let response = client
        .post("http://localhost:11434/api/chat")
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("Aggregation request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!("Aggregation failed - status {}: {}", status, error_text));
    }

    let result: ollama::OllamaChatResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse aggregation response: {}", e))?;

    // Strip think tags from Qwen3-VL responses
    let cleaned_response = strip_think_tags(&result.message.content);

    Ok(cleaned_response)
}

/// Event-driven PDF analysis with live progress events
/// Emits analysis-progress after each page, analysis-complete when done
#[tauri::command]
async fn send_pdf_to_ollama(
    app: tauri::AppHandle,
    model: String,
    prompt: String,
    pdf_path: String,
    temperature: f32,
    max_tokens: i32,
) -> Result<String, String> {
    use std::path::Path;
    
    println!("[Event-Driven] Starting PDF analysis: {}", pdf_path);

    // Convert PDF to JPEG images on disk
    let conversion = convert_pdf_to_images(pdf_path.clone(), 72).await?;
    let total_pages = conversion.pages.len();

    println!("[Event-Driven] PDF has {} pages - images saved to disk", total_pages);

    // Emit start event
    let _ = app.emit("analysis-progress", serde_json::json!({
        "type": "start",
        "total_pages": total_pages,
        "message": format!("Starting analysis of {} pages...", total_pages)
    }));

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(900))
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // Process each page sequentially with event emission
    let mut page_results: Vec<String> = Vec::with_capacity(total_pages);
    let mut temp_files: Vec<String> = Vec::new();

    // Decode image paths from base64 - with proper error handling
    let image_paths: Vec<String> = conversion.images.iter()
        .enumerate()
        .filter_map(|(idx, b64)| {
            match BASE64.decode(b64) {
                Ok(bytes) => match String::from_utf8(bytes) {
                    Ok(path) => Some(path),
                    Err(e) => {
                        println!("[Event-Driven] Failed to decode path for page {}: {}", idx + 1, e);
                        None
                    }
                },
                Err(e) => {
                    println!("[Event-Driven] Failed to base64 decode path for page {}: {}", idx + 1, e);
                    None
                }
            }
        })
        .collect();

    println!("[Event-Driven] Successfully decoded {} image paths out of {}", image_paths.len(), conversion.images.len());

    if image_paths.len() != total_pages {
        return Err(format!("Failed to decode all image paths: {} of {}", image_paths.len(), total_pages));
    }

    for (idx, image_path) in image_paths.iter().enumerate() {
        let page_num = idx + 1;
        temp_files.push(image_path.clone());

        println!("[Event-Driven] Processing page {}/{}: {}", page_num, total_pages, image_path);

        // Check if file exists before trying to read
        if !std::path::Path::new(image_path).exists() {
            let err = format!("Image file not found: {}", image_path);
            println!("[Event-Driven] ERROR: {}", err);
            return Err(err);
        }

        // Emit page start event
        let _ = app.emit("analysis-progress", serde_json::json!({
            "type": "page_start",
            "current_page": page_num,
            "total_pages": total_pages,
            "message": format!("Analyzing page {} of {}...", page_num, total_pages)
        }));

        // Read image and encode to base64 for this page only
        let image_data = fs::read(image_path)
            .map_err(|e| format!("Failed to read image {}: {}", image_path, e))?;
        let base64_image = BASE64.encode(&image_data);

        println!("[Event-Driven] Page {} image loaded: {} bytes", page_num, image_data.len());

        // Analyze this page
        let page_result = analyze_single_page(
            &client,
            &model,
            &prompt,
            &base64_image,
            temperature,
            max_tokens,
            page_num,
            total_pages,
        ).await?;

        page_results.push(page_result.clone());

        println!("[Event-Driven] Page {} analysis complete: {} chars", page_num, page_result.len());

        // Emit page complete event WITH the result
        let _ = app.emit("analysis-progress", serde_json::json!({
            "type": "page_complete",
            "current_page": page_num,
            "total_pages": total_pages,
            "page_result": page_result,
            "message": format!("Page {} complete", page_num)
        }));

        // Delete this page's image file immediately to free disk space
        let _ = fs::remove_file(image_path);
        println!("[Event-Driven] Deleted temp file: {}", image_path);
    }

    println!("[Event-Driven] All {} pages analyzed, aggregating results...", total_pages);

    // Emit aggregation start event
    let _ = app.emit("analysis-progress", serde_json::json!({
        "type": "aggregating",
        "total_pages": total_pages,
        "message": "Combining all pages into final report..."
    }));

    // Aggregate all page results into final analysis
    let final_result = aggregate_page_results(
        &client,
        &model,
        &prompt,
        &page_results,
        temperature,
        max_tokens,
    ).await?;

    // Clean up any remaining temp files (including temp dir)
    for temp_file in &temp_files {
        if Path::new(temp_file).exists() {
            let _ = fs::remove_file(temp_file);
            println!("[Event-Driven] Cleaned up temp file: {}", temp_file);
        }
    }

    println!("[Event-Driven] Analysis complete! Final response length: {} chars", final_result.len());

    // Emit completion event
    let _ = app.emit("analysis-complete", serde_json::json!({
        "type": "complete",
        "result": final_result,
        "total_pages": total_pages,
        "message": "Analysis complete!"
    }));

    Ok(final_result)
}

/// Text-only chat with Ollama - no images, pure text-to-text
/// Used for follow-up questions after initial analysis
#[tauri::command]
async fn chat_with_ollama(
    model: String,
    prompt: String,
    temperature: f32,
    max_tokens: i32,
) -> Result<String, String> {
    println!("[Chat] Starting text-only chat with model: {}", model);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120)) // 2 minute timeout for text chat
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let request = OllamaChatRequest {
        model: model.clone(),
        messages: vec![OllamaMessage {
            role: "user".to_string(),
            content: prompt,
            images: None, // No images - text only!
        }],
        stream: false,
        options: Some(OllamaOptions {
            temperature: Some(temperature),
            num_predict: Some(max_tokens),
            num_ctx: Some(8192), // 8K context for chat responses
        }),
        format: None,
    };

    println!("[Chat] Sending request to Ollama...");

    let response = client
        .post("http://localhost:11434/api/chat")
        .json(&request)
        .send()
        .await
        .map_err(|e| {
            println!("[Chat] Request failed: {}", e);
            format!("Failed to send request: {}", e)
        })?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        println!("[Chat] Error response: {}", error_text);
        return Err(format!("Ollama returned status {}: {}", status, error_text));
    }

    let result: ollama::OllamaChatResponse = response
        .json()
        .await
        .map_err(|e| {
            println!("[Chat] Failed to parse JSON: {}", e);
            format!("Failed to parse response: {}", e)
        })?;

    // Strip think tags from qwen3 thinking models
    let cleaned_response = strip_think_tags(&result.message.content);

    println!("[Chat] Response received: {} chars", cleaned_response.len());

    Ok(cleaned_response)
}

/// Aggregate batch results from multiple PDF files into one master report
/// Reuses the same aggregation logic as multi-page PDF processing
#[tauri::command]
async fn aggregate_batch_results(
    model: String,
    original_prompt: String,
    page_results: Vec<String>,
    temperature: f32,
    max_tokens: i32,
) -> Result<String, String> {
    println!("[Batch] Aggregating {} PDF results...", page_results.len());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300)) // 5 minute timeout for aggregation
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let final_result = aggregate_page_results(
        &client,
        &model,
        &original_prompt,
        &page_results,
        temperature,
        max_tokens,
    ).await?;

    println!("[Batch] Aggregation complete! Final response length: {} chars", final_result.len());

    Ok(final_result)
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
            check_ollama_connection,
            get_ollama_models,
            send_to_ollama,
            read_file_as_base64,
            convert_pdf_to_images,
            cleanup_temp_images,
            cleanup_temp_files,
            send_pdf_to_ollama,
            chat_with_ollama,
            aggregate_batch_results,
            test_ollama_model,
            export_json,
            export_csv,
            get_templates,
            save_template,
            delete_template,
            get_history,
            save_history_entry,
            delete_history_entry,
            clear_all_history
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
