//! One HTTP(S) download per process. stdout is a versioned JSON Lines protocol.
use clap::Parser;
use fs2::FileExt;
use libdl::{DlError, DownloadCancellation, DownloadOptions, DownloadPhase};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, ACCEPT_ENCODING};
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, sync::mpsc};

#[derive(Parser)]
#[command(
    version,
    about = "HTTP downloader with JSON Lines progress and stdin cancellation"
)]
struct Cli {
    /// HTTP(S) URL. Prefer --request-stdin for signed URLs and credentials.
    source: Option<String>,
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Read the initial request as one JSON line from stdin.
    #[arg(long, conflicts_with_all = ["source", "output", "header", "sha256", "expected_size", "connections", "chunk_size", "no_resume", "overwrite"])]
    request_stdin: bool,
    /// Additional HTTP header, e.g. 'Authorization: Bearer token'. Repeatable.
    #[arg(short = 'H', long)]
    header: Vec<String>,
    #[arg(long)]
    sha256: Option<String>,
    #[arg(long)]
    expected_size: Option<u64>,
    #[arg(short = 'j', long)]
    connections: Option<usize>,
    /// Chunk size in bytes (65536..67108864).
    #[arg(long)]
    chunk_size: Option<u64>,
    #[arg(long)]
    no_resume: bool,
    #[arg(long)]
    overwrite: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    url: String,
    output: PathBuf,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    sha256: Option<String>,
    expected_size: Option<u64>,
    connections: Option<usize>,
    chunk_size: Option<u64>,
    #[serde(default = "enabled")]
    resume: bool,
    #[serde(default)]
    overwrite: bool,
}
fn enabled() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Control {
    command: String,
}

#[derive(Debug)]
struct Failure {
    code: String,
    message: &'static str,
    exit: u8,
    http_status: Option<u16>,
    retryable: bool,
}
impl Failure {
    fn config(message: &'static str) -> Self {
        Self {
            code: "invalid_request".into(),
            message,
            exit: 2,
            http_status: None,
            retryable: false,
        }
    }
    fn integrity() -> Self {
        Self {
            code: "integrity_mismatch".into(),
            message: "Downloaded bytes failed the expected size or SHA-256 check",
            exit: 4,
            http_status: None,
            retryable: false,
        }
    }
    fn cancelled() -> Self {
        Self {
            code: "cancelled".into(),
            message: "Download cancelled; resumable state retained",
            exit: 3,
            http_status: None,
            retryable: false,
        }
    }
}
impl From<io::Error> for Failure {
    fn from(_: io::Error) -> Self {
        Self {
            code: "io_error".into(),
            message: "A local file or pipe operation failed",
            exit: 1,
            http_status: None,
            retryable: false,
        }
    }
}
impl From<DlError> for Failure {
    fn from(error: DlError) -> Self {
        if matches!(error, DlError::Cancelled) {
            return Self::cancelled();
        }
        let status = match &error {
            DlError::HttpStatus(status) => Some(*status),
            DlError::Http(error) => error.status().map(|s| s.as_u16()),
            DlError::RateLimited { .. } => Some(429),
            _ => None,
        };
        // Never stringify transport errors: they may contain signed URLs or server echoes.
        Self {
            code: if let Some(status) = status {
                format!("http_{status}")
            } else if matches!(
                error,
                DlError::InvalidResponse(_) | DlError::InvalidState(_) | DlError::RangeMismatch
            ) {
                "invalid_response".into()
            } else {
                "download_failed".into()
            },
            message: "Download failed; inspect the error code and HTTP status",
            exit: 1,
            http_status: status,
            retryable: status.map_or(
                matches!(error, DlError::Http(_) | DlError::ServerError(_)),
                |s| s == 408 || s == 429 || s >= 500,
            ),
        }
    }
}

fn emit(mut value: Value) -> io::Result<()> {
    value["protocol"] = json!(1);
    let stdout = io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer(&mut out, &value)?;
    out.write_all(b"\n")?;
    out.flush()
}

// A plain thread is intentional. Tokio stdin uses a blocking task whose pending read
// can prevent runtime shutdown while Java keeps its stdin pipe open.
fn stdin_lines() -> mpsc::Receiver<Result<String, ()>> {
    let (tx, rx) = mpsc::channel(16);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut input = stdin.lock();
        loop {
            let mut bytes = Vec::new();
            let result = std::io::Read::take(&mut input, 65537).read_until(b'\n', &mut bytes);
            match result {
                Ok(0) => break,
                Ok(_) if bytes.len() <= 65536 => {
                    let line = String::from_utf8(bytes).map_err(|_| ());
                    let invalid = line.is_err();
                    if tx.blocking_send(line).is_err() || invalid {
                        break;
                    }
                }
                _ => {
                    let _ = tx.blocking_send(Err(()));
                    break;
                }
            }
        }
    });
    rx
}

