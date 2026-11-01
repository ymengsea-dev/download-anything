//! One shared HTTP client, and streaming a response body to disk with progress and cancel.

use std::sync::LazyLock;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::io::AsyncWriteExt;

use crate::proc::{cancelled, err, Cancel, Result};

static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    reqwest::Client::builder()
        .user_agent(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
             (KHTML, like Gecko) Chrome/130.0 Safari/537.36",
        )
        .connect_timeout(Duration::from_secs(20))
        // A server that stops sending fails the request instead of hanging forever.
        .read_timeout(Duration::from_secs(30))
        .build()
        .expect("http client")
});

pub fn client() -> &'static reqwest::Client {
    &CLIENT
}

/// Streams `resp`'s body into `file`. `on_progress` gets whole percents, each one once,
/// or a single -1 when the size is unknown.
pub async fn stream_to_file(
    resp: reqwest::Response,
    file: &mut tokio::fs::File,
    cancel: Option<&Cancel>,
    on_progress: impl Fn(f64),
) -> Result<()> {
    let total = resp.content_length().filter(|t| *t > 0);
    let mut stream = resp.bytes_stream();
    let (mut done, mut last) = (0u64, None);
    loop {
        tokio::select! {
            chunk = stream.next() => match chunk {
                Some(chunk) => {
                    let chunk = chunk.map_err(err)?;
                    file.write_all(&chunk).await.map_err(err)?;
                    done += chunk.len() as u64;
                    let pct = total.map_or(-1.0, |t| (done as f64 / t as f64 * 100.0).floor().min(100.0));
                    if last != Some(pct) {
                        on_progress(pct);
                        last = Some(pct);
                    }
                }
                None => break,
            },
            _ = cancelled(cancel) => return Err("Cancelled".into()),
        }
    }
    file.flush().await.map_err(err)
}

/// GETs `url` into `dest`.
pub async fn fetch(url: &str, dest: &std::path::Path, on_progress: impl Fn(f64)) -> Result<()> {
    let resp = client().get(url).send().await.and_then(|r| r.error_for_status()).map_err(err)?;
    let mut file = tokio::fs::File::create(dest).await.map_err(err)?;
    stream_to_file(resp, &mut file, None, on_progress).await
}
