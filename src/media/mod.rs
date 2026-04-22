use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tonic::{metadata::MetadataValue, transport::Channel, Request as GrpcRequest, Status};

use crate::pb::service::media::media_service_client::MediaServiceClient;
use crate::pb::service::media::{
    CompleteUploadRequest as PbCompleteUploadRequest,
    DeleteFileRequest as PbDeleteFileRequest,
    GetFileDownloadUrlRequest as PbGetFileDownloadUrlRequest,
    GetFileRequest as PbGetFileRequest,
    InitUploadRequest as PbInitUploadRequest,
    ListFilesRequest as PbListFilesRequest,
};
use crate::pb::shared::media::{MediaFileStatus, MediaUploadStatus};
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/uploads/init", post(init_upload))
        .route("/uploads/complete", post(complete_upload))
        .route("/files", get(list_files))
        .route("/files/{id}", get(get_file))
        .route("/files/{id}", delete(delete_file))
        .route("/files/{id}/download-url", get(get_file_download_url))
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct InitUploadRequest {
    file_name: String,
    content_type: String,
    size: i64,
    org_id: Option<String>,
}

#[derive(Deserialize)]
struct CompleteUploadRequest {
    upload_id: String,
}

#[derive(Deserialize)]
struct ListFilesQuery {
    org_id: Option<String>,
    limit: Option<i32>,
    offset: Option<i32>,
}

#[derive(Serialize)]
struct InitUploadResponse {
    upload_id: String,
    file_id: String,
    bucket: String,
    object_key: String,
    presigned_url: String,
    expires_at: i64,
    required_headers: serde_json::Value,
}

#[derive(Serialize)]
struct CompleteUploadResponse {
    file_id: String,
    status: String,
    object_key: String,
    bucket: String,
    etag: String,
    confirmed_size: i64,
    public_url: String,
}

#[derive(Serialize)]
struct MediaFileResponse {
    file_id: String,
    bucket: String,
    object_key: String,
    content_type: String,
    original_name: String,
    size: u64,
    status: String,
    org_id: String,
    public_url: String,
    created_at: i64,
}

#[derive(Serialize)]
struct ListFilesResponse {
    files: Vec<MediaFileResponse>,
    total: i32,
}

#[derive(Serialize)]
struct FileDownloadUrlResponse {
    file_id: String,
    download_url: String,
    expires_at: i64,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn map_status(status: Status) -> (StatusCode, Json<ErrorResponse>) {
    let (http, envelope) = philand_error::http_error_from_tonic_status(&status);
    (http, Json(envelope))
}

async fn client(state: &AppState) -> ApiResult<MediaServiceClient<Channel>> {
    MediaServiceClient::connect(state.media_grpc_url.clone())
        .await
        .map_err(|e| map_status(Status::internal(e.to_string())))
}

fn with_auth<T>(headers: &HeaderMap, req: T) -> ApiResult<GrpcRequest<T>> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| map_status(Status::unauthenticated("Missing Authorization header")))?;

    let mut grpc_req = GrpcRequest::new(req);
    let value = MetadataValue::try_from(auth)
        .map_err(|_| map_status(Status::unauthenticated("Invalid Authorization header")))?;
    grpc_req.metadata_mut().insert("authorization", value);
    Ok(grpc_req)
}

fn upload_status_string(v: i32) -> String {
    match MediaUploadStatus::try_from(v).unwrap_or(MediaUploadStatus::MusNone) {
        MediaUploadStatus::MusInit => "init",
        MediaUploadStatus::MusUploading => "uploading",
        MediaUploadStatus::MusReady => "ready",
        MediaUploadStatus::MusFailed => "failed",
        MediaUploadStatus::MusExpired => "expired",
        MediaUploadStatus::MusNone => "none",
    }
    .to_string()
}

