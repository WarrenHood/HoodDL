use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use reqwest::cookie;
use smart_default::SmartDefault;
use tokio::{
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::Mutex,
};

/// File downloader supporting multiple connections
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Url of the file to download
    #[arg(short, long)]
    url: String,

    /// Output file name
    #[arg(short, long)]
    filename: Option<String>,

    /// Number of connections to use
    #[arg(short, long, default_value_t = 8)]
    num_connections: u64,

    /// Cookies to use with the download requests
    #[arg(short, long)]
    cookies: Vec<String>,

    /// Size of chunk (in bytes) to download at a time per connection
    #[arg(short, long, default_value_t = 8388608)]
    segment_chunk_size: u64,
}

#[derive(Debug, SmartDefault, Clone)]
struct DownloadOptions {
    #[default = 8192]
    chunk_size: u64,
    cookies: Vec<String>,
    _headers: Vec<(String, String)>,
}

#[derive(Debug, Clone)]
struct FileSegmentDownloader {
    filename: String,
    url: String,
    offset: u64,
    size: u64,
    progress: u64,
    options: DownloadOptions,
}

impl FileSegmentDownloader {
    pub fn new(
        filename: &str,
        url: &str,
        offset: u64,
        size: u64,
        options: &DownloadOptions,
    ) -> Self {
        Self::new_with_progress(filename, url, offset, size, 0, options)
    }

    pub fn new_with_progress(
        filename: &str,
        url: &str,
        offset: u64,
        size: u64,
        progress: u64,
        options: &DownloadOptions,
    ) -> Self {
        Self {
            filename: filename.into(),
            url: url.into(),
            offset,
            size,
            progress,
            options: options.clone(),
        }
    }

    pub async fn download(&mut self, progress_bar: Option<ProgressBar>) -> Result<()> {
        // Open up the file, and seek to the start pos
        let start_pos = self.offset + self.progress;
        let end_pos = self.offset + self.size - 1;
        if start_pos >= end_pos {
            // We are already done
            println!("Segment download {}-{} already done", self.offset, end_pos);
            return Ok(());
        }
        if let Some(progress_bar) = &progress_bar {
            progress_bar.tick();
        }

        let mut file = tokio::fs::File::options()
            .read(true)
            .write(true)
            .create(false)
            .open(&self.filename)
            .await?;
        let cookie_jar = cookie::Jar::default();
        for cookie in self.options.cookies.iter() {
            cookie_jar.add_cookie_str(cookie, &self.url.parse()?);
        }
        let client = reqwest::ClientBuilder::new()
            .cookie_store(true)
            .cookie_provider(cookie_jar.into())
            .build()?;

        // let mut headers = reqwest::header::HeaderMap::new();
        // headers.append(reqwest::header::RANGE, format!("bytes={}-{}", start_pos, end_pos).parse()?);
        // TODO: Add other headers from the options here

        let chunks = (start_pos..=end_pos).step_by(self.options.chunk_size as usize);
        for chunk_start in chunks {
            let chunk_end = (chunk_start + self.options.chunk_size - 1).clamp(0, end_pos);
            if let Ok(chunk_response) = client
                .get(&self.url)
                .header(
                    reqwest::header::RANGE,
                    format!("bytes={}-{}", chunk_start, chunk_end),
                )
                .send()
                .await
            {
                file.seek(std::io::SeekFrom::Start(chunk_start)).await?;
                let chunk_bytes = chunk_response.bytes().await?;
                file.write(&chunk_bytes).await?;
                file.flush().await?;
                self.progress += chunk_bytes.len() as u64;
                if let Some(pb) = &progress_bar {
                    pb.inc(chunk_bytes.len() as u64);
                }
            }
        }
        if let Some(pb) = &progress_bar {
            pb.finish_with_message(format!(
                "Segment {}-{} complete",
                self.offset,
                self.offset + self.size - 1
            ));
        }
        Ok(())
    }
}

async fn download_file(
    filename: Option<&str>,
    url: &str,
    connections: u64,
    options: &DownloadOptions,
) -> Result<()> {
    let mut filename = filename;
    if filename.is_none() {
        filename = url.split("/").last();
    }
    if filename.is_none() {
        return Err(anyhow::format_err!("Please provide a filename"));
    }
    let filename = filename.unwrap();

    let cookie_jar = cookie::Jar::default();
    for cookie in options.cookies.iter() {
        cookie_jar.add_cookie_str(cookie, &url.parse()?);
    }
    let client = reqwest::ClientBuilder::new()
        .cookie_store(true)
        .cookie_provider(cookie_jar.into())
        .build()?;

    let response = client.head(url).send().await?;
    if let Some(content_length) = response.headers().get(reqwest::header::CONTENT_LENGTH) {
        let content_length: u64 = content_length.to_str()?.parse()?;
        println!(
            "Starting download of file {}, connections={}, size is {} bytes",
            filename, connections, content_length
        );
        let mut file = tokio::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .open(&filename)
            .await?;

        file.seek(std::io::SeekFrom::Start(content_length - 1))
            .await?;
        file.write(&[0]).await?;
        file.flush().await?;

        let segment_size = content_length / connections;
        let segments = (0..=content_length - 1).step_by(segment_size as usize);
        let mut downloaders = Vec::new();
        for segment_start in segments {
            let segment_end = (segment_start + segment_size - 1).clamp(0, content_length - 1);
            downloaders.push(Arc::new(Mutex::new(FileSegmentDownloader::new(
                filename,
                url,
                segment_start,
                segment_end - segment_start + 1,
                options,
            ))));
        }

        let progress_bars = indicatif::MultiProgress::new();
        progress_bars.set_draw_target(ProgressDrawTarget::stderr());
        let mut downloader_handles = Vec::new();
        for downloader in downloaders.iter() {
            let downloader = downloader.clone();
            let pb_message = {
                let downloader = downloader.lock().await;
                format!(
                    "Segment {}-{}",
                    downloader.offset,
                    downloader.offset + downloader.size - 1
                )
            };
            let pb = progress_bars
                .add(ProgressBar::new(downloader.lock().await.size).with_message(pb_message));
            pb.set_style(ProgressStyle::with_template("{msg} {spinner:.green} [{elapsed_precise}] [{wide_bar:.green}] {bytes}/{total_bytes} ETA {eta}").unwrap());
            downloader_handles.push(tokio::task::spawn(async move {
                downloader.lock().await.download(Some(pb)).await
            }));
        }

        let results = futures::future::join_all(downloader_handles).await;
        for result in results.iter() {
            let result = result.as_ref().unwrap();
            if let Err(err) = result {
                return Err(anyhow::format_err!(
                    "Download failed due to: {}",
                    err.to_string()
                ));
            }
        }

        for downloader in downloaders.iter() {
            let downloader = downloader.lock().await;
            if downloader.progress < downloader.size {
                return Err(anyhow::format_err!("Download failed"));
            }
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    download_file(
        args.filename.as_deref(),
        &args.url,
        args.num_connections,
        &DownloadOptions {
            chunk_size: args.segment_chunk_size,
            cookies: args.cookies,
            _headers: Vec::new(),
        },
    )
    .await?;

    Ok(())
}
