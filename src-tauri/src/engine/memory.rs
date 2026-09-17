//! Memory budget for the engine. On laptops the GPU has no memory of its own: model
//! weights and caches live in system RAM, and when that runs out the graphics driver
//! starts swapping GPU buffers and the desktop freezes (seen three times on a 14 GB
//! machine, `ttm_global_swapout` in the kernel log). So the engine sizes itself to the
//! memory that is actually free right now, refuses to start when there is not enough,
//! and a watchdog stops it if free memory collapses while it runs.
//!
//! Buffer sizes measured with llama.cpp b11002 (`-lv 5`):
//! GLM-OCR: 683 MiB text weights + 484 MiB projector, KV 64 KiB per context token,
//! about 1 GB of image encoder buffers per page in flight.
//! Qwen3.5-4B Q4_K_M: 2604 MiB weights, KV 32 KiB per context token (hybrid attention).

use super::registry::ModelSpec;
use serde::Serialize;

const GB: f64 = 1e9;
/// The watchdog stops the engine when available memory falls under this.
pub const WATCHDOG_FLOOR_BYTES: u64 = 1_200_000_000;
/// Compute buffers, output buffers, allocator slack per loaded model.
const COMPUTE_BYTES: u64 = 400_000_000;
/// Image encoder working set per OCR page in flight (measured: a 2-slot run peaked
/// 3.2 GB above idle with 1.4 GB of weights and 0.8 GB of KV).
const IMAGE_BYTES_PER_SLOT: u64 = 600_000_000;

/// Where the work runs. Chosen from free memory so the engine coexists with whatever
/// else is open instead of fighting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Weights, caches and the image encoder on the GPU. Fastest; needs the most headroom
    /// because the image encoder allocates about a gigabyte per page in flight.
    GpuFull,
    /// Text models on the GPU, image encoder on the CPU: the GPU allocation stays flat
    /// while pages are read, so the graphics driver never has to swap under a spike.
    GpuText,
    /// Everything on the CPU. Slow (minutes per scanned page) but ordinary pageable
    /// memory: the operating system can always reclaim it without stalling the desktop.
    Cpu,
}

/// Headroom the machine keeps for everything else, per mode. GPU allocations on a
/// shared-memory GPU are the dangerous kind, so they need more room.
const HEADROOM_GPU_FULL: u64 = 3_000_000_000;
const HEADROOM_GPU_TEXT: u64 = 2_000_000_000;
const HEADROOM_CPU: u64 = 1_500_000_000;

/// What the engine will run with on this machine right now.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryPlan {
    pub total_bytes: u64,
    pub available_bytes: u64,
    pub mode: Mode,
    /// CPU threads for the engine: all cores, at low process priority, so a job finishes
    /// as fast as the machine allows while the desktop still gets the CPU when it asks.
    pub threads: u32,
    /// Hard memory cap for the engine process tree (cgroup on Linux, job object on Windows).
    pub cap_bytes: u64,
    /// Models kept resident by the router at once: 1 swaps between OCR and reasoning.
    pub models_max: u8,
    pub ocr_ctx: u32,
    pub ocr_parallel: u32,
    pub underwriter_ctx: u32,
    /// Largest resident set under this plan (weights, caches, image buffers).
    pub peak_bytes: u64,
    /// `peak_bytes + HEADROOM_BYTES` fits in `available_bytes`.
    pub fits: bool,
    /// Human sentence for the setup screen and error messages.
    pub message: String,
}

