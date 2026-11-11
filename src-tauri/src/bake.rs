//! The background bake: fill the cache before the user flies into a region.
//!
//! # What gets baked, and what does not
//!
//! **By reachability, never by enumeration.** A naive full bake is tens of
//! gigabytes — 61,430 `.yft`, 88,868 dictionaries, 335,729 textures — and most
//! of it is peds, vehicles, weapons, cutscenes and unplaced interiors that the
//! world view will never draw. So the only assets considered are the ones a
//! `.ymap` entity's archetype actually names, which falls out of walking the
//! maps rather than the archives.
//!
//! **Ordered by placement frequency, descending.** A model placed 800 times is
//! worth more baked than one placed once, and the histogram comes free while
//! reading the maps. That ordering is what makes a bake cancelled halfway still
//! worth having.
//!
//! **Bounded by the cache's own budget.** At the limit the pass stops
//! enqueueing rather than evicting: a bake that evicted its own earlier work
//! would run forever and finish with a cache full of whatever it did last.
//!
//! # Why it cannot starve the streamer
//!
//! Its rayon pool is sized `(cpus - 1).max(1)`, leaving a core for the model
//! the user is waiting on. Progress is coalesced to 4 Hz, because one event per
//! item at this scale would drown the WebView in IPC and cost more than the
//! bake.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cw_core::filetypes::MloArchetype;
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::State;

use crate::explorer::{GameState, Loaded};
use crate::world::{ymaps_in_group, GpuState};

/// How often progress reaches the WebView, at most.
const TICK: Duration = Duration::from_millis(250);

/// Where the pass is. `assets` is the phase that costs anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Reading `.ymap`s to find what is placed and how often.
    Maps,
    /// Compiling models into the cache.
    Assets,
    Done,
    Cancelled,
    /// The cache filled up. Everything baked so far is kept.
    Full,
}

/// One progress report. Cheap to serialize, because it is sent four times a
/// second for as long as the bake runs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BakeProgress {
    pub phase: Phase,
    pub done: usize,
    pub total: usize,
    /// What the cache holds now, so the UI can draw it against the budget
    /// without a second command.
    pub bytes: u64,
    pub budget_bytes: u64,
    /// Items that failed to compile. A bad model does not stop the pass.
    pub errors: usize,
    pub elapsed_ms: u64,
}

/// Whether a bake is running, and how to stop it.
///
/// No join handle: the pass holds an `Arc<Loaded>`, so the install and its cache
/// outlive any folder change, and a cancelled bake winding down in the
/// background cannot write to a cache anyone else is using.
#[derive(Debug, Default)]
pub struct BakeState {
    running: AtomicBool,
    cancel: AtomicBool,
}

impl BakeState {
    /// Ask a running bake to stop. Safe to call when none is.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    /// Claim the right to run, or report that someone already has it.
    fn claim(&self) -> bool {
        if self.running.swap(true, Ordering::SeqCst) {
            return false;
        }
        self.cancel.store(false, Ordering::Relaxed);
        true
    }

    fn release(&self) {
        self.running.store(false, Ordering::Relaxed);
    }

    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// Coalesces progress to [`TICK`], so the WebView sees four events a second
/// however many threads are reporting.
struct Reporter {
    channel: Channel<BakeProgress>,
    started: Instant,
    last: Mutex<Instant>,
    done: AtomicUsize,
    errors: AtomicUsize,
}

impl Reporter {
    fn new(channel: Channel<BakeProgress>) -> Self {
        let started = Instant::now();
        Self {
            channel,
            started,
            // Back-dated so the first item reports immediately, rather than the
            // UI sitting blank for a quarter second.
            last: Mutex::new(started - TICK),
            done: AtomicUsize::new(0),
            errors: AtomicUsize::new(0),
        }
    }

