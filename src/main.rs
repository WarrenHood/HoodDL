use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use regex::Regex;
use std::{collections::HashMap, sync::Arc, time::Duration};

use anyhow::Result;
use clap::Parser;
use itertools::Itertools;
use reqwest::cookie;
use smart_default::SmartDefault;
use tokio::{
    io::{AsyncSeekExt, AsyncWriteExt},
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex,
    },
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

    /// The concurrent target total size of all chunks in MB
    #[arg(short, long, default_value_t = 20.0)]
    target_total_chunk_size: f64,

    /// Timeout in seconds before retrying a chunk download
    #[arg(long, default_value_t = 15)]
    chunk_retry_timeout: u64,

    /// Chunk retry delay in seconds
    #[arg(long, default_value_t = 5)]
    chunk_retry_delay: u64,

    /// Create a new client every time we make a request
    #[arg(long, default_value_t = true)]
    new_client_per_request: bool,
}

#[derive(Debug, SmartDefault, Clone)]
struct DownloadOptions {
    #[default = 1048576]
    chunk_size: u64,
    cookies: Vec<String>,
    _headers: Vec<(String, String)>,
    retry_timeout: u64,
    retry_delay: u64,
    new_client_per_request: bool,
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

#[derive(Debug, Clone, Copy)]
enum ProgressManagerCommand {
    UpdateProgress(SegmentDownloadProgress),
    Stop,
}

#[derive(Debug, Clone, Copy)]
struct SegmentDownloadProgress {
    offset: u64,
    size: u64,
    progress: u64,
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

