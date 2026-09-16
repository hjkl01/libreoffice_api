use anyhow::{anyhow, Context, Result};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Multipart, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use std::{env, path::Path, sync::Arc, time::Duration};
use tokio::{fs, process::Command, sync::Semaphore, time::timeout};
use tracing::{error, info};
use utoipa::{OpenApi, ToSchema};
use utoipa_swagger_ui::SwaggerUi;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    semaphore: Arc<Semaphore>,
    max_upload_size: usize,
    timeout: Duration,
}
#[derive(Debug)]
struct ApiError(anyhow::Error);
impl<E> From<E> for ApiError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        error!(error = ?self.0, "conversion request failed");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("conversion failed: {}", self.0),
        )
            .into_response()
    }
}

#[allow(dead_code)]
#[derive(ToSchema)]
struct ConvertRequest {
    #[schema(value_type = String, format = Binary)]
    file: String,
    #[schema(default = "pdf", example = "pdf")]
    format: Option<String>,
    #[schema(default = 1, example = 3)]
    slide: Option<u32>,
}
#[derive(OpenApi)]
#[openapi(
    info(
        title = "LibreOffice Conversion API",
        version = "0.1.0",
        description = "HTTP API for converting documents, spreadsheets and presentations through LibreOffice."
    ),
    paths(health, convert),
    components(schemas(ConvertRequest))
)]
struct ApiDoc;

#[utoipa::path(get, path = "/health", responses((status = 200, description = "Service is healthy", body = String, content_type = "text/plain")))]
async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok")
}

#[utoipa::path(post, path = "/convert", request_body(content = ConvertRequest, content_type = "multipart/form-data"), responses((status = 200, description = "Converted file", content_type = "application/octet-stream", body = String), (status = 400, description = "Invalid request"), (status = 500, description = "Conversion failed")))]
async fn convert(
    State(state): State<AppState>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let _permit = state
        .semaphore
        .acquire()
        .await
        .context("failed to acquire conversion slot")?;
    let job_dir = env::temp_dir().join(format!("libreoffice-api-{}", Uuid::new_v4()));
    let profile_dir = job_dir.join("profile");
    let input_dir = job_dir.join("input");
    let output_dir = job_dir.join("output");
    fs::create_dir_all(&input_dir).await?;
    fs::create_dir_all(&output_dir).await?;
    let result = convert_inner(
        &state,
        &mut multipart,
        &input_dir,
        &output_dir,
        &profile_dir,
    )
    .await;
    let _ = fs::remove_dir_all(&job_dir).await;
    result
}