    fn step(&self, failed: bool) -> usize {
        if failed {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
        self.done.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Send unless one went out within [`TICK`]. `force` is for phase changes
    /// and for the final report, which must never be dropped.
    fn emit(&self, phase: Phase, total: usize, bytes: u64, budget: u64, force: bool) {
        {
            let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
            if !force && last.elapsed() < TICK {
                return;
            }
            *last = Instant::now();
        }
        let _ = self
            .channel
            .send(self.snapshot(phase, total, bytes, budget));
    }

    fn snapshot(&self, phase: Phase, total: usize, bytes: u64, budget: u64) -> BakeProgress {
        BakeProgress {
            phase,
            done: self.done.load(Ordering::Relaxed),
            total,
            bytes,
            budget_bytes: budget,
            errors: self.errors.load(Ordering::Relaxed),
            elapsed_ms: self.started.elapsed().as_millis() as u64,
        }
    }
}

/// Every model placed by the given content groups, with how often, most placed
/// first.
///
/// This is the reachability pass, and what it does not name is everything the
/// bake will not touch. Pure of Tauri state and of [`BakeState`] so the gate can
/// drive it against a real scan; see `tests/bake_gate.rs`.
pub fn reachable_models(
    manager: &RpfManager,
    keys: &GtaKeys,
    mlo: &HashMap<u32, MloArchetype>,
    names: &HashMap<u32, String>,
    groups: &[String],
    cancelled: &dyn Fn() -> bool,
) -> Vec<(String, usize)> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    for group in groups {
        for (path, _) in ymaps_in_group(manager, group) {
            if cancelled() {
                break;
            }
            let Ok(map) =
                crate::world::read_world_map(&path, keys, manager, Some(mlo), Some(names))
            else {
                continue;
            };
            for entity in &map.entities {
                if let Some(model) = crate::world::find_model_for(manager, &entity.name) {
                    *counts.entry(model).or_default() += 1;
                }
            }
        }
    }

    let mut models: Vec<(String, usize)> = counts.into_iter().collect();
    // Descending by placement count, then by path so a bake is reproducible
    // rather than following hash order.
    models.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    models
}

/// Compile every model into the cache, in order, until done or told to stop.
fn bake_models(
    loaded: &Loaded,
    models: &[String],
    caps: crate::textures::GpuCaps,
    state: &BakeState,
    reporter: &Reporter,
) -> Phase {
    let Some(cache) = &loaded.cache else {
        return Phase::Done;
    };
    let budget = cache.stats().budget_bytes;
    let total = models.len();
    let full = AtomicBool::new(false);

    // One core left for the model the user is waiting on. A bake that makes the
    // app it is speeding up feel slower has failed at its job.
    let threads = std::thread::available_parallelism()
        .map(|n| (n.get() - 1).max(1))
        .unwrap_or(1);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build()
        .ok();

    let run = || {
        use rayon::prelude::*;
        models.par_iter().for_each(|path| {
            if state.cancelled() || full.load(Ordering::Relaxed) {
                return;
            }
            // Checked per item rather than predicted: the only honest measure of
            // how much a bake costs is how much it has already stored.
            if cache.stats().bytes >= budget {
                full.store(true, Ordering::Relaxed);
                return;
            }

            let key = crate::cache::model_key(&loaded.manager, path, caps);
            let failed = cache
                .get_or_insert_with(key, cw_cache::Kind::Model, || {
                    crate::world::compile_world_model(
                        path,
                        &loaded.keys,
                        &loaded.manager,
                        Some(loaded.archetypes()),
                        caps,
                    )
                })
                .is_err();
            reporter.step(failed);
            reporter.emit(Phase::Assets, total, cache.stats().bytes, budget, false);
        });
    };
    match &pool {
        Some(pool) => pool.install(run),
        None => run(),
    }

    // Fold the journal in now rather than leaving a bake's worth of it for the
    // next launch to replay.
    let _ = cache.compact();

    if state.cancelled() {
        Phase::Cancelled
    } else if full.load(Ordering::Relaxed) {
        Phase::Full
    } else {
        Phase::Done
    }
}

/// Bake the given content groups in the background.
///
/// Returns the final report; the channel carries everything before it. Awaiting
/// this is optional — the UI can drive entirely off the channel and use
/// [`bake_cancel`] to stop.
#[tauri::command]
pub async fn bake_start(
    groups: Vec<String>,
    on: Channel<BakeProgress>,
    game: State<'_, GameState>,
    gpu: State<'_, GpuState>,
    bake: State<'_, Arc<BakeState>>,
) -> Result<BakeProgress, String> {
    let loaded = game.loaded()?;
    if loaded.cache.is_none() {
        return Err("the asset cache is not available".into());
    }
    let caps = gpu.get();
    let state = Arc::clone(&bake);
    if !state.claim() {
        return Err("a bake is already running".into());
    }

    let result = crate::world::blocking(move || {
        let reporter = Reporter::new(on);
        let budget = loaded
            .cache
            .as_ref()
            .map_or(0, |cache| cache.stats().budget_bytes);

        reporter.emit(Phase::Maps, 0, 0, budget, true);
        let models: Vec<String> = reachable_models(
            &loaded.manager,
            &loaded.keys,
            loaded.mlo_archetypes(),
            loaded.names(),
            &groups,
            &|| state.cancelled(),
        )
        .into_iter()
        .map(|(path, _)| path)
        .collect();

        let phase = if state.cancelled() {
            Phase::Cancelled
        } else {
            reporter.emit(Phase::Assets, models.len(), 0, budget, true);
            bake_models(&loaded, &models, caps, &state, &reporter)
        };

        let bytes = loaded.cache.as_ref().map_or(0, |cache| cache.stats().bytes);
        state.release();
        let final_report = reporter.snapshot(phase, models.len(), bytes, budget);
        reporter.emit(phase, models.len(), bytes, budget, true);
        Ok(final_report)
    })
    .await;

    // A panic in the pass must not leave the state claimed forever.
    if result.is_err() {
        bake.release();
    }
    result
}

/// Ask the running bake to stop. It winds down at the next item.
#[tauri::command]
pub fn bake_cancel(bake: State<'_, Arc<BakeState>>) -> Result<(), String> {
    bake.cancel();
    Ok(())
}

/// Whether a bake is running, so the UI can restore its state on remount.
#[tauri::command]
pub fn bake_running(bake: State<'_, Arc<BakeState>>) -> Result<bool, String> {
    Ok(bake.is_running())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_bake_can_claim_the_state() {
        let state = BakeState::default();
        assert!(state.claim());
        assert!(!state.claim(), "a second bake must be refused");
        state.release();
        assert!(state.claim(), "and allowed once the first finishes");
    }

    /// A cancel arriving before the next bake starts must not stop it, or a
    /// user who cancels and immediately restarts gets nothing.
    #[test]
    fn claiming_clears_a_stale_cancel() {
        let state = BakeState::default();
        state.cancel();
        assert!(state.cancelled());
        assert!(state.claim());
        assert!(!state.cancelled());
    }

    #[test]
    fn cancelling_when_nothing_runs_is_harmless() {
        let state = BakeState::default();
        state.cancel();
        assert!(!state.is_running());
    }
}