    pub async fn download(
        &mut self,
        progress_bar: Option<ProgressBar>,
        tx: Sender<ProgressManagerCommand>,
    ) -> Result<()> {
        if self.size == 0 {
            return Err(anyhow::format_err!("Cannot download zero-length segment"));
        }
        // Open up the file, and seek to the start pos
        let start_pos = self.offset + self.progress;
        let end_pos = self.offset + self.size - 1;
        if let Some(progress_bar) = &progress_bar {
            progress_bar.set_position(self.progress);
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
        let mut client = reqwest::ClientBuilder::new()
            .cookie_store(true)
            .cookie_provider(cookie_jar.into())
            .build()?;

        // let mut headers = reqwest::header::HeaderMap::new();
        // headers.append(reqwest::header::RANGE, format!("bytes={}-{}", start_pos, end_pos).parse()?);
        // TODO: Add other headers from the options here

        let chunks = (start_pos..=end_pos).step_by(self.options.chunk_size as usize);
        for chunk_start in chunks {
            if chunk_start < self.progress {
                // If we are resuming downloads, we can skip the chunks we already have
                continue;
            }
            let chunk_end = (chunk_start + self.options.chunk_size - 1).clamp(0, end_pos);
            loop {
                if let Ok(chunk_response) = client
                    .get(&self.url)
                    .header(
                        reqwest::header::RANGE,
                        format!("bytes={}-{}", chunk_start, chunk_end),
                    )
                    .timeout(Duration::from_secs(self.options.retry_timeout))
                    .send()
                    .await
                {
                    file.seek(std::io::SeekFrom::Start(chunk_start)).await?;
                    if self.options.new_client_per_request {
                        // Create a new client so the server doesn't try to throttle us
                        let cookie_jar = cookie::Jar::default();
                        for cookie in self.options.cookies.iter() {
                            cookie_jar.add_cookie_str(cookie, &self.url.parse()?);
                        }
                        client = reqwest::ClientBuilder::new()
                            .cookie_store(true)
                            .cookie_provider(cookie_jar.into())
                            .build()?;
                    }
                    if let Ok(chunk_bytes) = chunk_response.bytes().await {
                        file.write_all(&chunk_bytes).await?;
                        file.flush().await?;
                        self.progress += chunk_bytes.len() as u64;
                        if let Err(err) = tx
                            .send(ProgressManagerCommand::UpdateProgress(
                                SegmentDownloadProgress {
                                    offset: self.offset,
                                    size: self.size,
                                    progress: self.progress,
                                },
                            ))
                            .await
                        {
                            eprintln!(
                                "Failed to send progress update for segment {}-{}: {:?}",
                                self.offset,
                                self.offset + self.size - 1,
                                err
                            );
                        }
                        if let Some(pb) = &progress_bar {
                            pb.set_position(self.progress);
                        }
                        break;
                    }
                } else {
                    tokio::time::sleep(Duration::from_secs(self.options.retry_delay)).await;
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

async fn handle_segment_download_progress(
    filename: String,
    initial_progress: HashMap<u64, SegmentDownloadProgress>,
    mut rx: Receiver<ProgressManagerCommand>,
) {
    let progress_filename = format!("{}.dlprogress", filename);
    let mut current_progress = initial_progress;
    while let Some(command) = rx.recv().await {
        match command {
            ProgressManagerCommand::UpdateProgress(new_progress) => {
                // Got new progress update for a segment
                current_progress.insert(new_progress.offset, new_progress);
                let sorted_offsets = current_progress.keys().into_iter().sorted();
                let progress = sorted_offsets
                    .map(|k| current_progress.get(k).unwrap())
                    .map(|prog| format!("{} {} {}", prog.offset, prog.size, prog.progress))
                    .join("\n");
                if let Err(err) = std::fs::write(&progress_filename, &progress) {
                    eprintln!(
                        "An error occurred while saving download progress for {}: {:?}",
                        &filename, err
                    );
                }
            }
            ProgressManagerCommand::Stop => {
                break;
            }
        }
    }
}

fn get_progress(filename: &str) -> HashMap<u64, SegmentDownloadProgress> {
    let mut progress: HashMap<u64, SegmentDownloadProgress> = HashMap::new();
    let progress_filename = format!("{}.dlprogress", filename);
    for line in std::fs::read_to_string(progress_filename)
        .unwrap_or(String::new())
        .lines()
    {
        let line_parsed = line
            .trim()
            .split(" ")
            .map(|w| w.trim().parse::<u64>())
            .collect_vec();
        if line_parsed.len() != 3
            || line_parsed[0].is_err()
            || line_parsed[1].is_err()
            || line_parsed[2].is_err()
        {
            eprintln!(
                "Found unexpected line reading download progress of file {}: {}",
                &filename, &line
            );
            continue;
        }
        progress.insert(
            line_parsed[0].clone().unwrap(),
            SegmentDownloadProgress {
                offset: line_parsed[0].clone().unwrap(),
                size: line_parsed[1].clone().unwrap(),
                progress: line_parsed[2].clone().unwrap(),
            },
        );
    }
    progress
}

async fn get_filename_from_url(url: &str) -> Option<String> {
    // Attempt to get the filename
    let c = reqwest::Client::new();
    if let Ok(result) = c.head(url).send().await {
        let headers = result.headers();
        if let Some(content_disposition) = headers.get("Content-Disposition") {
            if let Ok(content_disposition) = content_disposition.to_str() {
                let re = Regex::new(r#"filename=['"]?([^'"]+)['"]?"#).unwrap();
                if let Some(captures) = re.captures(content_disposition) {
                    if captures.len() > 1 {
                        // Ensure there are no slashes in the filename...
                        let filename = captures[1].replace("/", "_").replace("\\", "_");
                        println!("Detected filename from Content-Disposition: {}", filename);
                        return Some(filename);
                    }
                }
            }
        }
    }

    if let Some(filename) = url.split('/').last() {
        if let Some(filename) = filename.split('?').nth(0) {
            return Some(filename.into());
        }
    }
    None
}

async fn download_file(
    filename: Option<String>,
    url: &str,
    connections: u64,
    options: &DownloadOptions,
) -> Result<()> {
    let mut filename: Option<String> = filename;
    if filename.is_none() {
        filename = get_filename_from_url(url).await;
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
    if response
        .headers()
        .get(reqwest::header::ACCEPT_RANGES)
        .unwrap_or(&reqwest::header::HeaderValue::from_static("none"))
        == "none"
    {
        return Err(anyhow::format_err!("Server doesn't seem to support partial requests (Accept-Ranges missing from headers). Aborting..."));
    }
    if let Some(content_length) = response.headers().get(reqwest::header::CONTENT_LENGTH) {
        let content_length: u64 = content_length.to_str()?.parse()?;
        let (tx, rx) = mpsc::channel::<ProgressManagerCommand>((connections + 1) as usize);
        let segment_size = (content_length as f64 / connections as f64).ceil() as u64;
        let segments = (0..=content_length - 1).step_by(segment_size as usize);
        let connections = segments.clone().count();
        let mut progress = get_progress(&filename);
        let file_exists = std::fs::metadata(&filename).is_ok();
        let mut is_resumed = false;
        if file_exists && progress.len() == connections {
            is_resumed = true;
            println!("Resuming download for file {}", &filename);
        } else {
            println!(
                "Starting download of file {}, connections={}, size is {} bytes",
                filename, connections, content_length
            );
            progress.clear();
            let _ = std::fs::remove_file(format!("{}.dlprogress", filename));
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
        }

        let mut downloaders = Vec::new();
        for segment_start in segments {
            let segment_end = (segment_start + segment_size - 1).clamp(0, content_length - 1);
            if is_resumed {
                downloaders.push(Arc::new(Mutex::new(
                    FileSegmentDownloader::new_with_progress(
                        &filename,
                        url,
                        segment_start,
                        segment_end - segment_start + 1,
                        progress
                            .get(&segment_start)
                            .expect(
                                &format!(
                                    "Existing progress does not contain segment offset {}",
                                    &segment_start
                                )
                                .to_string(),
                            )
                            .progress,
                        options,
                    ),
                )));
            } else {
                progress.insert(
                    segment_start,
                    SegmentDownloadProgress {
                        offset: segment_start,
                        size: segment_end - segment_start + 1,
                        progress: 0,
                    },
                );
                downloaders.push(Arc::new(Mutex::new(FileSegmentDownloader::new(
                    &filename,
                    url,
                    segment_start,
                    segment_end - segment_start + 1,
                    options,
                ))));
            }
        }
        let progress_manager = tokio::spawn(handle_segment_download_progress(
            filename.clone(),
            progress.clone(),
            rx,
        ));
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
            let tx = tx.clone();
            downloader_handles.push(tokio::task::spawn(async move {
                downloader.lock().await.download(Some(pb), tx).await
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
        if tx.send(ProgressManagerCommand::Stop).await.is_ok() {
            let _ = progress_manager.await;
        }
    } else {
        // Can't get content-length. Not supported
        return Err(anyhow::format_err!(
            "Couldn't get content length. Aborting..."
        ));
    }
    let _ = std::fs::remove_file(format!("{}.dlprogress", &filename));
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.num_connections < 1 {
        return Err(anyhow::format_err!(
            "Number of connections cannot be less than 1"
        ));
    }
    if args.target_total_chunk_size < 0.01 {
        return Err(anyhow::format_err!(
            "Target total chunk size should be at least 0.01"
        ));
    }
    download_file(
        args.filename,
        &args.url,
        args.num_connections,
        &DownloadOptions {
            chunk_size: (args.target_total_chunk_size * 1024.0 * 1024.0
                / args.num_connections as f64)
                .ceil() as u64,
            cookies: args.cookies,
            _headers: Vec::new(),
            retry_timeout: args.chunk_retry_timeout,
            retry_delay: args.chunk_retry_delay,
            new_client_per_request: args.new_client_per_request,
        },
    )
    .await?;

    Ok(())
}
