use colored::Colorize;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::env;
use std::io::{self, IsTerminal};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct Reporter {
    verbose: u8,
    quiet: bool,
    interactive: bool,
    use_color: bool,
}

impl Reporter {
    pub fn new(verbose: u8, quiet: bool) -> Self {
        let no_color = env::var("NO_COLOR")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);

        let interactive = io::stderr().is_terminal();
        let use_color = interactive && !no_color;

        Self {
            verbose,
            quiet,
            interactive,
            use_color,
        }
    }

    pub fn info(&self, message: impl AsRef<str>) {
        if self.quiet {
            return;
        }

        println!("{}", self.prefixed("info", message.as_ref()));
    }

    pub fn success(&self, message: impl AsRef<str>) {
        if self.quiet {
            return;
        }

        println!("{}", self.prefixed("ok", message.as_ref()));
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        eprintln!("{}", self.prefixed("warn", message.as_ref()));
    }

    pub fn error(&self, message: impl AsRef<str>) {
        eprintln!("{}", self.prefixed("error", message.as_ref()));
    }

    pub fn debug(&self, message: impl AsRef<str>) {
        if self.quiet || self.verbose == 0 {
            return;
        }

        eprintln!("{}", self.prefixed("debug", message.as_ref()));
    }

    pub fn spinner(&self, message: impl AsRef<str>) -> Option<ProgressBar> {
        if self.quiet {
            return None;
        }

        if !self.interactive {
            self.info(message.as_ref());
            return None;
        }

        let spinner = ProgressBar::new_spinner();
        spinner.set_draw_target(ProgressDrawTarget::stderr());
        spinner.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner()),
        );
        spinner.set_message(message.as_ref().to_string());
        spinner.enable_steady_tick(Duration::from_millis(100));
        Some(spinner)
    }

    pub fn progress_bytes(&self, total: Option<u64>, message: impl AsRef<str>) -> Option<ProgressBar> {
        if self.quiet {
            return None;
        }

        if !self.interactive {
            self.info(message.as_ref());
            return None;
        }

        let progress = match total {
            Some(total) if total > 0 => {
                let bar = ProgressBar::new(total);
                bar.set_style(
                    ProgressStyle::with_template(
                        "{spinner:.cyan} [{bar:40.cyan/blue}] {bytes}/{total_bytes} @ {binary_bytes_per_sec} ({eta})",
                    )
                    .unwrap_or_else(|_| ProgressStyle::default_bar())
                    .progress_chars("=>-"),
                );
                bar
            }
            _ => {
                let spinner = ProgressBar::new_spinner();
                spinner.set_style(
                    ProgressStyle::with_template("{spinner:.cyan} {msg} {bytes} @ {binary_bytes_per_sec}")
                        .unwrap_or_else(|_| ProgressStyle::default_spinner()),
                );
                spinner.enable_steady_tick(Duration::from_millis(100));
                spinner
            }
        };

        progress.set_draw_target(ProgressDrawTarget::stderr());
        progress.set_message(message.as_ref().to_string());
        Some(progress)
    }

    pub fn progress_items(&self, total: u64, message: impl AsRef<str>) -> Option<ProgressBar> {
        if self.quiet {
            return None;
        }

        if !self.interactive {
            self.info(message.as_ref());
            return None;
        }

        if total == 0 {
            return self.spinner(message);
        }

        let bar = ProgressBar::new(total);
        bar.set_draw_target(ProgressDrawTarget::stderr());
        bar.set_style(
            ProgressStyle::with_template(
                "{spinner:.cyan} [{bar:40.cyan/blue}] {pos}/{len} files ({per_sec}, {eta})",
            )
            .unwrap_or_else(|_| ProgressStyle::default_bar())
            .progress_chars("=>-"),
        );
        bar.set_message(message.as_ref().to_string());
        Some(bar)
    }

    fn prefixed(&self, level: &str, message: &str) -> String {
        if self.use_color {
            match level {
                "ok" => format!("{} {}", "cook".green().bold(), message),
                "warn" => format!("{} {}", "cook".yellow().bold(), message),
                "error" => format!("{} {}", "cook".red().bold(), message),
                "debug" => format!("{} {}", "cook".blue().bold(), message),
                _ => format!("{} {}", "cook".cyan().bold(), message),
            }
        } else {
            format!("cook: {}", message)
        }
    }
}
