// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ollama;

use ollama::{OllamaChatRequest, OllamaMessage, OllamaOptions, OllamaModelsResponse, PdfConversionResult, PdfPageInfo};
use std::fs;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::GenericImageView;
use tauri_plugin_dialog::DialogExt;

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
        model,
        messages: vec![OllamaMessage {
            role: "user".to_string(),
            content: prompt,
            images,
        }],
        stream: false,
        options: Some(OllamaOptions {
            temperature: Some(temperature),
            num_predict: Some(max_tokens),
        }),
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

    Ok(result.message.content)
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
    
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let request = OllamaChatRequest {
        model,
        messages: vec![OllamaMessage {
            role: "user".to_string(),
            content: "Say hello in one word".to_string(),
            images: None,
        }],
        stream: false,
        options: Some(OllamaOptions {
            temperature: Some(0.1),
            num_predict: Some(10),
        }),
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

    println!("[Test] Success: {}", result.message.content);
    Ok(result.message.content)
}

/// Convert PDF to temporary JPEG images and return Base64 strings
/// Uses JPEG compression to reduce payload size (~30KB per page vs ~130KB for PNG)
#[tauri::command]
async fn convert_pdf_to_images(pdf_path: String, dpi: u32) -> Result<PdfConversionResult, String> {
    use std::process::Command;
    use tempfile::tempdir;
    use image::ImageFormat;
    
    println!("[PDF] Converting PDF to JPEG images at {} DPI...", dpi);
    
    // Create temp directory for page images
    let temp_dir = tempdir()
        .map_err(|e| format!("Failed to create temp dir: {}", e))?;
    
    // Try pdftocairo first (part of poppler-utils)
    let output_prefix = temp_dir.path().join("page");
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
            let mut images = Vec::new();
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
                    
                    // Convert to JPEG with 80% quality for compression
                    let mut jpeg_data: Vec<u8> = Vec::new();
                    grayscale_img.write_to(
                        &mut std::io::Cursor::new(&mut jpeg_data),
                        ImageFormat::Jpeg,
                    )
                    .map_err(|e| format!("Failed to compress page {} to JPEG: {}", page_num, e))?;
                    
                    // Encode JPEG to Base64
                    let base64_image = BASE64.encode(&jpeg_data);
                    
                    let compression_pct = if png_data.len() > jpeg_data.len() {
                        ((png_data.len() - jpeg_data.len()) as u64 * 100 / png_data.len() as u64) as usize
                    } else {
                        0
                    };
                    
                    println!("[PDF] Page {}: PNG {}KB → Grayscale JPEG {}KB (compressed {}%)", 
                        page_num,
                        png_data.len() / 1024,
                        jpeg_data.len() / 1024,
                        compression_pct
                    );
                    
                    pages.push(PdfPageInfo { page_number: page_num, width, height });
                    images.push(base64_image);
                    
                    // Clean up temp PNG
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
            
            Ok(PdfConversionResult { pages, images })
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
        }),
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

    Ok(result.message.content)
}

/// Aggregate results from multiple pages into final analysis
/// Generic aggregator - works with any document type, not just MCA
async fn aggregate_page_results(
    client: &reqwest::Client,
    model: &str,
    original_prompt: &str,
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

    // Generic aggregation prompt - defers to original prompt for formatting rules
    let aggregate_prompt = format!(
        r#"You have analyzed a multi-page document. Below are the individual page analyses.

YOUR TASK:
Combine these page analyses into one cohesive final response.

IMPORTANT:
- Merge all findings from all pages into a single comprehensive analysis
- You MUST strictly adhere to the formatting and rules requested in the original prompt
- If the original prompt requested JSON, return ONLY valid JSON
- Combine numerical values (sums, counts, etc.) appropriately across pages
- Do not introduce any new information - only synthesize what was extracted

ORIGINAL PROMPT (follow these rules and format):
{}

PAGE ANALYSES TO COMBINE:
{}

FINAL COMBINED RESPONSE (follow original prompt format exactly):"#,
        original_prompt,
        combined_context
    );

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
        }),
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

    Ok(result.message.content)
}

#[tauri::command]
async fn send_pdf_to_ollama(
    model: String,
    prompt: String,
    pdf_path: String,
    temperature: f32,
    max_tokens: i32,
) -> Result<String, String> {
    // Convert PDF to JPEG images (compressed Base64)
    let conversion = convert_pdf_to_images(pdf_path.clone(), 72).await?;
    let total_pages = conversion.images.len();

    println!("[Multi-page] PDF has {} pages - processing all pages sequentially", total_pages);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(900)) // 15 minute total timeout for multi-page
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // Process each page sequentially
    let mut page_results: Vec<String> = Vec::with_capacity(total_pages);

    for (idx, base64_image) in conversion.images.iter().enumerate() {
        let page_num = idx + 1;
        
        // Per-page timeout: 5 minutes per page
        let page_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .build()
            .map_err(|e| format!("Failed to create page client: {}", e))?;

        let page_result = analyze_single_page(
            &page_client,
            &model,
            &prompt,
            base64_image,
            temperature,
            max_tokens,
            page_num,
            total_pages,
        ).await?;

        page_results.push(page_result);
        println!("[Multi-page] Page {}/{} complete", page_num, total_pages);
    }

    println!("[Multi-page] All {} pages analyzed, aggregating results...", total_pages);

    // Aggregate all page results into final analysis
    let final_result = aggregate_page_results(
        &client,
        &model,
        &prompt,
        &page_results,
        temperature,
        max_tokens,
    ).await?;

    println!("[Multi-page] Analysis complete! Final response length: {} chars", final_result.len());

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
        }),
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

    println!("[Chat] Response received: {} chars", result.message.content.len());

    Ok(result.message.content)
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
            send_pdf_to_ollama,
            chat_with_ollama,
            test_ollama_model,
            export_json,
            export_csv
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