pub fn available_bytes() -> (u64, u64) {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    // Testing aid: MCA_FAKE_AVAILABLE_BYTES pretends this much is free.
    let available = std::env::var("MCA_FAKE_AVAILABLE_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(sys.available_memory());
    (sys.total_memory(), available)
}

/// Machines with this much RAM or more may keep both models resident and run the full
/// OCR presets; below it the GPU shares system memory too tightly for that.
const ROOMY_TOTAL_BYTES: u64 = 24_000_000_000;

fn resident(model: &ModelSpec, ctx: u32, image_slots: u32) -> u64 {
    model.total_size() + model.kv_bytes_per_token * ctx as u64 + COMPUTE_BYTES + IMAGE_BYTES_PER_SLOT * image_slots as u64
}

/// Size the engine for the memory available now: the largest preset and the fastest
/// mode whose peak plus headroom fits. Only when even CPU mode does not fit does the
/// plan say so; that is a machine with under about 4 GB free, where nothing runs well.
pub fn plan(ocr: &ModelSpec, underwriter: &ModelSpec) -> MemoryPlan {
    let (total, available) = available_bytes();
    let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    // (ocr ctx, ocr parallel, underwriter ctx, models resident), largest first. The full
    // presets are only offered on roomy machines.
    let full: [(u32, u32, u32, u8); 2] = [
        (ocr.ctx_size, ocr.parallel, underwriter.ctx_size, 2),
        (ocr.ctx_size, ocr.parallel, underwriter.ctx_size, 1),
    ];
    let modest: [(u32, u32, u32, u8); 2] = [
        (12288.min(ocr.ctx_size), 2.min(ocr.parallel), 16384.min(underwriter.ctx_size), 1),
        (6144.min(ocr.ctx_size), 1, 12288.min(underwriter.ctx_size), 1),
    ];
    let candidates: Vec<(u32, u32, u32, u8)> = if total >= ROOMY_TOTAL_BYTES { full.iter().chain(modest.iter()).copied().collect() } else { modest.to_vec() };
    // Fastest mode first; within a mode the largest preset that fits.
    let mut chosen = None;
    'modes: for (mode, headroom) in [(Mode::GpuFull, HEADROOM_GPU_FULL), (Mode::GpuText, HEADROOM_GPU_TEXT), (Mode::Cpu, HEADROOM_CPU)] {
        for &(octx, opar, uctx, max) in &candidates {
            let ocr_bytes = resident(ocr, octx, opar);
            let uw_bytes = resident(underwriter, uctx, 0);
            let peak = if max == 2 { ocr_bytes + uw_bytes } else { ocr_bytes.max(uw_bytes) };
            if peak + headroom <= available {
                chosen = Some((mode, octx, opar, uctx, max, peak));
                break 'modes;
            }
        }
    }
    let (mode, octx, opar, uctx, max, peak, fits) = match chosen {
        Some((m, a, b, c, d, p)) => (m, a, b, c, d, p, true),
        None => {
            let (a, b, c, d) = candidates[candidates.len() - 1];
            let p = resident(ocr, a, b).max(resident(underwriter, c, 0));
            (Mode::Cpu, a, b, c, d, p, false)
        }
    };
    // The cap sits above the estimate so a normal run never trips it, and below the point
    // where the machine would start swapping.
    let cap = (peak + 1_000_000_000).min(available.saturating_sub(HEADROOM_CPU / 2)).max(peak);
    let mode_text = match mode {
        Mode::GpuFull => "GPU",
        Mode::GpuText => "GPU for text, image reading on the CPU",
        Mode::Cpu => "CPU only, slower",
    };
    let message = if fits {
        format!(
            "{:.1} GB free of {:.0} GB: {mode_text}, up to {:.1} GB, {} OCR page{} at a time, {threads} threads",
            available as f64 / GB, total as f64 / GB, peak as f64 / GB, opar, if opar == 1 { "" } else { "s" }
        )
    } else {
        format!(
            "Only {:.1} GB of {:.0} GB is free. The engine needs about {:.1} GB even on the CPU. Close other programs and try again.",
            available as f64 / GB, total as f64 / GB, (peak + HEADROOM_CPU) as f64 / GB
        )
    };
    MemoryPlan { total_bytes: total, available_bytes: available, mode, threads, cap_bytes: cap, models_max: max, ocr_ctx: octx, ocr_parallel: opar, underwriter_ctx: uctx, peak_bytes: peak, fits, message }
}

/// Before a job on a running engine: the models are already resident, so only the
/// per-job working set (image buffers, prompt) must still fit with headroom.
pub fn check_before_job(ocr_parallel: u32) -> Result<(), String> {
    let (total, available) = available_bytes();
    let need = IMAGE_BYTES_PER_SLOT * ocr_parallel as u64 + HEADROOM_CPU;
    if available < need {
        return Err(format!(
            "Not enough free memory to analyze safely: {:.1} GB free of {:.0} GB, about {:.1} GB is needed. Close other programs and try again.",
            available as f64 / GB, total as f64 / GB, need as f64 / GB
        ));
    }
    Ok(())
}