fn validate(request: &Request) -> Result<HeaderMap, Failure> {
    let url = url::Url::parse(&request.url).map_err(|_| Failure::config("Invalid HTTP(S) URL"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Failure::config(
            "Use an HTTP(S) URL without userinfo or fragment",
        ));
    }
    if request.output.file_name().is_none() {
        return Err(Failure::config("Output must name a file"));
    }
    if request.connections.is_some_and(|n| !(1..=32).contains(&n)) {
        return Err(Failure::config("Connections must be between 1 and 32"));
    }
    if request
        .chunk_size
        .is_some_and(|n| !(65536..=67108864).contains(&n))
    {
        return Err(Failure::config(
            "Chunk size must be between 65536 and 67108864 bytes",
        ));
    }
    if request
        .sha256
        .as_ref()
        .is_some_and(|hash| hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(Failure::config(
            "SHA-256 must contain 64 hexadecimal digits",
        ));
    }
    let mut headers = HeaderMap::new();
    for (name, value) in &request.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| Failure::config("Invalid HTTP header name"))?;
        if matches!(
            name.as_str(),
            "range"
                | "if-range"
                | "content-length"
                | "host"
                | "accept-encoding"
                | "transfer-encoding"
        ) {
            return Err(Failure::config("Reserved HTTP header cannot be overridden"));
        }
        let mut value = HeaderValue::from_str(value)
            .map_err(|_| Failure::config("Invalid HTTP header value"))?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
    Ok(headers)
}

fn suffix(path: &Path, value: &str) -> PathBuf {
    let mut path = path.as_os_str().to_os_string();
    path.push(value);
    path.into()
}

async fn verify(
    path: &Path,
    request: &Request,
    cancellation: &DownloadCancellation,
) -> Result<(u64, String), Failure> {
    let mut file = tokio::fs::File::open(path).await?;
    let size = file.metadata().await?.len();
    if request
        .expected_size
        .is_some_and(|expected| expected != size)
    {
        return Err(Failure::integrity());
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let read = tokio::select! {
            result = file.read(&mut buffer) => result?,
            _ = cancellation.cancelled() => return Err(Failure::cancelled()),
        };
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = format!("{:x}", hasher.finalize());
    if request
        .sha256
        .as_ref()
        .is_some_and(|expected| !expected.eq_ignore_ascii_case(&digest))
    {
        return Err(Failure::integrity());
    }
    Ok((size, digest))
}

async fn transfer(request: Request, cancellation: DownloadCancellation) -> Result<Value, Failure> {
    let headers = validate(&request)?;
    if let Some(parent) = request
        .output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        tokio::fs::create_dir_all(parent).await?;
    }
    // Keep this sidecar: unlinking a locked file creates a lock race on Unix.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(suffix(&request.output, ".json-dl.lock"))?;
    lock.try_lock_exclusive()
        .map_err(|_| Failure::config("Output is locked by another download"))?;
    if request.output.exists() && !request.overwrite {
        return Err(Failure::config(
            "Output already exists; use overwrite explicitly",
        ));
    }
    let staging = suffix(&request.output, ".json-dl-part");
    let resume_key = request
        .sha256
        .as_ref()
        .map(|s| format!("sha256:{}", s.to_ascii_lowercase()))
        .unwrap_or_else(|| format!("url:{:x}", Sha256::digest(request.url.as_bytes())));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let options = DownloadOptions {
        connections: request.connections,
        chunk_size: request
            .chunk_size
            .unwrap_or(libdl::types::DEFAULT_CHUNK_SIZE),
        resume: request.resume && request.sha256.is_some(),
        overwrite: true,
        headers,
        cancellation: Some(cancellation.clone()),
        resume_key: Some(resume_key),
        same_origin_redirects: true,
        progress: Some(tx),
        ..DownloadOptions::default()
    };
    emit(json!({"event":"started","output":request.output}))?;
    let download = libdl::download_http(request.url.clone(), &staging, options);
    tokio::pin!(download);
    let mut last = Instant::now() - Duration::from_secs(1);
    let summary = loop {
        tokio::select! {
            biased;
            result = &mut download => break result.map_err(Failure::from)?,
            Some(progress) = rx.recv() => {
                if last.elapsed() >= Duration::from_millis(200) && progress.phase != DownloadPhase::Complete {
                    last = Instant::now();
                    emit(json!({"event":"progress","phase":progress.phase,
                        "downloaded":progress.downloaded_bytes,"total":progress.total_bytes,
                        "active_workers":progress.active_workers}))?;
                }
            }
        }
    };
    emit(json!({"event":"verifying"}))?;
    let (size, sha256) = verify(&staging, &request, &cancellation).await?;
    if cancellation.is_cancelled() {
        return Err(Failure::cancelled());
    }
    // Publish only after verification. A failed download never overwrites the old pack.
    if request.overwrite {
        tokio::fs::rename(&staging, &request.output).await?;
    } else {
        // hard_link is an atomic no-clobber publish on the same filesystem.
        tokio::fs::hard_link(&staging, &request.output).await?;
        // The destination is now complete; staging cleanup cannot undo publication.
        let _ = tokio::fs::remove_file(&staging).await;
    }
    drop(lock);
    Ok(
        json!({"event":"complete","output":request.output,"bytes":size,"sha256":sha256,"resumed":summary.resumed}),
    )
}

