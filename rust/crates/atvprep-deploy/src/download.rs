//! Bounded HTTPS fetching without redirects or unverified installation.
use crate::{
    Asset, Error, MAX_ASSET_BYTES, MAX_MANIFEST_BYTES, Manifest, Result, parse_signed_manifest,
    verify_asset,
};
use std::{io::Read, time::Duration};

pub struct Downloader {
    client: reqwest::blocking::Client,
    allow_loopback_http: bool,
}

impl Downloader {
    /// Build a downloader with an overall request timeout and no redirects.
    ///
    /// # Errors
    /// The timeout is zero or the HTTPS client cannot be initialized.
    pub fn new(timeout: Duration) -> Result<Self> {
        if timeout.is_zero() {
            return Err(Error::Invalid(
                "download timeout must be positive".to_owned(),
            ));
        }
        let client = reqwest::blocking::Client::builder()
            .timeout(timeout)
            .connect_timeout(timeout.min(Duration::from_secs(15)))
            .redirect(reqwest::redirect::Policy::none())
            .https_only(true)
            .user_agent(concat!("atvprep/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| Error::Invalid("cannot initialize HTTPS downloader".to_owned()))?;
        Ok(Self {
            client,
            allow_loopback_http: false,
        })
    }

    fn fetch(&self, url: &str, limit: u64) -> Result<Vec<u8>> {
        let url = reqwest::Url::parse(url)
            .map_err(|_| Error::Invalid("invalid download URL".to_owned()))?;
        let local = self.allow_loopback_http
            && url.scheme() == "http"
            && url
                .host_str()
                .is_some_and(|host| host == "127.0.0.1" || host == "[::1]");
        if (!local && url.scheme() != "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Error::Invalid(
                "download URL must use HTTPS without embedded credentials".to_owned(),
            ));
        }
        let response = self
            .client
            .get(url)
            .send()
            .map_err(|_| Error::Invalid("release download failed".to_owned()))?;
        if !response.status().is_success() {
            return Err(Error::Invalid(
                "release server returned an unsuccessful status".to_owned(),
            ));
        }
        if response.content_length().is_some_and(|size| size > limit) {
            return Err(Error::Invalid(
                "release download exceeds size limit".to_owned(),
            ));
        }
        let mut body = Vec::new();
        response
            .take(limit.saturating_add(1))
            .read_to_end(&mut body)
            .map_err(|_| Error::Invalid("release download was interrupted".to_owned()))?;
        if u64::try_from(body.len()).unwrap_or(u64::MAX) > limit {
            return Err(Error::Invalid(
                "release download exceeds size limit".to_owned(),
            ));
        }
        Ok(body)
    }

    /// Fetch and authenticate the manifest before trusting any asset descriptor.
    ///
    /// # Errors
    /// Fetching, size limits, signatures or manifest validation fail.
    pub fn manifest(&self, url: &str, signature_url: &str, trusted_key: &str) -> Result<Manifest> {
        let manifest = self.fetch(url, MAX_MANIFEST_BYTES as u64)?;
        let signature = self.fetch(signature_url, 64 * 1024)?;
        parse_signed_manifest(trusted_key, &manifest, &signature)
    }

    /// Download exactly the authenticated asset size and verify its SHA-256.
    ///
    /// # Errors
    /// The asset exceeds supported limits, is interrupted or fails verification.
    pub fn asset(&self, asset: &Asset) -> Result<Vec<u8>> {
        if asset.size == 0 || asset.size > MAX_ASSET_BYTES {
            return Err(Error::Invalid("invalid release asset size".to_owned()));
        }
        let body = self.fetch(&asset.url, asset.size)?;
        verify_asset(asset, &body)?;
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::{io::Write, net::TcpListener, thread};

    fn source(response: Vec<u8>) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/asset", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&response);
        });
        (url, handle)
    }
    fn local() -> Downloader {
        Downloader {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_secs(2))
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            allow_loopback_http: true,
        }
    }
    fn asset(url: String) -> Asset {
        Asset {
            platform: "linux-x86_64".to_owned(),
            url,
            sha256: format!("{:x}", Sha256::digest(b"abc")),
            size: 3,
        }
    }

    #[test]
    fn downloads_only_exact_verified_bytes_and_refuses_redirects_or_truncation() {
        let responses=[b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nabc".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nbad",
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nabcd",
            b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\nab",
            b"HTTP/1.1 302 Found\r\nLocation: https://example.test/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\nabcd\r\n0\r\n\r\n"];
        for (index, response) in responses.into_iter().enumerate() {
            let (url, server) = source(response.to_vec());
            let result = local().asset(&asset(url));
            if index == 0 {
                assert_eq!(result.unwrap(), b"abc");
            } else {
                assert!(result.is_err(), "response {index}");
            }
            server.join().unwrap();
        }
    }

    #[test]
    fn overall_timeout_interrupts_a_stalled_response_body() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/asset", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\na")
                .unwrap();
            thread::sleep(Duration::from_millis(300));
            let _ = stream.write_all(b"bc");
        });
        let downloader = Downloader {
            client: reqwest::blocking::Client::builder()
                .timeout(Duration::from_millis(75))
                .build()
                .unwrap(),
            allow_loopback_http: true,
        };
        let started = std::time::Instant::now();
        assert!(downloader.asset(&asset(url)).is_err());
        assert!(started.elapsed() < Duration::from_secs(1));
        server.join().unwrap();
    }

    #[test]
    fn production_downloader_refuses_plain_http_before_connecting() {
        let downloader = Downloader::new(Duration::from_secs(1)).unwrap();
        assert!(
            downloader
                .asset(&asset("http://127.0.0.1:1/private".to_owned()))
                .is_err()
        );
    }
}
