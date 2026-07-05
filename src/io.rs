use anyhow::Result;

pub trait InputSource {
    fn prompt(&self, label: &str) -> Result<String>;
    fn prompt_with_default(&self, label: &str, default: &str) -> Result<String>;
    fn prompt_yn(&self, label: &str, default: bool) -> Result<bool>;
}

pub struct StdioInput;

impl InputSource for StdioInput {
    fn prompt(&self, label: &str) -> Result<String> {
        use std::io::Write;
        print!("{label}: ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        Ok(input.trim().to_string())
    }

    fn prompt_with_default(&self, label: &str, default: &str) -> Result<String> {
        use std::io::Write;
        print!("{label} [{default}]: ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim();
        if trimmed.is_empty() {
            Ok(default.to_string())
        } else {
            Ok(trimmed.to_string())
        }
    }

    fn prompt_yn(&self, label: &str, default: bool) -> Result<bool> {
        use std::io::Write;
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        print!("{label} {hint} ");
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        let trimmed = input.trim().to_lowercase();
        if trimmed.is_empty() {
            Ok(default)
        } else {
            Ok(trimmed.starts_with('y'))
        }
    }
}

#[cfg(test)]
pub struct MockInput {
    responses: std::sync::Mutex<Vec<String>>,
}

#[cfg(test)]
impl MockInput {
    pub fn new(responses: Vec<&str>) -> Self {
        Self {
            responses: std::sync::Mutex::new(
                responses.into_iter().map(|s| s.to_string()).rev().collect(),
            ),
        }
    }

    fn next_response(&self) -> String {
        self.responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_default()
    }
}

#[cfg(test)]
impl InputSource for MockInput {
    fn prompt(&self, _label: &str) -> Result<String> {
        Ok(self.next_response())
    }

    fn prompt_with_default(&self, _label: &str, default: &str) -> Result<String> {
        let resp = self.next_response();
        if resp.is_empty() {
            Ok(default.to_string())
        } else {
            Ok(resp)
        }
    }

    fn prompt_yn(&self, _label: &str, default: bool) -> Result<bool> {
        let resp = self.next_response();
        if resp.is_empty() {
            Ok(default)
        } else {
            Ok(resp.to_lowercase().starts_with('y'))
        }
    }
}
