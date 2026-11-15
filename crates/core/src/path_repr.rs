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

/// Native root-relative path bytes used as an exact store key.
///
/// Unix preserves the raw `OsStr` bytes. Other platforms use lossy UTF-8;
/// discovery rejects escaping paths before they reach the prototype store.
pub fn native_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes()
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
#[path = "path_repr.test.rs"]
mod tests;
