//! Pinned inventory of everything the built-in engine downloads at first run.
//!
//! Nothing here is bundled with the installer. The app fetches these files into
//! the OS app-data directory, verifies them, and runs llama-server from there.
//! Swapping a model is a one-entry change in this file plus a re-test.

use serde::{Deserialize, Serialize};

/// llama.cpp release tag. Every runtime asset below comes from this one release.
pub const LLAMA_RELEASE_TAG: &str = "b11002";

/// Compute backend for the llama-server binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Vulkan on Windows/Linux (AMD, Intel, NVIDIA including integrated GPUs). Metal on macOS.
    Gpu,
    /// Plain CPU build. Fallback when the GPU build fails to start.
    Cpu,
}

/// One downloadable file.
#[derive(Debug, Clone, Serialize)]
pub struct Asset {
    /// Stable id used in progress events and on disk.
    pub id: &'static str,
    pub url: String,
    /// File name on disk (inside the component's directory).
    pub file_name: String,
    /// Exact byte size published by the source. Used for progress and a cheap integrity check.
    pub size: u64,
    /// sha256 hex when the source publishes one (Hugging Face LFS does, GitHub releases do not).
    pub sha256: Option<&'static str>,
}

/// Which llama.cpp archive fits this OS/arch/backend, or None if unsupported.
pub fn runtime_asset(backend: Backend) -> Option<Asset> {
    let tag = LLAMA_RELEASE_TAG;
    let (name, size): (String, u64) = match (std::env::consts::OS, std::env::consts::ARCH, backend) {
        ("windows", "x86_64", Backend::Gpu) => (format!("llama-{tag}-bin-win-vulkan-x64.zip"), 31_706_629),
        ("windows", "x86_64", Backend::Cpu) => (format!("llama-{tag}-bin-win-cpu-x64.zip"), 18_434_892),
        ("windows", "aarch64", _) => (format!("llama-{tag}-bin-win-cpu-arm64.zip"), 11_997_338),
        // macOS builds ship with Metal; there is no separate CPU archive.
        ("macos", "aarch64", _) => (format!("llama-{tag}-bin-macos-arm64.tar.gz"), 11_153_837),
        ("macos", "x86_64", _) => (format!("llama-{tag}-bin-macos-x64.tar.gz"), 11_201_241),
        ("linux", "x86_64", Backend::Gpu) => (format!("llama-{tag}-bin-ubuntu-vulkan-x64.tar.gz"), 30_216_503),
        ("linux", "x86_64", Backend::Cpu) => (format!("llama-{tag}-bin-ubuntu-x64.tar.gz"), 16_852_047),
        ("linux", "aarch64", Backend::Gpu) => (format!("llama-{tag}-bin-ubuntu-vulkan-arm64.tar.gz"), 24_243_197),
        ("linux", "aarch64", Backend::Cpu) => (format!("llama-{tag}-bin-ubuntu-arm64.tar.gz"), 13_474_245),
        _ => return None,
    };
    Some(Asset {
        id: "runtime",
        url: format!("https://github.com/ggml-org/llama.cpp/releases/download/{tag}/{name}"),
        file_name: name,
        // Exact sizes from the GitHub release API for this tag.
        size,
        sha256: None,
    })
}

/// A model the engine knows how to use. `files` are all downloaded into one directory.
#[derive(Debug, Clone, Serialize)]
pub struct ModelSpec {
    /// Preset name passed to llama-server and used in requests: "ocr" or "underwriter".
    pub role: &'static str,
    /// Stable id, also the on-disk directory name.
    pub id: &'static str,
    pub display_name: &'static str,
    pub hf_repo: &'static str,
    pub files: Vec<Asset>,
    /// Rough RAM the machine should have for this model to be a sensible default.
    pub min_ram_gb: u32,
    /// Context size handed to llama-server for this model.
    pub ctx_size: u32,
}

impl ModelSpec {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
    /// The main GGUF (first file). Multimodal models also carry an mmproj file.
    pub fn main_file(&self) -> &Asset {
        &self.files[0]
    }
    pub fn mmproj_file(&self) -> Option<&Asset> {
        self.files.iter().find(|f| f.file_name.starts_with("mmproj"))
    }
}

fn hf(repo: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/main/{file}")
}

/// The OCR model. There is exactly one; it reads a page image and returns its text.
pub fn ocr_model() -> ModelSpec {
    let repo = "ggml-org/GLM-OCR-GGUF";
    ModelSpec {
        role: "ocr",
        id: "glm-ocr-q8",
        display_name: "GLM-OCR (0.9B, Q8_0)",
        hf_repo: repo,
        files: vec![
            Asset {
                id: "glm-ocr-q8/model",
                url: hf(repo, "GLM-OCR-Q8_0.gguf"),
                file_name: "GLM-OCR-Q8_0.gguf".into(),
                size: 950_433_408,
                sha256: Some("45bc244a6446aff850521dc41f18bc8d7105ad5f0c2c8c28af04e7cc4f4d50b1"),
            },
            Asset {
                id: "glm-ocr-q8/mmproj",
                url: hf(repo, "mmproj-GLM-OCR-Q8_0.gguf"),
                file_name: "mmproj-GLM-OCR-Q8_0.gguf".into(),
                size: 484_403_648,
                sha256: Some("9c4b58e33e316ed142eb5dcb41abec3844d3e6e5dc361ffb782c3fa9d175141f"),
            },
        ],
        min_ram_gb: 8,
        ctx_size: 8192,
    }
}

/// Reasoning model choices. The first entry is the default; the UI offers the rest
/// when the machine has enough RAM.
pub fn underwriter_models() -> Vec<ModelSpec> {
    vec![
        ModelSpec {
            role: "underwriter",
            id: "qwen3.5-4b-q4",
            display_name: "Qwen3.5 4B (Q4_K_M)",
            hf_repo: "unsloth/Qwen3.5-4B-GGUF",
            files: vec![Asset {
                id: "qwen3.5-4b-q4/model",
                url: hf("unsloth/Qwen3.5-4B-GGUF", "Qwen3.5-4B-Q4_K_M.gguf"),
                file_name: "Qwen3.5-4B-Q4_K_M.gguf".into(),
                size: 2_740_937_888,
                sha256: Some("00fe7986ff5f6b463e62455821146049db6f9313603938a70800d1fb69ef11a4"),
            }],
            min_ram_gb: 8,
            ctx_size: 32768,
        },
        ModelSpec {
            role: "underwriter",
            id: "qwen3.5-9b-q4",
            display_name: "Qwen3.5 9B (Q4_K_M)",
            hf_repo: "unsloth/Qwen3.5-9B-GGUF",
            files: vec![Asset {
                id: "qwen3.5-9b-q4/model",
                url: hf("unsloth/Qwen3.5-9B-GGUF", "Qwen3.5-9B-Q4_K_M.gguf"),
                file_name: "Qwen3.5-9B-Q4_K_M.gguf".into(),
                size: 5_680_522_464,
                sha256: Some("03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8"),
            }],
            min_ram_gb: 16,
            ctx_size: 16384,
        },
    ]
}

pub fn underwriter_model(id: &str) -> Option<ModelSpec> {
    underwriter_models().into_iter().find(|m| m.id == id)
}
