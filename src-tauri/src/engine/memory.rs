//! Memory budget for the engine. On laptops the GPU has no memory of its own: model
//! weights and caches live in system RAM, and when that runs out the graphics driver
//! starts swapping GPU buffers and the desktop freezes (seen three times on a 14 GB
//! machine, `ttm_global_swapout` in the kernel log).
//!
//! The rule is dynamic, not capped: the engine runs at full speed on the GPU with as many
//! OCR slots as the free memory allows right now, and when memory gets tight it waits
//! between pages instead of piling on (see `wait_for_room`). The operating-system cage
//! and the watchdog are last resorts that never fire in normal operation, and a job that
//! does get interrupted resumes from the OCR cache instead of failing.
//!
//! Buffer sizes measured with llama.cpp b11002 (`-lv 5`):
//! GLM-OCR: 683 MiB text weights + 484 MiB projector, KV 64 KiB per context token at
//! f16 (the engine runs q8_0, about 34 KiB), about 0.6 GB of image encoder buffers per
//! page in flight. Qwen3.5-4B Q4_K_M: 2604 MiB weights, KV 32 KiB per token at f16
//! (hybrid attention).
//!
//! Measured on the 680M, six dense scanned pages: 4 slots 240 s, 2 slots 290 s, 4 slots
//! with q8_0 KV 210 s (adopted). 125 DPI would be 136 s but lost rows on two of six
//! statements, so pages stay at 150 DPI.

use super::registry::ModelSpec;
use serde::Serialize;
use std::time::{Duration, Instant};

const GB: f64 = 1e9;
/// Compute buffers, output buffers, allocator slack per loaded model.
const COMPUTE_BYTES: u64 = 400_000_000;
/// Image encoder working set per OCR page in flight.
pub const IMAGE_BYTES_PER_SLOT: u64 = 600_000_000;
/// Room left for everything else when the engine is at its planned peak. The watchdog
/// below is the real net, so this only has to cover the estimate's error.
const HEADROOM_BYTES: u64 = 2_000_000_000;
/// The pipeline does not start another OCR page while less than this is free; it waits.
pub const SOFT_FLOOR_BYTES: u64 = 2_000_000_000;
/// Last resort: the watchdog stops the engine when available memory falls under this.
/// Above zram/swap kicking in, below anything a normal job reaches.
pub const WATCHDOG_FLOOR_BYTES: u64 = 1_000_000_000;

/// What the engine will run with on this machine right now.
#[derive(Debug, Clone, Serialize)]
pub struct MemoryPlan {
    pub total_bytes: u64,
    pub available_bytes: u64,
    /// CPU threads: all cores. The engine gets the machine while it works, like any
    /// other heavy program; the desktop is protected by memory limits, not by starving it.
    pub threads: u32,
    /// Soft cgroup limit (throttle by reclaim) and hard limit (kill) for the engine tree.
    pub high_bytes: u64,
    pub max_bytes: u64,
    /// Models kept resident by the router at once: 1 swaps between OCR and reasoning,
    /// which the pipeline needs only once per job.
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

fn resident(model: &ModelSpec, ctx: u32, image_slots: u32) -> u64 {
    model.total_size() + model.kv_bytes_per_token * ctx as u64 + COMPUTE_BYTES + IMAGE_BYTES_PER_SLOT * image_slots as u64
}

/// Size the engine for the memory available now: the most OCR slots that fit, both
/// models resident when the machine is roomy enough to skip the swap. Only when even the
/// smallest setup does not fit does the plan say so. `underwriter` is None for jobs that
/// only read pages (the headless ledger dump): then only the OCR model has to fit.
pub fn plan(ocr: &ModelSpec, underwriter: Option<&ModelSpec>) -> MemoryPlan {
    let (total, available) = available_bytes();
    let threads = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(4);
    let uctx = underwriter.map(|u| u.ctx_size).unwrap_or(0);
    // (ocr slots, models resident), fastest first. Slot context is fixed per page:
    // a page image is about 2,700 tokens plus up to 1,500 of text.
    let per_slot_ctx = ocr.ctx_size / ocr.parallel.max(1);
    let candidates: Vec<(u32, u8)> = vec![(ocr.parallel, 2), (ocr.parallel, 1), (2.min(ocr.parallel), 1), (1, 1)];
    let mut chosen = None;
    for &(slots, max) in &candidates {
        let octx = per_slot_ctx * slots;
        let ocr_bytes = resident(ocr, octx, slots);
        let uw_bytes = underwriter.map(|u| resident(u, uctx, 0)).unwrap_or(0);
        let peak = if max == 2 { ocr_bytes + uw_bytes } else { ocr_bytes.max(uw_bytes) };
        if peak + HEADROOM_BYTES <= available {
            chosen = Some((slots, octx, max, peak));
            break;
        }
    }
    let (slots, octx, max, peak, fits) = match chosen {
        Some((s, c, m, p)) => (s, c, m, p, true),
        None => {
            let (s, m) = candidates[candidates.len() - 1];
            let c = per_slot_ctx * s;
            (s, c, m, resident(ocr, c, s).max(underwriter.map(|u| resident(u, uctx, 0)).unwrap_or(0)), false)
        }
    };
    // The soft limit throttles at the estimate; the hard limit sits well above so a
    // normal run never trips it.
    let high = peak + 500_000_000;
    let max_bytes = peak + 1_500_000_000;
    let message = if fits {
        format!(
            "{:.1} GB free of {:.0} GB: GPU, {} OCR page{} at a time, {} model{} resident, up to {:.1} GB",
            available as f64 / GB, total as f64 / GB, slots, if slots == 1 { "" } else { "s" }, max, if max == 1 { "" } else { "s" }, peak as f64 / GB
        )
    } else {
        format!(
            "Only {:.1} GB of {:.0} GB is free; the engine needs about {:.1} GB. Close other programs and try again.",
            available as f64 / GB, total as f64 / GB, (peak + HEADROOM_BYTES) as f64 / GB
        )
    };
    MemoryPlan { total_bytes: total, available_bytes: available, threads, high_bytes: high, max_bytes, models_max: max, ocr_ctx: octx, ocr_parallel: slots, underwriter_ctx: uctx, peak_bytes: peak, fits, message }
}

/// Wait until at least `SOFT_FLOOR_BYTES` are free, polling every half second, for up
/// to `max_wait`. Returns how long it waited; Err when the wait ran out. The pipeline
/// calls this before every OCR page so a busy machine slows the job down instead of
/// the job pushing the machine over the edge.
pub async fn wait_for_room(max_wait: Duration, mut on_wait: impl FnMut(u64)) -> Result<Duration, String> {
    let started = Instant::now();
    loop {
        let (_, available) = available_bytes();
        if available >= SOFT_FLOOR_BYTES {
            return Ok(started.elapsed());
        }
        if started.elapsed() > max_wait {
            return Err(format!(
                "Waited {:.0} s for memory: only {:.1} GB free, {:.1} GB is needed to read another page. Close other programs and run again; pages already read are kept.",
                max_wait.as_secs_f32(), available as f64 / GB, SOFT_FLOOR_BYTES as f64 / GB
            ));
        }
        on_wait(available);
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
