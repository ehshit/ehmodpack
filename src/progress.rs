use std::io::{IsTerminal, Write};
use std::time::Instant;

pub const LABEL: usize = 52;
pub const BAR: usize = 20;
const INDENT: usize = 2;

fn frame_cap() -> usize {
    let width = terminal_size::terminal_size()
        .map(|(w, _)| w.0 as usize)
        .unwrap_or(120);
    width.saturating_sub(1).max(20)
}

pub fn interactive() -> bool {
    std::io::stdout().is_terminal()
}

pub struct Bar {
    label: String,
    live: bool,
    start: Instant,
}

impl Bar {
    pub fn new(label: &str) -> Self {
        let bar = Self {
            label: label.to_string(),
            live: interactive(),
            start: Instant::now(),
        };
        bar.draw(0, None);
        bar
    }

    pub fn tick(&mut self, got: u64, total: Option<u64>) {
        self.draw(got, total);
    }

    pub fn done(&mut self, total: u64) {
        if self.live {
            self.emit(&crate::staging::human(total));
            println!();
            self.live = false;
        }
    }

    fn rate(&self, got: u64) -> f64 {
        let dt = self.start.elapsed().as_secs_f64();
        if dt <= 0.0 {
            return 0.0;
        }
        got as f64 / dt
    }

    fn draw(&self, got: u64, total: Option<u64>) {
        if !self.live {
            return;
        }
        let (fill, size) = match total {
            Some(t) if t > 0 => {
                let filled = ((got as f64 / t as f64) * BAR as f64).round() as usize;
                let filled = filled.min(BAR);
                (
                    format!("[{}{}]", "#".repeat(filled), "-".repeat(BAR - filled)),
                    format!(
                        "{}/{}",
                        crate::staging::human(got),
                        crate::staging::human(t)
                    ),
                )
            }
            _ => (
                format!("[{}]", "~".repeat(BAR)),
                crate::staging::human(got),
            ),
        };
        let tail = match total {
            Some(t) if t > 0 => {
                let rate = self.rate(got);
                if rate > 0.0 {
                    let eta = ((t - got) as f64 / rate).max(0.0);
                    format!(
                        "{fill} {size} @ {} · {} left",
                        human_rate(rate),
                        human_time(eta)
                    )
                } else {
                    format!("{fill} {size}")
                }
            }
            _ => format!("{fill} {size}"),
        };
        self.emit(&tail);
    }

    fn emit(&self, tail: &str) {
        let raw = format!(
            "{indent}{label:<LABEL$} {tail}",
            indent = " ".repeat(INDENT),
            label = clip(&self.label, LABEL)
        );
        let cap = frame_cap();
        let line = clip(&raw, cap);
        let pad = cap.saturating_sub(line.chars().count());
        print!("\r{line}{}", " ".repeat(pad));
        let _ = std::io::stdout().flush();
    }
}

pub fn human_rate(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 4] = ["B/s", "KB/s", "MB/s", "GB/s"];
    let mut v = bytes_per_sec;
    let mut unit = 0;
    while v >= 1024.0 && unit + 1 < UNITS.len() {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{v:.0} {}", UNITS[unit])
    } else {
        format!("{v:.1} {}", UNITS[unit])
    }
}

pub fn human_time(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 3600 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

pub fn line(text: &str) {
    if interactive() {
        println!("  {text}");
    }
}

pub struct Steps {
    total: usize,
    live: bool,
}

impl Steps {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            live: interactive(),
        }
    }

    pub fn at(&mut self, n: usize, text: &str) {
        if !self.live {
            return;
        }
        let cap = frame_cap();
        let line = clip(&format!("  [{n}/{}] {text}", self.total), cap);
        let pad = cap.saturating_sub(line.chars().count());
        print!("\r{line}{}", " ".repeat(pad));
        let _ = std::io::stdout().flush();
    }

    pub fn finish(&mut self) {
        if self.live {
            println!();
            self.live = false;
        }
    }
}