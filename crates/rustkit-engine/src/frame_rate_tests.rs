//! PACE2: animation frames and 10 ms timer links per wall second on the content thread's loop.
//!
//! The page (`docs/diagnostics/2026-11-15/frame_rate_probe.html`) runs a requestAnimationFrame
//! loop and a `setTimeout(link, 10)` chain for 3000 ms of `performance.now`. Chrome for Testing
//! 148.0.7778.216, five runs (`frame_rate_probe_cft148.json`): 180-181 frames and 281-283 links.
//!
//! The grid probe (`frame_grid_probe.html`) runs a requestAnimationFrame loop whose every 4th
//! callback runs 4 ms and every 25th 24 ms, and records the gaps between the callbacks'
//! timestamps (`frame_grid_probe_cft148.json`: 180-181 frames, every gap 16.5-16.8 ms).

use std::time::{Duration, Instant};

use crate::load_turns_tests::{html, serve, Load};

pub(crate) const PROBE_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-11-15/frame_rate_probe.html");
pub(crate) const GRID_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-11-15/frame_grid_probe.html");

/// Loads `page` with `#manual`, starts it in a live turn after the load, and returns its record.
pub(crate) fn run_probe_page(page: &'static str) -> serde_json::Value {
    run_probe_page_with(page, "")
}

/// [`run_probe_page`], with `setup` run before `go()`.
pub(crate) fn run_probe_page_with(page: &'static str, setup: &str) -> serde_json::Value {
    let fixture = serve(vec![("/", html(page))]);
    let mut load = Load::start(&fixture, "/#manual");
    load.wait_complete();
    let _ = load.eval(&format!("{setup} go(); 'started'"), Duration::from_secs(10));
    // No commands while the probe runs: an eval is a turn the loop did not pace.
    std::thread::sleep(Duration::from_millis(3300));
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (answer, _) = load.eval(
            "window.rec && rec.done ? JSON.stringify(rec) : ''",
            Duration::from_secs(10),
        );
        let answer = answer.unwrap_or_default();
        let text = answer
            .strip_prefix("String(\"")
            .and_then(|s| s.strip_suffix("\")"))
            .unwrap_or_default()
            .replace("\\\"", "\"");
        if !text.is_empty() {
            return serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
        }
        assert!(Instant::now() < deadline, "the probe did not finish");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The frame rate probe's record.
pub(crate) fn run_probe() -> serde_json::Value {
    run_probe_page(PROBE_PAGE)
}

#[cfg(windows)]
#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(period: u32) -> u32;
}

/// Windows sleeps in 15.6 ms ticks unless a process asks for 1 ms; `HIWAVE_PACE2_1MS` asks, so a
/// sleep lasts about what it does on macOS.
fn sleeps_as_on_macos_if_asked() {
    #[cfg(windows)]
    if std::env::var_os("HIWAVE_PACE2_1MS").is_some() {
        unsafe { timeBeginPeriod(1) };
    }
}

#[test]
#[ignore]
fn measure_the_content_threads_frame_rate() {
    sleeps_as_on_macos_if_asked();
    for _ in 0..3 {
        eprintln!("PACE2-RESULT {}", run_probe());
    }
}

#[test]
#[ignore]
fn measure_the_content_threads_frame_grid() {
    sleeps_as_on_macos_if_asked();
    for _ in 0..3 {
        eprintln!("PACE2-GRID {}", run_probe_page(GRID_PAGE));
    }
}

/// LINKS-SLOW: the lateness of each link of the 10 ms chain on the content thread's loop
/// (`links_lateness_probe.html`; Chrome, five runs, `links_lateness_probe_cft148.json`: 278-281
/// links, lateness mean 0.64-0.75 ms, median 0.6-0.7, p90 1.1-1.5, max 2.0-2.7).
pub(crate) const LINKS_PAGE: &str =
    include_str!("../../../docs/diagnostics/2026-11-16/links_lateness_probe.html");