fn file_status_string(v: i32) -> String {
    match MediaFileStatus::try_from(v).unwrap_or(MediaFileStatus::MfsNone) {
        MediaFileStatus::MfsReady => "ready",
        MediaFileStatus::MfsDeleted => "deleted",
        MediaFileStatus::MfsNone => "none",
    }
    .to_string()
}

fn map_file_proto(file: crate::pb::shared::media::MediaFile) -> MediaFileResponse {
    let created_at = file.base.as_ref().map(|b| b.created_at).unwrap_or_default();
    let file_id = file.base.map(|b| b.id).unwrap_or_default();
    MediaFileResponse {
        file_id,
        bucket: file.bucket,
        object_key: file.object_key,
        content_type: file.content_type,
        original_name: file.original_name,
        size: file.size.max(0) as u64,
        status: file_status_string(file.file_status),
        org_id: file.org_id,
        public_url: file.public_url,
        created_at,
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn init_upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<InitUploadRequest>,
) -> ApiResult<(StatusCode, Json<InitUploadResponse>)> {
    let mut c = client(&state).await?;
    let resp = c
        .init_upload(with_auth(
            &headers,
            PbInitUploadRequest {
                file_name: body.file_name,
                content_type: body.content_type,
                size: body.size,
                org_id: body.org_id.unwrap_or_default(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();

    Ok((
        StatusCode::CREATED,
        Json(InitUploadResponse {
            upload_id: resp.upload_id,
            file_id: resp.file_id,
            bucket: resp.bucket,
            object_key: resp.object_key,
            presigned_url: resp.presigned_url,
            expires_at: resp.expires_at,
            required_headers: serde_json::json!({
                "content-type": resp.required_content_type,
            }),
        }),
    ))
}

async fn complete_upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CompleteUploadRequest>,
) -> ApiResult<Json<CompleteUploadResponse>> {
    let mut c = client(&state).await?;
    let resp = c
        .complete_upload(with_auth(
            &headers,
            PbCompleteUploadRequest {
                upload_id: body.upload_id,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();

    Ok(Json(CompleteUploadResponse {
        file_id: resp.file_id,
        status: upload_status_string(resp.upload_status),
        object_key: resp.object_key,
        bucket: resp.bucket,
        etag: resp.etag,
        confirmed_size: resp.confirmed_size,
        public_url: resp.public_url,
    }))
}

async fn list_files(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<ListFilesQuery>,
) -> ApiResult<Json<ListFilesResponse>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_files(with_auth(
            &headers,
            PbListFilesRequest {
                org_id: params.org_id.unwrap_or_default(),
                limit: params.limit.unwrap_or(20),
                offset: params.offset.unwrap_or(0),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();

    Ok(Json(ListFilesResponse {
        files: resp.files.into_iter().map(map_file_proto).collect(),
        total: resp.total,
    }))
}

async fn get_file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(file_id): Path<String>,
) -> ApiResult<Json<MediaFileResponse>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_file(with_auth(&headers, PbGetFileRequest { file_id })?)
        .await
        .map_err(map_status)?
        .into_inner();

    let file = resp
        .file
        .ok_or_else(|| map_status(Status::not_found("file not found")))?;

    Ok(Json(map_file_proto(file)))
}

async fn delete_file(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(file_id): Path<String>,
) -> ApiResult<StatusCode> {
    let mut c = client(&state).await?;
    c.delete_file(with_auth(&headers, PbDeleteFileRequest { file_id })?)
        .await
        .map_err(map_status)?;

    Ok(StatusCode::NO_CONTENT)
}

async fn get_file_download_url(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(file_id): Path<String>,
) -> ApiResult<Json<FileDownloadUrlResponse>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_file_download_url(with_auth(
            &headers,
            PbGetFileDownloadUrlRequest { file_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();

    Ok(Json(FileDownloadUrlResponse {
        file_id: resp.file_id,
        download_url: resp.download_url,
        expires_at: resp.expires_at,
    }))
}
