use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

const BAR_WIDTH: usize = 20;

/// Animated progress bar with percentage, spinner, and live stats.
/// Background thread redraws a single terminal line at 10fps.
pub struct ProgressBar {
    done: Arc<AtomicBool>,
    pct: Arc<AtomicUsize>,   // 0..100
    phase: Arc<Mutex<String>>,
    stats: Arc<Mutex<String>>,
    start: Instant,
}

impl ProgressBar {
    pub fn start(phase: &str) -> Self {
        let done = Arc::new(AtomicBool::new(false));
        let pct = Arc::new(AtomicUsize::new(0));
        let phase = Arc::new(Mutex::new(phase.to_string()));
        let stats = Arc::new(Mutex::new(String::new()));
        let start = Instant::now();

        let d = done.clone();
        let p = pct.clone();
        let ph = phase.clone();
        let st = stats.clone();
        let s = start;

        std::thread::spawn(move || {
            let spinner = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let mut i = 0;
            while !d.load(Ordering::Relaxed) {
                let pct = p.load(Ordering::Relaxed);
                let phase = ph.lock().unwrap().clone();
                let stats = st.lock().unwrap().clone();
                let elapsed = s.elapsed().as_secs();

                let filled = BAR_WIDTH * pct / 100;
                let empty = BAR_WIDTH - filled;
                let bar = format!(
                    "{}{}",
                    "█".repeat(filled),
                    "░".repeat(empty),
                );

                let stats_display = if stats.is_empty() {
                    format!("{}s", elapsed)
                } else {
                    format!("{}  │  {}s", stats, elapsed)
                };

                eprint!(
                    "\x1b[2K\r  {} [{}] {:>3}%  {:<40} {}",
                    spinner[i % spinner.len()],
                    bar,
                    pct,
                    phase,
                    stats_display,
                );

                i += 1;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            // Final line with checkmark (shown by finish())
            eprint!("\x1b[2K\r");
        });

        Self { done, pct, phase, stats, start }
    }

    /// Set percentage (0..100)
    pub fn set_pct(&self, pct: usize) {
        self.pct.store(pct.min(100), Ordering::Relaxed);
    }

    /// Update phase text
    pub fn set_phase(&self, text: &str) {
        *self.phase.lock().unwrap() = text.to_string();
    }

    /// Update stats text (file/symbol counts)
    pub fn set_stats(&self, files: usize, funcs: usize) {
        *self.stats.lock().unwrap() = format!("{} files, {} symbols", files, funcs);
    }

    /// Stop and print final summary.
    pub fn finish(self, msg: &str) {
        self.set_pct(100);
        // Let the thread render the 100% frame
        std::thread::sleep(std::time::Duration::from_millis(50));
        self.done.store(true, Ordering::Relaxed);
        let elapsed = self.start.elapsed().as_secs();
        let stats = self.stats.lock().unwrap().clone();
        let detail = if stats.is_empty() {
            format!("{}s", elapsed)
        } else {
            format!("{}, {}s", stats, elapsed)
        };
        eprintln!("  \x1b[32m✓\x1b[0m {} ({})", msg, detail);
    }
}
