use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaModel {
    pub name: String,
    pub modified_at: Option<String>,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaModelsResponse {
    pub models: Vec<OllamaModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaMessage>,
    pub stream: bool,
    pub options: Option<OllamaOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>, // Set to "json" to enforce JSON output
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaOptions {
    pub temperature: Option<f32>,
    pub num_predict: Option<i32>,
    pub num_ctx: Option<i32>, // Context window size (default 2048, set to 16384 for large docs)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaChatResponse {
    pub model: String,
    pub message: OllamaMessage,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfPageInfo {
    pub page_number: usize,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfConversionResult {
    pub pages: Vec<PdfPageInfo>,
    pub images: Vec<String>, // base64 encoded paths for Ollama
    pub preview_path: Option<String>, // Absolute path to page1.jpg (for debugging)
    pub preview_image_data_uri: Option<String>, // Data URI for frontend preview (data:image/jpeg;base64,...)
}

/// Response with extracted thoughts (from thinking models like Qwen3-VL) and content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaResponse {
    pub thoughts: Option<String>, // Thinking process (if model is a thinking model)
    pub content: String,          // Actual response content (JSON for analysis)
}

/// Streaming chunk from Ollama API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaStreamChunk {
    pub model: String,
    pub message: Option<OllamaStreamMessage>,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaStreamMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>, // Thinking field for thinking models
}
