//! Bounded, noninteractive input for credentials supplied by a native client.
use std::io::{self, IsTerminal, Read};

pub(crate) const CONFIRMATION_LIMIT: usize = 1024;
pub(crate) const PROFILE_TOKEN_LIMIT: usize = 16 * 1024;

pub(crate) fn read_stdin(limit: usize) -> Result<String, &'static str> {
    let stdin = io::stdin();
    read_secret(stdin.lock(), stdin.is_terminal(), limit)
}

fn read_secret(reader: impl Read, terminal: bool, limit: usize) -> Result<String, &'static str> {
    if terminal {
        return Err(
            "Terminal stdin is not supported. Pipe the secret to stdin and close the stream.",
        );
    }
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Could not read secret input from stdin.")?;
    if bytes.len() > limit {
        return Err("Secret input exceeds the byte limit. See this command's --help.");
    }
    let input = String::from_utf8(bytes).map_err(|_| "Secret input must be valid UTF-8.")?;
    let input = input
        .trim_end_matches(['\r', '\n'])
        .trim_matches([' ', '\t']);
    if input.is_empty() {
        return Err("No secret content was received from stdin.");
    }
    if input.contains(['\r', '\n', '\0']) {
        return Err("Secret input must contain one line without embedded newlines or NUL bytes.");
    }
    Ok(input.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_input_handles_boundaries_and_newlines_without_echo() {
        for suffix in ["", "\n", "\r\n", "\n\n"] {
            let input = format!(" \tsynthetic-secret\t {suffix}");
            assert_eq!(
                read_secret(input.as_bytes(), false, 64).unwrap(),
                "synthetic-secret"
            );
        }
        for limit in [CONFIRMATION_LIMIT, PROFILE_TOKEN_LIMIT] {
            assert!(read_secret(vec![b'x'; limit].as_slice(), false, limit).is_ok());
            assert!(read_secret(vec![b' '; limit + 1].as_slice(), false, limit).is_err());
        }
        for input in [
            b"".as_slice(),
            b" \t\r\n",
            b"synthetic-secret\nsecond",
            b"synthetic-secret\0",
            b"synthetic-secret\xff",
        ] {
            let error = read_secret(input, false, 64).unwrap_err();
            assert!(!error.contains("synthetic-secret"));
        }
    }

    #[test]
    fn secret_input_rejects_terminal_and_stops_at_limit() {
        struct NeverRead;
        impl Read for NeverRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                panic!("terminal input must not be read")
            }
        }
        assert!(read_secret(NeverRead, true, 8).is_err());
        let mut endless = io::repeat(b'x');
        assert!(read_secret(&mut endless, false, 8)
            .unwrap_err()
            .contains("byte limit"));
        struct FailingRead;
        impl Read for FailingRead {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::other("synthetic-secret"))
            }
        }
        assert!(!read_secret(FailingRead, false, 8)
            .unwrap_err()
            .contains("synthetic-secret"));
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::{
        ffi::OsString,
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    pub(crate) struct Home {
        pub(crate) path: PathBuf,
        old_home: Option<OsString>,
        old_keychain: Option<OsString>,
    }
    impl Home {
        pub(crate) fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "cargo-ai-secret-unit-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            let result = Self {
                path,
                old_home: std::env::var_os("CARGO_AI_HOME"),
                old_keychain: std::env::var_os("CARGO_AI_DISABLE_KEYCHAIN"),
            };
            std::env::set_var("CARGO_AI_HOME", &result.path);
            std::env::set_var("CARGO_AI_DISABLE_KEYCHAIN", "1");
            fs::write(result.path.join("config.toml"), "secret_store = 'file'\n[[profile]]\nname = 'example'\nserver = 'openai'\nmodel = 'synthetic-model'\nauth_mode = 'api_key'\n[account]\nemail = 'owner@example.test'\n").unwrap();
            result
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            for (name, value) in [
                ("CARGO_AI_HOME", &self.old_home),
                ("CARGO_AI_DISABLE_KEYCHAIN", &self.old_keychain),
            ] {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
