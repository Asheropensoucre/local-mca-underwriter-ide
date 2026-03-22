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
    
    println!("[Ollama] Converting {} pages, sending to model...", conversion.pages.len());
    println!("[Ollama] Total images: {}", conversion.images.len());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600)) // 10 minute total timeout
        .connect_timeout(std::time::Duration::from_secs(30)) // 30 second connect timeout
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    // For vision models, send only the FIRST page for now (debugging)
    // TODO: Implement proper multi-image support
    let images_to_send = if conversion.images.len() > 1 {
        println!("[Ollama] Sending only first page (multi-image not stable yet)");
        vec![conversion.images[0].clone()]
    } else {
        conversion.images.clone()
    };

    // Build prompt
    let full_prompt = if conversion.images.len() > 1 {
        format!("This is a {}-page document. Analyze the first page shown.\n\n{}", 
            conversion.images.len(), prompt)
    } else {
        prompt.clone()
    };

    let request = OllamaChatRequest {
        model: model.clone(),
        messages: vec![OllamaMessage {
            role: "user".to_string(),
            content: full_prompt.clone(),
            images: Some(images_to_send),
        }],
        stream: false,
        options: Some(OllamaOptions {
            temperature: Some(temperature),
            num_predict: Some(max_tokens),
        }),
    };
    
    println!("[Ollama] Sending request to http://localhost:11434/api/chat...");
    println!("[Ollama] Model: {}, Prompt length: {} chars, Images: {}", 
        model, full_prompt.len(), request.messages[0].images.as_ref().map(|i| i.len()).unwrap_or(0));

    let response = client
        .post("http://localhost:11434/api/chat")
        .json(&request)
        .send()
        .await;
    
    println!("[Ollama] Request sent, waiting for response...");
    
    match response {
        Ok(resp) => {
            let status = resp.status();
            println!("[Ollama] Got HTTP response, status: {}", status);
            
            // Check if response is an error
            if !status.is_success() {
                let error_text = resp.text().await.unwrap_or_default();
                println!("[Ollama] Error response: {}", error_text);
                return Err(format!("Ollama returned status {}: {}", status, error_text));
            }
            
            let result: ollama::OllamaChatResponse = resp
                .json()
                .await
                .map_err(|e| {
                    println!("[Ollama] Failed to parse JSON: {}", e);
                    format!("Failed to parse response: {}", e)
                })?;
            
            println!("[Ollama] Done! Response length: {} chars", result.message.content.len());

            Ok(result.message.content)
        }
        Err(e) => {
            println!("[Ollama] Request failed: {}", e);
            println!("[Ollama] Error type: {:?}", e);
            
            // Provide specific error messages
            let error_msg = if e.is_timeout() {
                "Request timed out. Ollama is taking too long to process the image.".to_string()
            } else if e.is_connect() {
                "Could not connect to Ollama. Is 'ollama serve' running?".to_string()
            } else if e.is_request() {
                format!("Request error: {}. The model may have crashed or rejected the request.", e)
            } else {
                format!("HTTP error: {}. Check Ollama logs for details.", e)
            };
            
            Err(error_msg)
        }
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
            test_ollama_model
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
