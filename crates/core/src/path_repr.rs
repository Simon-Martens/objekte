//! Reversible query representation of native paths. Slash is the separator;
//! percent escapes encode a literal percent, backslash, and invalid UTF-8 bytes.

use std::path::Path;

pub fn query_path(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let mut output = String::new();
        let mut remaining = path.as_os_str().as_bytes();
        while !remaining.is_empty() {
            match std::str::from_utf8(remaining) {
                Ok(valid) => {
                    append_valid(&mut output, valid);
                    break;
                }
                Err(error) => {
                    let (valid, rest) = remaining.split_at(error.valid_up_to());
                    append_valid(
                        &mut output,
                        std::str::from_utf8(valid).expect("valid prefix"),
                    );
                    let invalid_len = error.error_len().unwrap_or(rest.len());
                    for byte in &rest[..invalid_len] {
                        output.push_str(&format!("%{byte:02X}"));
                    }
                    remaining = &rest[invalid_len..];
                }
            }
        }
        output
    }
    #[cfg(windows)]
    {
        use std::path::Component;
        let mut output = String::new();
        for (index, component) in path.components().enumerate() {
            if index > 0 {
                output.push('/');
            }
            match component {
                Component::Normal(name) => {
                    use std::os::windows::ffi::OsStrExt;
                    for unit in name.encode_wide() {
                        output.push_str(&format!("%u{unit:04X}"));
                    }
                }
                Component::Prefix(prefix) => {
                    use std::os::windows::ffi::OsStrExt;
                    for unit in prefix.as_os_str().encode_wide() {
                        output.push_str(&format!("%u{unit:04X}"));
                    }
                }
                Component::RootDir => output.push('/'),
                Component::CurDir => output.push('.'),
                Component::ParentDir => output.push_str(".."),
            }
        }
        output
    }
}

#[cfg(unix)]
fn append_valid(output: &mut String, valid: &str) {
    for ch in valid.chars() {
        match ch {
            '%' => output.push_str("%25"),
            '\\' => output.push_str("%5C"),
            _ => output.push(ch),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn invalid_bytes_and_literal_percent_remain_distinct() {
        use std::os::unix::ffi::OsStrExt;
        let native = Path::new(std::ffi::OsStr::from_bytes(b"a/\xFF%20\\.json"));
        assert_eq!(query_path(native), "a/%FF%2520%5C.json");
        assert_eq!(query_path(Path::new("a/ü.json")), "a/ü.json");
    }
}