async fn convert_inner(
    state: &AppState,
    multipart: &mut Multipart,
    input_dir: &Path,
    output_dir: &Path,
    profile_dir: &Path,
) -> Result<Response, ApiError> {
    let mut original_name = None;
    let mut format = String::from("pdf");
    let mut slide = None;
    let mut input_path = None;

    while let Some(field) = multipart.next_field().await? {
        let name = field.name().unwrap_or("").to_owned();
        if name == "format" {
            format = field
                .text()
                .await?
                .trim()
                .trim_start_matches('.')
                .to_ascii_lowercase();
            if format.is_empty()
                || !format
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
            {
                return Err(anyhow!("invalid output format").into());
            }
            if format.len() > 32 {
                return Err(anyhow!("output format is too long").into());
            }
        } else if name == "slide" {
            let value = field.text().await?;
            let value = value.trim();
            let number = value
                .parse::<u32>()
                .map_err(|_| anyhow!("slide must be a positive integer"))?;
            if number == 0 {
                return Err(anyhow!("slide must be a positive integer").into());
            }
            slide = Some(number);
        } else if name == "file" {
            let filename = field.file_name().unwrap_or("input");
            let original_basename = Path::new(filename)
                .file_name()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("input")
                .to_owned();
            let extension = Path::new(&original_basename)
                .extension()
                .and_then(|s| s.to_str())
                .filter(|s| !s.is_empty())
                .unwrap_or("bin");
            let temp_name = format!("input.{}", extension);
            let path = input_dir.join(temp_name);
            let data = field.bytes().await?;
            if data.len() > state.max_upload_size {
                return Err(anyhow!("uploaded file exceeds MAX_UPLOAD_SIZE").into());
            }
            fs::write(&path, &data).await?;
            original_name = Some(original_basename);
            input_path = Some(path);
        }
    }

    let input_path = input_path.ok_or_else(|| anyhow!("multipart field 'file' is required"))?;
    let original_name = original_name.unwrap_or_else(|| "input".into());
    let stem = Path::new(&original_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");

    let image_format = matches!(format.as_str(), "png" | "jpg" | "jpeg" | "webp" | "svg");
    if slide.is_some() && !image_format {
        return Err(anyhow!("slide can only be used when converting to an image format").into());
    }

    let generated_name = format!("input.{format}");
    let generated_path = output_dir.join(&generated_name);
    let profile_url = format!("file://{}", profile_dir.to_string_lossy());
    let mut cmd = Command::new("libreoffice");
    cmd.arg("--headless")
        .arg("--nologo")
        .arg("--nodefault")
        .arg("--nofirststartwizard")
        .arg(format!("-env:UserInstallation={profile_url}"));

    if image_format && slide.is_some() {
        let filter = format!("impress_{}_Export", image_filter_format(&format));
        let params = format!(
            "{{\"PageNumber\":{{\"type\":\"long\",\"value\":\"{}\"}}}}",
            slide.unwrap()
        );
        cmd.arg("--convert-to")
            .arg(format!("{format}:{filter}:{params}"));
    } else {
        cmd.arg("--convert-to").arg(&format);
    }

    cmd.arg("--outdir")
        .arg(output_dir)
        .arg(&input_path)
        .kill_on_drop(true);
    info!(
        file = %original_name,
        format = %format,
        slide = ?slide,
        "starting LibreOffice conversion"
    );

    let output = timeout(state.timeout, cmd.output())
        .await
        .map_err(|_| anyhow!("LibreOffice conversion timed out"))??;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(anyhow!(
            "LibreOffice exited with {}: {} {}",
            output.status,
            stderr.trim(),
            stdout.trim()
        )
        .into());
    }

    if !fs::try_exists(&generated_path).await.unwrap_or(false) {
        return Err(anyhow!("LibreOffice completed but output file was not created").into());
    }

    let download_name = format!("{stem}.{format}");
    let final_path = output_dir.join(&download_name);
    if final_path != generated_path {
        fs::rename(&generated_path, &final_path).await?;
    }
    let bytes = fs::read(&final_path).await?;
    let content_type = content_type_for(&format);
    let encoded_name = percent_encode_filename(&download_name);
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename*=UTF-8''{encoded_name}"
        ))?,
    );
    Ok(response)
}

fn image_filter_format(format: &str) -> &'static str {
    match format {
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "webp" => "webp",
        "svg" => "svg",
        _ => unreachable!(),
    }
}

fn percent_encode_filename(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for byte in name.as_bytes() {
        if matches!(
            *byte,
            b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'!'
                | b'#'
                | b'$'
                | b'&'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        ) {
            encoded.push(*byte as char);
        } else {
            encoded.push('%');
            encoded.push(hex_digit(byte >> 4));
            encoded.push(hex_digit(byte & 0x0f));
        }
    }
    encoded
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        10..=15 => (b'A' + value - 10) as char,
        _ => unreachable!(),
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn content_type_for(format: &str) -> &'static str {
    match format {
        "pdf" => "application/pdf",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "doc" => "application/msword",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "ppt" => "application/vnd.ms-powerpoint",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "odt" => "application/vnd.oasis.opendocument.text",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        "odp" => "application/vnd.oasis.opendocument.presentation",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "txt" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(env::var("RUST_LOG").unwrap_or_else(|_| "info".into()))
        .init();
    let port = env_usize("PORT", 8080) as u16;
    let max_upload_size = env_usize("MAX_UPLOAD_SIZE", 100 * 1024 * 1024);
    let concurrency = env_usize("LO_MAX_CONCURRENCY", 2).max(1);
    let timeout_secs = env_usize("CONVERSION_TIMEOUT_SECS", 300).max(1);
    let state = AppState {
        semaphore: Arc::new(Semaphore::new(concurrency)),
        max_upload_size,
        timeout: Duration::from_secs(timeout_secs as u64),
    };
    let app = Router::new()
        .route("/health", get(health))
        .route("/convert", post(convert))
        .merge(SwaggerUi::new("/docs").url("/api-docs/openapi.json", ApiDoc::openapi()))
        .layer(DefaultBodyLimit::max(max_upload_size))
        .with_state(state);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(%addr, concurrency, "libreoffice_api started");
    axum::serve(listener, app).await?;
    Ok(())
}
