use indicatif::{ProgressBar, ProgressStyle};
use std::time::Duration;

pub struct Spinner {
    bar: ProgressBar,
}

impl Spinner {
    pub fn new(message: &str) -> Self {
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::default_spinner()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"])
                .template("{spinner} {msg}")
                .expect("hardcoded spinner template"),
        );
        bar.enable_steady_tick(Duration::from_millis(80));
        bar.set_message(message.to_string());
        Self { bar }
    }

    pub fn set_message(&self, message: &str) {
        self.bar.set_message(message.to_string());
    }

    /// Clear the spinner line entirely (no trace left).
    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }

    /// Stop the spinner but leave the message as a static line.
    pub fn finish_with_message(&self, message: &str) {
        self.bar.finish_with_message(message.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spinner_creation_and_finish() {
        let spinner = Spinner::new("Testing...");
        spinner.set_message("Updated message");
        spinner.finish();
    }
}
