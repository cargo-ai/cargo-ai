//! Explicit consent for hosted account lifecycle changes.
use std::io::{self, IsTerminal, Write};

pub(super) fn confirm(prompt: &str, accepted: bool) -> Result<bool, String> {
    if accepted {
        return Ok(true);
    }
    let answer = read_answer(prompt)?;
    Ok(matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

pub(super) fn read_answer(prompt: &str) -> Result<String, String> {
    if !io::stdin().is_terminal() {
        return Err(
            "Noninteractive account changes require explicit confirmation flags. See --help."
                .into(),
        );
    }
    print!("{prompt}");
    io::stdout().flush().map_err(|error| error.to_string())?;
    let mut answer = String::new();
    io::stdin()
        .read_line(&mut answer)
        .map_err(|error| error.to_string())?;
    Ok(answer.trim().to_string())
}
