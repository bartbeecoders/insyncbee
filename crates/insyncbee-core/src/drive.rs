//! Google Drive v3 REST API client.
//!
//! The [`DriveClient`] trait is the seam the sync engine speaks to. The real
//! implementation that talks to Google over HTTPS is [`HttpDriveClient`];
//! tests provide their own in-memory `FakeDriveClient` (see
//! `crates/insyncbee-core/tests/common/mod.rs`).

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::auth::AuthManager;

/// Called on every byte-progress update during a resumable upload.
/// Arguments: the local file path, bytes uploaded so far, total bytes.
/// Invoked once at the start with `0` and again after every successful
/// chunk; the last call has `bytes_uploaded == total_bytes`.
pub type ProgressCallback = Arc<dyn Fn(&Path, u64, u64) + Send + Sync>;

const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";

/// Files at or below this size go via the single-shot multipart upload.
/// Anything larger is streamed via the resumable protocol so we never load
/// the whole file into RAM and can handle GB-scale uploads.
const RESUMABLE_THRESHOLD_BYTES: u64 = 4 * 1024 * 1024;

/// Per-PUT chunk size for resumable uploads. Google requires non-final
/// chunks to be a multiple of 256 KiB; 8 MiB is the common sweet spot
/// (few round-trips, bounded RAM).
const UPLOAD_CHUNK_BYTES: usize = 8 * 1024 * 1024;

/// Fields requested back after a successful upload — kept identical between
/// the multipart and resumable paths so the resulting [`DriveFile`] is
/// indistinguishable.
const UPLOAD_FIELDS: &str = "id,name,mimeType,md5Checksum,size,modifiedTime,parents";

/// The subset of Google Drive operations the sync engine depends on.
///
/// Implementations must be `Send + Sync` so the engine can drive them from
/// any tokio task. Real callers use [`HttpDriveClient`]; tests use their own
/// in-memory fake.
#[async_trait]
pub trait DriveClient: Send + Sync {
    /// List every file/folder directly inside `folder_id` (handles pagination).
    async fn list_all_files(&self, folder_id: &str) -> anyhow::Result<Vec<DriveFile>>;

    /// Fetch metadata for a single file by ID.
    async fn get_file(&self, file_id: &str) -> anyhow::Result<DriveFile>;

    /// Stream the contents of `file_id` to `dest`.
    async fn download_file(&self, file_id: &str, dest: &Path) -> anyhow::Result<()>;

    /// Upload `local_path` as a new child of `parent_id`, named `name`.
    async fn upload_file(
        &self,
        parent_id: &str,
        name: &str,
        local_path: &Path,
    ) -> anyhow::Result<DriveFile>;

    /// Replace the contents of `file_id` with `local_path`.
    async fn update_file(&self, file_id: &str, local_path: &Path) -> anyhow::Result<DriveFile>;

    /// Create an empty folder named `name` under `parent_id`.
    async fn create_folder(&self, parent_id: &str, name: &str) -> anyhow::Result<DriveFile>;

    /// Trash (soft-delete) `file_id`.
    async fn trash_file(&self, file_id: &str) -> anyhow::Result<()>;
}

/// HTTPS-backed implementation of [`DriveClient`] that talks to Google Drive v3.
pub struct HttpDriveClient {
    http: reqwest::Client,
    auth: AuthManager,
    account_id: String,
    progress: Option<ProgressCallback>,
}

