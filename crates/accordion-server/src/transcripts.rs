//! Downloading the speech models that transcribe voice memos (decision 33),
//! with the fetcher updates use, so requests say nothing about the server.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use accordion_core::transcripts::{Download, MODELS_URL};

use crate::updates::fetch::Fetcher;

/// Downloads models from Accordion's release.
pub struct ModelDownload {
    fetcher: Fetcher,
}

impl ModelDownload {
    /// `allow_http` is for a test folder on the home network.
    pub fn new(allow_http: bool) -> Result<Self, String> {
        Ok(Self {
            fetcher: Fetcher::new(allow_http)?,
        })
    }
}

impl Download for ModelDownload {
    fn download<'a>(
        &'a self,
        url: &'a str,
        to: &'a Path,
        limit: u64,
        progress: &'a (dyn Fn(u64) + Send + Sync),
    ) -> Pin<Box<dyn Future<Output = Result<[u8; 32], String>> + Send + 'a>> {
        Box::pin(self.fetcher.download_reporting(url, to, limit, progress))
    }
}

/// Where models come from: Accordion's release, or the folder in
/// `ACCORDION_MODELS_URL`, for trying them on a test machine. The second
/// part is whether it's plain HTTP. Models are checked against the hashes
/// built into this program either way.
pub fn models_url() -> Result<(String, bool), String> {
    let Ok(url) = std::env::var("ACCORDION_MODELS_URL") else {
        return Ok((MODELS_URL.to_owned(), false));
    };
    let url = url.trim().trim_end_matches('/');
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err(format!(
            "ACCORDION_MODELS_URL is \"{url}\". Set it to the web address of a folder of \
             model files, such as http://192.168.1.20:8000/models."
        ));
    }
    Ok((url.to_owned(), url.starts_with("http://")))
}

/// whisper.cpp's program: the one Accordion's package installs, or the
/// one in `ACCORDION_WHISPER`, for a developer's own build. A program
/// outside `/usr` runs only without the worker service.
pub fn whisper_program() -> Option<PathBuf> {
    std::env::var_os("ACCORDION_WHISPER")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
}

/// The host models come from, for the Network page.
pub fn models_host(url: &str) -> String {
    url.parse::<hyper::Uri>()
        .ok()
        .and_then(|uri| uri.host().map(str::to_owned))
        .unwrap_or_else(|| url.to_owned())
}
