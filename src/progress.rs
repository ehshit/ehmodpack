use std::io::{IsTerminal, Write};

pub const LABEL: usize = 52;
pub const BAR: usize = 20;
const INDENT: usize = 2;
const TOTAL: usize = INDENT + LABEL + 1 + BAR + 1 + 24;

pub fn interactive() -> bool {
    std::io::stdout().is_terminal()
}

pub struct Bar {
    label: String,
    live: bool,
}

impl Bar {
    pub fn new(label: &str) -> Self {
        let bar = Self {
            label: label.to_string(),
            live: interactive(),
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
            self.live = false;
        }
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
        self.emit(&format!("{fill} {size}"));
    }

    fn emit(&self, tail: &str) {
        let line = format!("{indent}{label:<LABEL$} {tail}", indent = " ".repeat(INDENT), label = clip(&self.label, LABEL));
        let pad = TOTAL.saturating_sub(line.chars().count());
        print!("\r{line}{}", " ".repeat(pad));
        let _ = std::io::stdout().flush();
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
    width: usize,
}

impl Steps {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            live: interactive(),
            width: 0,
        }
    }

    pub fn at(&mut self, n: usize, text: &str) {
        if !self.live {
            return;
        }
        let line = format!("  [{n}/{}] {text}", self.total);
        let pad = self.width.saturating_sub(line.chars().count());
        self.width = self.width.max(line.chars().count());
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