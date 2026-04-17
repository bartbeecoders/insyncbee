//! Google Drive v3 REST API client.

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tokio::io::AsyncWriteExt;

use crate::auth::AuthManager;

const DRIVE_API: &str = "https://www.googleapis.com/drive/v3";
const UPLOAD_API: &str = "https://www.googleapis.com/upload/drive/v3";

/// A client for the Google Drive v3 API.
pub struct DriveClient {
    http: reqwest::Client,
    auth: AuthManager,
    account_id: String,
}

impl DriveClient {
    pub fn new(auth: AuthManager, account_id: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            auth,
            account_id,
        }
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

    /// Upload a new file (simple upload, for files < 5 MB).
    pub async fn upload_file(
        &self,
        parent_id: &str,
        name: &str,
        local_path: &Path,
    ) -> anyhow::Result<DriveFile> {
        let token = self.token().await?;
        let content = tokio::fs::read(local_path).await?;
        let mime = mime_guess::from_path(local_path)
            .first_or_octet_stream()
            .to_string();

        let metadata = serde_json::json!({
            "name": name,
            "parents": [parent_id],
        });

        // Use multipart upload
        let boundary = "insyncbee_boundary";
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n").as_bytes());
        body.extend_from_slice(serde_json::to_string(&metadata)?.as_bytes());
        body.extend_from_slice(format!("\r\n--{boundary}\r\nContent-Type: {mime}\r\n\r\n").as_bytes());
        body.extend_from_slice(&content);
        body.extend_from_slice(format!("\r\n--{boundary}--").as_bytes());

        let resp = self
            .http
            .post(format!("{UPLOAD_API}/files?uploadType=multipart&fields=id,name,mimeType,md5Checksum,size,modifiedTime,parents"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, format!("multipart/related; boundary={boundary}"))
            .body(body)
            .send()
            .await?
            .error_for_status()?;

        let file: DriveFile = resp.json().await?;
        Ok(file)
    }

    /// Update an existing file's content.
    pub async fn update_file(
        &self,
        file_id: &str,
        local_path: &Path,
    ) -> anyhow::Result<DriveFile> {
        let token = self.token().await?;
        let content = tokio::fs::read(local_path).await?;
        let mime = mime_guess::from_path(local_path)
            .first_or_octet_stream()
            .to_string();

        let resp = self
            .http
            .patch(format!("{UPLOAD_API}/files/{file_id}?uploadType=media&fields=id,name,mimeType,md5Checksum,size,modifiedTime,parents"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, mime)
            .body(content)
            .send()
            .await?
            .error_for_status()?;

        let file: DriveFile = resp.json().await?;
        Ok(file)
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
