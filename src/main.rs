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

#[derive(ToSchema)]
struct ConvertRequest {
    #[schema(value_type = String, format = Binary)]
    file: String,
    #[schema(default = "pdf", example = "pdf")]
    format: Option<String>,
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
        } else if name == "file" {
            let filename = field.file_name().unwrap_or("input");
            let safe_name = sanitize_filename(filename);
            let path = input_dir.join(&safe_name);
            let data = field.bytes().await?;
            if data.len() > state.max_upload_size {
                return Err(anyhow!("uploaded file exceeds MAX_UPLOAD_SIZE").into());
            }
            fs::write(&path, &data).await?;
            original_name = Some(safe_name);
            input_path = Some(path);
        }
    }
    let input_path = input_path.ok_or_else(|| anyhow!("multipart field 'file' is required"))?;
    let original_name = original_name.unwrap_or_else(|| "input".into());
    let stem = Path::new(&original_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    let expected = output_dir.join(format!("{stem}.{format}"));
    let profile_url = format!("file://{}", profile_dir.to_string_lossy());
    let mut cmd = Command::new("libreoffice");
    cmd.arg("--headless")
        .arg("--nologo")
        .arg("--nodefault")
        .arg("--nofirststartwizard")
        .arg(format!("-env:UserInstallation={profile_url}"))
        .arg("--convert-to")
        .arg(&format)
        .arg("--outdir")
        .arg(output_dir)
        .arg(&input_path)
        .kill_on_drop(true);
    info!(file = %original_name, format = %format, "starting LibreOffice conversion");
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
    if !fs::try_exists(&expected).await.unwrap_or(false) {
        return Err(anyhow!("LibreOffice completed but output file was not created").into());
    }
    let bytes = fs::read(&expected).await?;
    let content_type = content_type_for(&format);
    let download_name = format!("{stem}.{format}");
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"{}\"",
            download_name.replace('"', "_")
        ))?,
    );
    Ok(response)
}
fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
fn sanitize_filename(name: &str) -> String {
    let base = Path::new(name)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("input");
    let safe: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "input".into()
    } else {
        safe
    }
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
