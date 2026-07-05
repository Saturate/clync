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
        self.responses.lock().unwrap().pop().unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_prompt_returns_responses_in_order() {
        let mock = MockInput::new(vec!["first", "second", "third"]);
        assert_eq!(mock.prompt("a").unwrap(), "first");
        assert_eq!(mock.prompt("b").unwrap(), "second");
        assert_eq!(mock.prompt("c").unwrap(), "third");
    }

    #[test]
    fn mock_prompt_returns_empty_when_exhausted() {
        let mock = MockInput::new(vec!["only"]);
        assert_eq!(mock.prompt("a").unwrap(), "only");
        assert_eq!(mock.prompt("b").unwrap(), "");
    }

    #[test]
    fn mock_prompt_with_default_uses_default_on_empty() {
        let mock = MockInput::new(vec!["", "custom"]);
        assert_eq!(
            mock.prompt_with_default("a", "fallback").unwrap(),
            "fallback"
        );
        assert_eq!(mock.prompt_with_default("b", "fallback").unwrap(), "custom");
    }

    #[test]
    fn mock_prompt_yn_defaults() {
        let mock = MockInput::new(vec!["", ""]);
        assert!(mock.prompt_yn("a", true).unwrap());
        assert!(!mock.prompt_yn("b", false).unwrap());
    }

    #[test]
    fn mock_prompt_yn_yes_no() {
        let mock = MockInput::new(vec!["y", "yes", "Y", "n", "no", "N"]);
        assert!(mock.prompt_yn("a", false).unwrap());
        assert!(mock.prompt_yn("b", false).unwrap());
        assert!(mock.prompt_yn("c", false).unwrap());
        assert!(!mock.prompt_yn("d", true).unwrap());
        assert!(!mock.prompt_yn("e", true).unwrap());
        assert!(!mock.prompt_yn("f", true).unwrap());
    }
}
