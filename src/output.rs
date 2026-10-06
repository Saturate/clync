use crossterm::style::{Attribute, Color, Stylize};
use std::io::{IsTerminal, Write};

pub struct SyncPrinter {
    color: bool,
}

impl SyncPrinter {
    pub fn new() -> Self {
        Self {
            color: std::io::stdout().is_terminal(),
        }
    }

    pub fn pull_start(&self) {
        if self.color {
            print!(
                "  {} ",
                "↓ pull".with(Color::Cyan).attribute(Attribute::Bold)
            );
        } else {
            print!("  ↓ pull ");
        }
        std::io::stdout().flush().ok();
    }

    pub fn push_start(&self) {
        if self.color {
            print!(
                "  {} ",
                "↑ push".with(Color::Magenta).attribute(Attribute::Bold)
            );
        } else {
            print!("  ↑ push ");
        }
        std::io::stdout().flush().ok();
    }

    pub fn pull_result(
        &self,
        pulled: u32,
        merged: u32,
        skipped: u32,
        archived: u32,
        extras: u32,
        memories: u32,
    ) {
        let mut parts = Vec::new();

        if pulled > 0 {
            parts.push(self.highlight(&format!("{pulled} new")));
        }
        if merged > 0 {
            parts.push(self.highlight(&format!("{merged} merged")));
        }
        parts.push(self.dim(&format!("{skipped} unchanged")));
        if archived > 0 {
            parts.push(self.dim(&format!("{archived} archived")));
        }

        println!("{}", parts.join(&self.sep()));

        let mut extras_parts = Vec::new();
        if extras > 0 {
            extras_parts.push(format!("{extras} extras"));
        }
        if memories > 0 {
            extras_parts.push(format!("{memories} memories"));
        }
        if !extras_parts.is_empty() {
            println!("           {}", self.dim(&extras_parts.join(" · ")));
        }
    }

    pub fn push_result(&self, pushed: u32, skipped: u32, extras: u32, memories: u32) {
        let mut parts = Vec::new();

        if pushed > 0 {
            parts.push(self.highlight(&format!("{pushed} encrypted")));
        }
        parts.push(self.dim(&format!("{skipped} unchanged")));

        println!("{}", parts.join(&self.sep()));

        let mut extras_parts = Vec::new();
        if extras > 0 {
            extras_parts.push(format!("{extras} extras"));
        }
        if memories > 0 {
            extras_parts.push(format!("{memories} memories"));
        }
        if !extras_parts.is_empty() {
            println!("           {}", self.dim(&extras_parts.join(" · ")));
        }
    }

    pub fn checkout_notice(&self, count: usize) {
        let msg = format!(
            "{count} project{} not cloned locally, run `clync checkout`",
            if count == 1 { "" } else { "s" }
        );
        if self.color {
            println!(
                "           {}",
                msg.with(Color::Yellow).attribute(Attribute::Dim)
            );
        } else {
            println!("           {msg}");
        }
    }

    fn highlight(&self, s: &str) -> String {
        if self.color {
            format!("{}", s.with(Color::White).attribute(Attribute::Bold))
        } else {
            s.to_string()
        }
    }

    fn dim(&self, s: &str) -> String {
        if self.color {
            format!("{}", s.with(Color::DarkGrey))
        } else {
            s.to_string()
        }
    }

    fn sep(&self) -> String {
        if self.color {
            format!(" {} ", "·".with(Color::DarkGrey))
        } else {
            " · ".to_string()
        }
    }
}
