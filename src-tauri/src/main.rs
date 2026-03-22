// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ollama;

use ollama::{OllamaChatRequest, OllamaMessage, OllamaOptions, OllamaModelsResponse, PdfConversionResult, PdfPageInfo};
use std::fs;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use image::GenericImageView;

#[tauri::command]
async fn check_ollama_connection() -> Result<bool, String> {
    let client = reqwest::Client::new();
    match client.get("http://localhost:11434/api/tags").send().await {
        Ok(response) => Ok(response.status().is_success()),
        Err(_) => Ok(false),
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
async fn convert_pdf_to_images(pdf_path: String, dpi: u32) -> Result<PdfConversionResult, String> {
    use std::process::Command;
    use tempfile::tempdir;
    
    // Create temp directory for page images
    let temp_dir = tempdir()
        .map_err(|e| format!("Failed to create temp dir: {}", e))?;
    
    // Try pdftocairo first (part of poppler-utils)
    let output_prefix = temp_dir.path().join("page");
    let output_pattern = output_prefix.to_str().unwrap();
    
    let result = Command::new("pdftocairo")
        .args([
            "-png",
            "-r", &dpi.to_string(),
            &pdf_path,
            output_pattern
        ])
        .output();
    
    match result {
        Ok(output) if output.status.success() => {
            // Find all generated PNG files
            let mut pages = Vec::new();
            let mut images = Vec::new();
            let mut page_num = 1;
            
            loop {
                let png_path = format!("{}-{}.png", output_pattern, page_num);
                if std::path::Path::new(&png_path).exists() {
                    let png_data = fs::read(&png_path)
                        .map_err(|e| format!("Failed to read page {}: {}", page_num, e))?;
                    
                    // Get image dimensions
                    let img = image::load_from_memory(&png_data)
                        .map_err(|e| format!("Failed to decode image: {}", e))?;
                    let (width, height) = img.dimensions();
                    
                    let base64_image = BASE64.encode(&png_data);
                    
                    pages.push(PdfPageInfo { page_number: page_num, width, height });
                    images.push(base64_image);
                    
                    // Clean up temp file
                    let _ = fs::remove_file(&png_path);
                    
                    page_num += 1;
                } else {
                    break;
                }
            }
            
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

#[tauri::command]
async fn send_pdf_to_ollama(
    model: String,
    prompt: String,
    pdf_path: String,
    temperature: f32,
    max_tokens: i32,
) -> Result<String, String> {
    // First convert PDF to images
    let conversion = convert_pdf_to_images(pdf_path.clone(), 150).await?;
    
    let client = reqwest::Client::new();
    
    // Build multi-page prompt
    let mut full_prompt = prompt.clone();
    if conversion.pages.len() > 1 {
        full_prompt = format!("This is a {}-page document. Analyze all pages.\n\n{}", 
            conversion.pages.len(), prompt);
    }
    
    let request = OllamaChatRequest {
        model,
        messages: vec![OllamaMessage {
            role: "user".to_string(),
            content: full_prompt,
            images: Some(conversion.images),
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
            send_pdf_to_ollama
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