impl HttpDriveClient {
    pub fn new(auth: AuthManager, account_id: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            auth,
            account_id,
            progress: None,
        }
    }

    /// Attach a callback that fires on every chunk completion during a
    /// resumable upload. Use this from the GUI to drive a progress bar;
    /// the daemon (which has no UI) leaves it unset.
    pub fn with_progress_callback(mut self, cb: ProgressCallback) -> Self {
        self.progress = Some(cb);
        self
    }

    async fn token(&self) -> anyhow::Result<String> {
        self.auth.get_valid_token(&self.account_id).await
    }

    /// List files in a folder.
    pub async fn list_files(
        &self,
        folder_id: &str,
        page_token: Option<&str>,
    ) -> anyhow::Result<FileListResponse> {
        let token = self.token().await?;
        let query = format!("'{folder_id}' in parents and trashed = false");

        let mut req = self
            .http
            .get(format!("{DRIVE_API}/files"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .query(&[
                ("q", query.as_str()),
                ("fields", "nextPageToken,files(id,name,mimeType,md5Checksum,size,modifiedTime,parents)"),
                ("pageSize", "1000"),
                ("orderBy", "name"),
            ]);

        if let Some(pt) = page_token {
            req = req.query(&[("pageToken", pt)]);
        }

        let resp = req.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Drive files.list {status}: {}",
                extract_drive_error(&body).unwrap_or(body)
            );
        }
        let body: FileListResponse = resp.json().await?;
        Ok(body)
    }

    /// List ALL files in a folder (paginated).
    pub async fn list_all_files(&self, folder_id: &str) -> anyhow::Result<Vec<DriveFile>> {
        let mut all_files = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let resp = self.list_files(folder_id, page_token.as_deref()).await?;
            all_files.extend(resp.files);
            match resp.next_page_token {
                Some(token) => page_token = Some(token),
                None => break,
            }
        }

        Ok(all_files)
    }

    /// Get metadata for a single file.
    pub async fn get_file(&self, file_id: &str) -> anyhow::Result<DriveFile> {
        let token = self.token().await?;
        let resp = self
            .http
            .get(format!("{DRIVE_API}/files/{file_id}"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .query(&[("fields", "id,name,mimeType,md5Checksum,size,modifiedTime,parents")])
            .send()
            .await?
            .error_for_status()?;
        let file: DriveFile = resp.json().await?;
        Ok(file)
    }

    /// Download a file to a local path.
    pub async fn download_file(&self, file_id: &str, dest: &Path) -> anyhow::Result<()> {
        let token = self.token().await?;
        let resp = self
            .http
            .get(format!("{DRIVE_API}/files/{file_id}"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .query(&[("alt", "media")])
            .send()
            .await?
            .error_for_status()?;

        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let mut file = tokio::fs::File::create(dest).await?;
        let bytes = resp.bytes().await?;
        file.write_all(&bytes).await?;
        file.flush().await?;

        Ok(())
    }

    /// Upload `local_path` as a new child of `parent_id`. Picks the
    /// multipart fast path for small files and switches to a streaming
    /// resumable upload above [`RESUMABLE_THRESHOLD_BYTES`] so multi-GB
    /// files don't OOM the daemon and don't hit Drive's multipart size cap.
    pub async fn upload_file(
        &self,
        parent_id: &str,
        name: &str,
        local_path: &Path,
    ) -> anyhow::Result<DriveFile> {
        let size = tokio::fs::metadata(local_path).await?.len();
        let mime = mime_guess::from_path(local_path)
            .first_or_octet_stream()
            .to_string();
        let metadata = serde_json::json!({
            "name": name,
            "parents": [parent_id],
        });

        if size <= RESUMABLE_THRESHOLD_BYTES {
            self.multipart_upload(reqwest::Method::POST,
                format!("{UPLOAD_API}/files"),
                Some(metadata),
                local_path,
                &mime,
            ).await
        } else {
            self.resumable_upload(reqwest::Method::POST,
                format!("{UPLOAD_API}/files"),
                Some(metadata),
                local_path,
                size,
                &mime,
            ).await
        }
    }

    /// Replace the contents of `file_id`. Same multipart-vs-resumable
    /// dispatch as [`upload_file`].
    pub async fn update_file(
        &self,
        file_id: &str,
        local_path: &Path,
    ) -> anyhow::Result<DriveFile> {
        let size = tokio::fs::metadata(local_path).await?.len();
        let mime = mime_guess::from_path(local_path)
            .first_or_octet_stream()
            .to_string();

        if size <= RESUMABLE_THRESHOLD_BYTES {
            // Plain media PATCH — no metadata change, just bytes.
            let token = self.token().await?;
            let content = tokio::fs::read(local_path).await?;
            let resp = self
                .http
                .patch(format!("{UPLOAD_API}/files/{file_id}?uploadType=media&fields={UPLOAD_FIELDS}"))
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header(CONTENT_TYPE, &mime)
                .body(content)
                .send()
                .await?;
            check_success(resp, "Drive update_file").await
        } else {
            self.resumable_upload(reqwest::Method::PATCH,
                format!("{UPLOAD_API}/files/{file_id}"),
                None,
                local_path,
                size,
                &mime,
            ).await
        }
    }

    /// Single-shot multipart upload — one HTTP round trip with metadata and
    /// bytes glued together. Only used for files at or below
    /// [`RESUMABLE_THRESHOLD_BYTES`]; larger ones go through
    /// [`Self::resumable_upload`].
    async fn multipart_upload(
        &self,
        method: reqwest::Method,
        url: String,
        metadata: Option<serde_json::Value>,
        local_path: &Path,
        mime: &str,
    ) -> anyhow::Result<DriveFile> {
        let token = self.token().await?;
        let content = tokio::fs::read(local_path).await?;
        let metadata = metadata.unwrap_or_else(|| serde_json::json!({}));

        let boundary = "insyncbee_boundary";
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n").as_bytes());
        body.extend_from_slice(serde_json::to_string(&metadata)?.as_bytes());
        body.extend_from_slice(format!("\r\n--{boundary}\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
        body.extend_from_slice(&content);
        body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());

        let resp = self
            .http
            .request(method, format!("{url}?uploadType=multipart&fields={UPLOAD_FIELDS}"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, format!("multipart/related; boundary={boundary}"))
            .body(body)
            .send()
            .await?;
        check_success(resp, "Drive multipart upload").await
    }

    /// Streaming resumable upload. Step 1 starts a session and gets back a
    /// `Location` URL; step 2 PUTs 8 MiB chunks (the last one shorter). A
    /// 308 means "keep going", a 200/201 means "done — here's the file
    /// metadata". We never read more than one chunk into RAM, so multi-GB
    /// files are fine.
    async fn resumable_upload(
        &self,
        method: reqwest::Method,
        url: String,
        metadata: Option<serde_json::Value>,
        local_path: &Path,
        total_size: u64,
        mime: &str,
    ) -> anyhow::Result<DriveFile> {
        let token = self.token().await?;
        let init_url = format!("{url}?uploadType=resumable&fields={UPLOAD_FIELDS}");

        // The metadata body for an update with no metadata change must still
        // be valid JSON — `{}` is the documented placeholder.
        let body_json = serde_json::to_vec(&metadata.unwrap_or_else(|| serde_json::json!({})))?;

        let init_resp = self
            .http
            .request(method, &init_url)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json; charset=UTF-8")
            .header("X-Upload-Content-Type", mime)
            .header("X-Upload-Content-Length", total_size.to_string())
            .body(body_json)
            .send()
            .await?;

        if !init_resp.status().is_success() {
            let status = init_resp.status();
            let body = init_resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "Drive resumable init {status}: {}",
                extract_drive_error(&body).unwrap_or(body)
            );
        }

        let session_url = init_resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| anyhow::anyhow!("Drive resumable init: no Location header"))?
            .to_string();

        // Announce the upload at 0% so the UI can render a bar before the
        // first (potentially slow) chunk PUT completes.
        if let Some(cb) = &self.progress {
            cb(local_path, 0, total_size);
        }

        let mut file = tokio::fs::File::open(local_path).await?;
        let mut buf = vec![0u8; UPLOAD_CHUNK_BYTES];
        let mut offset: u64 = 0;

        loop {
            // Fill the buffer up to chunk size or EOF, whichever comes first.
            // A single `read` may return short, so loop until full or EOF.
            let mut filled = 0usize;
            while filled < buf.len() {
                let n = file.read(&mut buf[filled..]).await?;
                if n == 0 {
                    break;
                }
                filled += n;
            }

            if filled == 0 {
                // The file ended exactly on a chunk boundary on the previous
                // iteration AND we already returned. Reaching here means the
                // file shrank under us — bail rather than send a 0-byte PUT.
                anyhow::bail!(
                    "Drive resumable upload: file shorter than expected ({offset}/{total_size})"
                );
            }

            let chunk_end = offset + filled as u64 - 1;
            let is_final = offset + filled as u64 >= total_size;
            let content_range = format!("bytes {offset}-{chunk_end}/{total_size}");

            tracing::debug!(
                "Resumable PUT {content_range} ({} of {} bytes, final={is_final})",
                offset + filled as u64,
                total_size
            );

            let resp = self
                .http
                .put(&session_url)
                .header(CONTENT_TYPE, mime)
                .header("Content-Range", &content_range)
                .body(buf[..filled].to_vec())
                .send()
                .await?;

            let status = resp.status();
            if is_final {
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    anyhow::bail!(
                        "Drive resumable final chunk {status}: {}",
                        extract_drive_error(&body).unwrap_or(body)
                    );
                }
                if let Some(cb) = &self.progress {
                    cb(local_path, total_size, total_size);
                }
                let file: DriveFile = resp.json().await?;
                return Ok(file);
            }

            // Drive uses the non-standard 308 "Resume Incomplete" between
            // chunks. Anything else is fatal — we don't yet implement
            // mid-upload retry, so a 5xx aborts the whole upload.
            if status.as_u16() != 308 {
                let body = resp.text().await.unwrap_or_default();
                anyhow::bail!(
                    "Drive resumable chunk {status}: {}",
                    extract_drive_error(&body).unwrap_or(body)
                );
            }
            offset += filled as u64;
            if let Some(cb) = &self.progress {
                cb(local_path, offset, total_size);
            }
        }
    }

    /// Create a folder on Drive.
    pub async fn create_folder(
        &self,
        parent_id: &str,
        name: &str,
    ) -> anyhow::Result<DriveFile> {
        let token = self.token().await?;
        let metadata = serde_json::json!({
            "name": name,
            "mimeType": "application/vnd.google-apps.folder",
            "parents": [parent_id],
        });

        let resp = self
            .http
            .post(format!("{DRIVE_API}/files?fields=id,name,mimeType,modifiedTime,parents"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .json(&metadata)
            .send()
            .await?
            .error_for_status()?;

        let file: DriveFile = resp.json().await?;
        Ok(file)
    }

    /// Delete a file (move to trash).
    pub async fn trash_file(&self, file_id: &str) -> anyhow::Result<()> {
        let token = self.token().await?;
        self.http
            .patch(format!("{DRIVE_API}/files/{file_id}"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .json(&serde_json::json!({"trashed": true}))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// Get the start page token for the Changes API.
    pub async fn get_start_page_token(&self) -> anyhow::Result<String> {
        let token = self.token().await?;
        let resp = self
            .http
            .get(format!("{DRIVE_API}/changes/startPageToken"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .send()
            .await?
            .error_for_status()?;

        #[derive(Deserialize)]
        struct Resp {
            #[serde(rename = "startPageToken")]
            start_page_token: String,
        }
        let r: Resp = resp.json().await?;
        Ok(r.start_page_token)
    }

    /// List changes since the given page token.
    pub async fn list_changes(&self, page_token: &str) -> anyhow::Result<ChangesResponse> {
        let token = self.token().await?;
        let resp = self
            .http
            .get(format!("{DRIVE_API}/changes"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .query(&[
                ("pageToken", page_token),
                ("fields", "nextPageToken,newStartPageToken,changes(fileId,removed,file(id,name,mimeType,md5Checksum,size,modifiedTime,parents,trashed))"),
                ("pageSize", "1000"),
                ("includeRemoved", "true"),
                ("spaces", "drive"),
            ])
            .send()
            .await?
            .error_for_status()?;

        let body: ChangesResponse = resp.json().await?;
        Ok(body)
    }

    /// Get "About" info (storage quota, user info).
    pub async fn get_about(&self) -> anyhow::Result<AboutResponse> {
        let token = self.token().await?;
        let resp = self
            .http
            .get(format!("{DRIVE_API}/about"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .query(&[("fields", "user,storageQuota")])
            .send()
            .await?
            .error_for_status()?;
        let about: AboutResponse = resp.json().await?;
        Ok(about)
    }
}

// ── DriveClient impl for HttpDriveClient ─────────────────────────────
// Delegates to the inherent methods above. Rust prefers inherent methods
// over trait methods at call sites, so `self.list_all_files(...)` inside
// these bodies resolves to the inherent impl (no recursion).

#[async_trait]
impl DriveClient for HttpDriveClient {
    async fn list_all_files(&self, folder_id: &str) -> anyhow::Result<Vec<DriveFile>> {
        HttpDriveClient::list_all_files(self, folder_id).await
    }

    async fn get_file(&self, file_id: &str) -> anyhow::Result<DriveFile> {
        HttpDriveClient::get_file(self, file_id).await
    }

    async fn download_file(&self, file_id: &str, dest: &Path) -> anyhow::Result<()> {
        HttpDriveClient::download_file(self, file_id, dest).await
    }

    async fn upload_file(
        &self,
        parent_id: &str,
        name: &str,
        local_path: &Path,
    ) -> anyhow::Result<DriveFile> {
        HttpDriveClient::upload_file(self, parent_id, name, local_path).await
    }

    async fn update_file(&self, file_id: &str, local_path: &Path) -> anyhow::Result<DriveFile> {
        HttpDriveClient::update_file(self, file_id, local_path).await
    }

    async fn create_folder(&self, parent_id: &str, name: &str) -> anyhow::Result<DriveFile> {
        HttpDriveClient::create_folder(self, parent_id, name).await
    }

    async fn trash_file(&self, file_id: &str) -> anyhow::Result<()> {
        HttpDriveClient::trash_file(self, file_id).await
    }
}

// ── API Response Types ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveFile {
    pub id: String,
    pub name: String,
    #[serde(rename = "mimeType")]
    pub mime_type: String,
    #[serde(rename = "md5Checksum")]
    pub md5_checksum: Option<String>,
    pub size: Option<String>,
    #[serde(rename = "modifiedTime")]
    pub modified_time: Option<String>,
    pub parents: Option<Vec<String>>,
}

impl DriveFile {
    pub fn is_folder(&self) -> bool {
        self.mime_type == "application/vnd.google-apps.folder"
    }

    pub fn is_google_doc(&self) -> bool {
        self.mime_type.starts_with("application/vnd.google-apps.")
            && !self.is_folder()
    }

    pub fn size_bytes(&self) -> i64 {
        self.size
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }
}

#[derive(Debug, Deserialize)]
pub struct FileListResponse {
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    pub files: Vec<DriveFile>,
}

#[derive(Debug, Deserialize)]
pub struct ChangesResponse {
    #[serde(rename = "nextPageToken")]
    pub next_page_token: Option<String>,
    #[serde(rename = "newStartPageToken")]
    pub new_start_page_token: Option<String>,
    pub changes: Vec<Change>,
}

#[derive(Debug, Deserialize)]
pub struct Change {
    #[serde(rename = "fileId")]
    pub file_id: String,
    pub removed: Option<bool>,
    pub file: Option<DriveFile>,
}

#[derive(Debug, Deserialize)]
pub struct AboutResponse {
    pub user: Option<AboutUser>,
    #[serde(rename = "storageQuota")]
    pub storage_quota: Option<StorageQuota>,
}

#[derive(Debug, Deserialize)]
pub struct AboutUser {
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
    #[serde(rename = "emailAddress")]
    pub email_address: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StorageQuota {
    pub limit: Option<String>,
    pub usage: Option<String>,
    #[serde(rename = "usageInDrive")]
    pub usage_in_drive: Option<String>,
    #[serde(rename = "usageInDriveTrash")]
    pub usage_in_drive_trash: Option<String>,
}

/// Decode a successful upload response into a [`DriveFile`], or surface the
/// API's human-readable error if the status is non-2xx.
async fn check_success(resp: reqwest::Response, ctx: &str) -> anyhow::Result<DriveFile> {
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "{ctx} {status}: {}",
            extract_drive_error(&body).unwrap_or(body)
        );
    }
    Ok(resp.json().await?)
}

/// Extract the human-readable error from a Google API JSON error body.
/// Shape: `{ "error": { "code": 403, "message": "...", "errors": [{ "reason": "..." }] } }`
fn extract_drive_error(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let err = v.get("error")?;
    let message = err.get("message").and_then(|m| m.as_str()).unwrap_or("");
    let reason = err
        .get("errors")
        .and_then(|e| e.as_array())
        .and_then(|arr| arr.first())
        .and_then(|e| e.get("reason"))
        .and_then(|r| r.as_str());
    Some(match reason {
        Some(r) => format!("{message} (reason: {r})"),
        None => message.to_string(),
    })
}