async fn run(cli: Cli) -> Result<Value, Failure> {
    let mut input = stdin_lines();
    let request = if cli.request_stdin {
        let line = tokio::time::timeout(Duration::from_secs(30), input.recv())
            .await
            .map_err(|_| Failure::config("Timed out waiting for the initial stdin request"))?
            .ok_or_else(|| Failure::config("Missing initial stdin request"))?
            .map_err(|_| Failure::config("Request must be UTF-8 JSON and at most 64 KiB"))?;
        serde_json::from_str::<Request>(&line)
            .map_err(|_| Failure::config("Invalid initial JSON request"))?
    } else {
        let mut headers = BTreeMap::new();
        for header in cli.header {
            let (name, value) = header
                .split_once(':')
                .ok_or_else(|| Failure::config("Header must use Name: Value syntax"))?;
            headers.insert(name.trim().to_string(), value.trim().to_string());
        }
        Request {
            url: cli
                .source
                .ok_or_else(|| Failure::config("URL is required"))?,
            output: cli
                .output
                .ok_or_else(|| Failure::config("Output is required"))?,
            headers,
            sha256: cli.sha256,
            expected_size: cli.expected_size,
            connections: cli.connections,
            chunk_size: cli.chunk_size,
            resume: !cli.no_resume,
            overwrite: cli.overwrite,
        }
    };
    let cancellation = DownloadCancellation::default();
    let token = cancellation.clone();
    let control = tokio::spawn(async move {
        if let Some(line) = input.recv().await {
            match line
                .ok()
                .and_then(|line| serde_json::from_str::<Control>(&line).ok())
            {
                Some(command) if command.command == "cancel" => {
                    token.cancel();
                    return Ok(());
                }
                _ => {
                    token.cancel();
                    return Err(Failure::config(
                        "Only a JSON cancel command is accepted after startup",
                    ));
                }
            }
        }
        Ok(()) // EOF is not cancellation; a parent may close stdin after sending its request.
    });
    let mut result = transfer(request, cancellation.clone()).await;
    // Cancellation wakes transfer before the control task necessarily finishes.
    // Join that task so its protocol error takes precedence over cancellation.
    if cancellation.is_cancelled() || control.is_finished() {
        if let Ok(Err(error)) = control.await {
            result = Err(error);
        }
    } else {
        control.abort();
    }
    result
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let informational = !error.use_stderr();
            let value = if informational {
                json!({"event":"info","text":error.to_string()})
            } else {
                json!({"event":"error","code":"invalid_arguments","message":"Invalid arguments; use --help","retryable":false,"refresh_url":false})
            };
            let _ = emit(value);
            return ExitCode::from(if informational { 0 } else { 2 });
        }
    };
    match run(cli).await {
        Ok(event) => {
            if emit(event).is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(error) => {
            let event = if error.exit == 3 {
                "cancelled"
            } else {
                "error"
            };
            let _ = emit(
                json!({"event":event,"code":error.code,"message":error.message,
                "http_status":error.http_status,"retryable":error.retryable,
                "refresh_url":matches!(error.http_status, Some(401 | 403))}),
            );
            ExitCode::from(error.exit)
        }
    }
}