/// LINKS-SLOW part 2: how late a sleep to an instant a fraction of a millisecond away ends, on a
/// current-thread tokio runtime (the content loop's) and with `std::thread::sleep`: 200 sleeps
/// each, aimed 9.0-9.9 ms ahead in 0.1 ms steps. Prints the overshoot's percentiles in ms.
#[test]
#[ignore]
fn measure_sleep_overshoot() {
    sleeps_as_on_macos_if_asked();
    fn pcts(mut xs: Vec<f64>) -> String {
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let q = |p: f64| xs[((xs.len() as f64 * p) as usize).min(xs.len() - 1)];
        let mean = xs.iter().sum::<f64>() / xs.len() as f64;
        format!("mean {mean:.3} p10 {:.3} p50 {:.3} p90 {:.3} max {:.3}", q(0.1), q(0.5), q(0.9), xs[xs.len() - 1])
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut tokio_late = Vec::new();
    let mut std_late = Vec::new();
    for i in 0..200u64 {
        let ahead = Duration::from_micros(9_000 + (i % 10) * 100);
        let due = Instant::now() + ahead;
        rt.block_on(async { tokio::time::sleep_until(tokio::time::Instant::from_std(due)).await });
        tokio_late.push(Instant::now().saturating_duration_since(due).as_secs_f64() * 1000.0);
        let due = Instant::now() + ahead;
        std::thread::sleep(due.saturating_duration_since(Instant::now()));
        std_late.push(Instant::now().saturating_duration_since(due).as_secs_f64() * 1000.0);
    }
    eprintln!("SLEEP-OVERSHOOT tokio sleep_until: {}", pcts(tokio_late));
    eprintln!("SLEEP-OVERSHOOT std::thread::sleep: {}", pcts(std_late));
}

/// This process's CPU time so far (user + kernel), all threads.
fn process_cpu() -> Duration {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetCurrentProcess() -> isize;
            fn GetProcessTimes(process: isize, created: *mut u64, exited: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
        }
        let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
        // 100 ns units.
        Duration::from_nanos((k + u) * 100)
    }
    #[cfg(unix)]
    {
        let mut r: libc::rusage = unsafe { std::mem::zeroed() };
        unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut r) };
        let tv = |t: libc::timeval| Duration::from_secs(t.tv_sec as u64) + Duration::from_micros(t.tv_usec as u64);
        tv(r.ru_utime) + tv(r.ru_stime)
    }
}

/// LINKS-SLOW part 2 R1: the CPU a page costs that runs `setInterval(f, 10)` and nothing else, on
/// the content thread's loop, against the same page with no timer: 3000 ms each, three runs.
/// Prints `CPU-RESULT idle|interval <cpu ms> <percent of one core>`.
#[test]
#[ignore]
fn measure_an_interval_pages_cpu() {
    sleeps_as_on_macos_if_asked();
    const PAGE: &str = "<!doctype html><body><script>var n = 0; \
        function go() { setInterval(function () { n++; }, 10); }</script></body>";
    for run in 0..3 {
        for (name, start) in [("idle", "'started'"), ("interval", "go(); 'started'")] {
            let fixture = serve(vec![("/", html(PAGE))]);
            let mut load = Load::start(&fixture, "/");
            load.wait_complete();
            let _ = load.eval(start, Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(300));
            let (cpu0, t0) = (process_cpu(), Instant::now());
            std::thread::sleep(Duration::from_millis(3000));
            let (cpu, wall) = (process_cpu() - cpu0, t0.elapsed());
            let (n, _) = load.eval("n", Duration::from_secs(10));
            eprintln!(
                "CPU-RESULT run {run} {name} {:.1} ms {:.2}% ticks {:?}",
                cpu.as_secs_f64() * 1000.0,
                cpu.as_secs_f64() / wall.as_secs_f64() * 100.0,
                n
            );
        }
    }
}

#[test]
#[ignore]
fn measure_the_content_threads_link_lateness() {
    sleeps_as_on_macos_if_asked();
    let setup = if std::env::var_os("LINKS_NO_FRAMES").is_some() { "window.NO_FRAMES = true;" } else { "" };
    for _ in 0..3 {
        let rec = run_probe_page_with(LINKS_PAGE, setup);
        let min = rec["late"]
            .as_array()
            .map(|l| l.iter().filter_map(|x| x.as_f64()).fold(f64::INFINITY, f64::min))
            .unwrap_or(f64::NAN);
        eprintln!(
            "LINKS-RESULT frames {} links {} summary {} min {min}",
            rec["frames"], rec["timers"], rec["summary"]
        );
    }
}
