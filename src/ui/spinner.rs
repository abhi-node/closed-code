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
                .unwrap(),
        );
        bar.enable_steady_tick(Duration::from_millis(80));
        bar.set_message(message.to_string());
        Self { bar }
    }

    pub fn set_message(&self, message: &str) {
        self.bar.set_message(message.to_string());
    }

    pub fn finish(&self) {
        self.bar.finish_and_clear();
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
