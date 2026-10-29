//! Simple HTTP downloads with a progress callback.

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;

use crate::error::{Error, IoContext, Result};

/// HTTP client for all downloads. It trusts the operating system's certificate
/// store (the macOS Keychain), like other Mac apps do, so downloads also work
/// behind company proxies and VPNs that use their own root certificates.
pub fn agent() -> ureq::Agent {
    use ureq::tls::{RootCerts, TlsConfig};
    ureq::Agent::config_builder()
        .tls_config(
            TlsConfig::builder()
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
        .into()
}

/// Download `url` to `dest`. The data is written to a `.part` file first and
/// renamed on success, so an interrupted download never looks complete.
///
/// `progress` is called with `(bytes_so_far, total_bytes_if_known)`.
pub fn download(url: &str, dest: &Path, mut progress: impl FnMut(u64, Option<u64>)) -> Result<()> {
    let fail = |message: String| Error::Download {
        url: url.to_string(),
        message,
    };

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).at(parent)?;
    }

    let mut response = agent().get(url).call().map_err(|e| fail(e.to_string()))?;
    let total = response.body().content_length();
    let mut reader = response.body_mut().as_reader();

    let part = dest.with_extension("part");
    let mut file = File::create(&part).at(&part)?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| fail(e.to_string()))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).at(&part)?;
        done += n as u64;
        progress(done, total);
    }
    file.sync_all().at(&part)?;
    drop(file);

    if let Some(total) = total.filter(|&t| t != done) {
        let _ = std::fs::remove_file(&part);
        return Err(fail(format!("expected {total} bytes, received {done}")));
    }
    std::fs::rename(&part, dest).at(dest)
}
