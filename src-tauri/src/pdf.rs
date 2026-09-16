//! Shapes returned by the PDF preview conversion command.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfPageInfo {
    pub page_number: usize,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PdfConversionResult {
    pub pages: Vec<PdfPageInfo>,
    /// Page image paths, base64-encoded (legacy transport shape kept for the frontend).
    pub images: Vec<String>,
    /// Absolute path to the first page JPEG.
    pub preview_path: Option<String>,
    /// `data:image/jpeg;base64,...` for the preview pane.
    pub preview_image_data_uri: Option<String>,
}
