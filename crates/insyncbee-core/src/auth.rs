use oauth2::basic::BasicTokenType;
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EmptyExtraTokenFields,
    PkceCodeChallenge, RedirectUrl, RefreshToken, Scope, StandardTokenResponse, TokenResponse,
    TokenUrl,
};
use std::io::{BufRead, BufReader, Write as _};
use std::net::TcpListener;

use crate::db::models::Account;
use crate::db::Database;
use crate::Result;

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

/// Google Drive permission scopes.
const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/drive",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
];

/// Credentials loaded from environment or config.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OAuthCredentials {
    pub client_id: String,
    pub client_secret: String,
}

impl OAuthCredentials {
    /// Load credentials from environment variables.
    pub fn from_env() -> Result<Self> {
        let client_id = std::env::var("INSYNCBEE_CLIENT_ID")
            .map_err(|_| crate::Error::Auth("INSYNCBEE_CLIENT_ID env var not set".into()))?;
        let client_secret = std::env::var("INSYNCBEE_CLIENT_SECRET")
            .map_err(|_| crate::Error::Auth("INSYNCBEE_CLIENT_SECRET env var not set".into()))?;
        Ok(Self {
            client_id,
            client_secret,
        })
    }

    /// Read credentials from `credentials.json` in the config directory.
    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|e| {
            crate::Error::Auth(format!("could not read {}: {e}", path.display()))
        })?;
        serde_json::from_str(&raw)
            .map_err(|e| crate::Error::Auth(format!("could not parse {}: {e}", path.display())))
    }

    /// Environment first, then the config file.
    ///
    /// The environment keeps working for shells and CI, but it is not a
    /// mechanism a GUI can rely on: apps started from a desktop launcher,
    /// a file manager, or an autostart entry inherit the session
    /// environment, not the one your shell profile builds. Credentials that
    /// only ever lived in `~/.bashrc` made every desktop launch report
    /// "OAuth credentials not configured".
    pub fn load(credentials_path: &std::path::Path) -> Result<Self> {
        match Self::from_env() {
            Ok(creds) => Ok(creds),
            Err(env_err) => match Self::from_file(credentials_path) {
                Ok(creds) => Ok(creds),
                Err(_) if !credentials_path.exists() => Err(crate::Error::Auth(format!(
                    "OAuth credentials not configured. Run `insyncbee configure \
                     --client-id <id> --client-secret <secret>`, or set \
                     INSYNCBEE_CLIENT_ID and INSYNCBEE_CLIENT_SECRET ({env_err})"
                ))),
                Err(file_err) => Err(file_err),
            },
        }
    }

    /// Write credentials to the config file, readable only by this user.
    pub fn save(&self, credentials_path: &std::path::Path) -> Result<()> {
        if let Some(parent) = credentials_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| crate::Error::Auth(format!("could not create {}: {e}", parent.display())))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| crate::Error::Auth(format!("could not serialize credentials: {e}")))?;
        std::fs::write(credentials_path, json).map_err(|e| {
            crate::Error::Auth(format!("could not write {}: {e}", credentials_path.display()))
        })?;

        // The client secret is not a password, but it is not public either;
        // no reason for it to be world-readable.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(credentials_path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| {
                    crate::Error::Auth(format!(
                        "could not restrict permissions on {}: {e}",
                        credentials_path.display()
                    ))
                })?;
        }
        Ok(())
    }
}

type TokenResp = StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>;

/// Manager for Google OAuth2 authentication.
pub struct AuthManager {
    creds: OAuthCredentials,
    db: Database,
}

impl AuthManager {
    pub fn new(creds: OAuthCredentials, db: Database) -> Self {
        Self { creds, db }
    }

    /// Start the OAuth2 flow: opens the browser for consent, starts a local
    /// server to receive the callback, exchanges the code for tokens, fetches
    /// user info, and stores the account in the database.
    pub async fn login(&self) -> anyhow::Result<Account> {
        let flow = self.run_oauth_flow().await?;

        let account = Account {
            id: uuid::Uuid::new_v4().to_string(),
            email: flow.user_info.email,
            display_name: flow.user_info.name,
            access_token: flow.access_token,
            refresh_token: flow.refresh_token,
            token_expiry: flow.expiry.to_rfc3339(),
            change_token: None,
            created_at: chrono::Utc::now().to_rfc3339(),
        };

        self.db.with_conn(|conn| {
            account.insert(conn)?;
            Ok(())
        })?;

        tracing::info!("Logged in as {}", account.email);
        Ok(account)
    }

    /// Re-run the OAuth flow for an existing account, replacing its stored
    /// tokens. Use this when a refresh token has been revoked (`invalid_grant`)
    /// — the account row keeps its `id`, so existing sync pairs stay linked.
    /// Fails if the user authorizes with a different Google account.
    pub async fn reconnect_account(&self, account_id: &str) -> anyhow::Result<Account> {
        let existing = self
            .db
            .with_conn(|conn| Account::get_by_id(conn, account_id))?
            .ok_or_else(|| crate::Error::NotFound(format!("Account {account_id}")))?;

        let flow = self.run_oauth_flow().await?;

        if !flow.user_info.email.eq_ignore_ascii_case(&existing.email) {
            anyhow::bail!(
                "Reconnect was authorized as '{}', but this account is '{}'. \
                 Sign in with the same Google account, or remove this account and add the other one.",
                flow.user_info.email,
                existing.email
            );
        }

        self.db.with_conn(|conn| {
            Account::update_credentials(
                conn,
                account_id,
                &flow.access_token,
                &flow.refresh_token,
                &flow.expiry.to_rfc3339(),
                flow.user_info.name.as_deref(),
            )
        })?;

        tracing::info!("Reconnected account {}", existing.email);
        let refreshed = self
            .db
            .with_conn(|conn| Account::get_by_id(conn, account_id))?
            .ok_or_else(|| crate::Error::NotFound(format!("Account {account_id}")))?;
        Ok(refreshed)
    }

    async fn run_oauth_flow(&self) -> anyhow::Result<OAuthFlowResult> {
        // Bind to a random port for the loopback redirect
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let redirect_url = format!("http://127.0.0.1:{port}");

        let client = oauth2::basic::BasicClient::new(ClientId::new(self.creds.client_id.clone()))
            .set_client_secret(ClientSecret::new(self.creds.client_secret.clone()))
            .set_auth_uri(AuthUrl::new(GOOGLE_AUTH_URL.to_string())?)
            .set_token_uri(TokenUrl::new(GOOGLE_TOKEN_URL.to_string())?)
            .set_redirect_uri(RedirectUrl::new(redirect_url)?);

        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        // `access_type=offline` → always return a refresh token.
        // `prompt=consent` → force the consent screen so newly declared scopes are
        // actually granted (otherwise Google silently reuses a prior grant).
        let mut auth_request = client
            .authorize_url(CsrfToken::new_random)
            .set_pkce_challenge(pkce_challenge)
            .add_extra_param("access_type", "offline")
            .add_extra_param("prompt", "consent");

        for scope in SCOPES {
            auth_request = auth_request.add_scope(Scope::new(scope.to_string()));
        }

        let (auth_url, csrf_state) = auth_request.url();

        tracing::info!("Opening browser for Google sign-in...");
        let url_str = auth_url.to_string();
        if open::that(&url_str).is_err() {
            println!("\nOpen this URL in your browser:\n{url_str}\n");
        }

        let (code, _state) = receive_callback(listener, &csrf_state)?;

        let http_client = reqwest::Client::new();
        let token_response: TokenResp = client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pkce_verifier)
            .request_async(&http_client)
            .await
            .map_err(|e| crate::Error::Auth(format!("Token exchange failed: {e}")))?;

        let access_token = token_response.access_token().secret().to_string();
        let refresh_token = token_response
            .refresh_token()
            .map(|t: &oauth2::RefreshToken| t.secret().to_string())
            .ok_or_else(|| crate::Error::Auth("No refresh token received".into()))?;

        let expires_in = token_response
            .expires_in()
            .unwrap_or(std::time::Duration::from_secs(3600));
        let expiry = chrono::Utc::now() + chrono::Duration::from_std(expires_in).unwrap_or_default();

        let user_info = fetch_user_info(&access_token).await?;

        Ok(OAuthFlowResult {
            access_token,
            refresh_token,
            expiry,
            user_info,
        })
    }

    /// Refresh the access token for an account.
    pub async fn refresh_token(&self, account_id: &str) -> anyhow::Result<String> {
        let account = self
            .db
            .with_conn(|conn| Account::get_by_id(conn, account_id))?
            .ok_or_else(|| crate::Error::NotFound(format!("Account {account_id}")))?;

        let client = oauth2::basic::BasicClient::new(ClientId::new(self.creds.client_id.clone()))
            .set_client_secret(ClientSecret::new(self.creds.client_secret.clone()))
            .set_auth_uri(AuthUrl::new(GOOGLE_AUTH_URL.to_string())?)
            .set_token_uri(TokenUrl::new(GOOGLE_TOKEN_URL.to_string())?);

        let http_client = reqwest::Client::new();
        let token_response: TokenResp = client
            .exchange_refresh_token(&RefreshToken::new(account.refresh_token))
            .request_async(&http_client)
            .await
            .map_err(|e| crate::Error::Auth(format!("Token refresh failed: {e}")))?;

        let new_access_token = token_response.access_token().secret().to_string();
        let expires_in = token_response
            .expires_in()
            .unwrap_or(std::time::Duration::from_secs(3600));
        let expiry = chrono::Utc::now() + chrono::Duration::from_std(expires_in).unwrap_or_default();

        self.db.with_conn(|conn| {
            Account::update_tokens(conn, account_id, &new_access_token, &expiry.to_rfc3339())?;
            Ok(())
        })?;

        Ok(new_access_token)
    }

    /// Get a valid access token, refreshing if expired.
    pub async fn get_valid_token(&self, account_id: &str) -> anyhow::Result<String> {
        let account = self
            .db
            .with_conn(|conn| Account::get_by_id(conn, account_id))?
            .ok_or_else(|| crate::Error::NotFound(format!("Account {account_id}")))?;

        let expiry = chrono::DateTime::parse_from_rfc3339(&account.token_expiry)
            .unwrap_or_else(|_| chrono::Utc::now().into());

        // Refresh if token expires within 5 minutes
        if chrono::Utc::now() + chrono::Duration::minutes(5) >= expiry {
            tracing::debug!("Access token expired or expiring soon, refreshing...");
            return self.refresh_token(account_id).await;
        }

        Ok(account.access_token)
    }

    pub fn list_accounts(&self) -> Result<Vec<Account>> {
        self.db.with_conn(|conn| Account::list(conn))
    }

    pub fn remove_account(&self, account_id: &str) -> Result<()> {
        self.db.with_conn(|conn| {
            let dependents: i64 = conn.query_row(
                "SELECT COUNT(*) FROM sync_pairs WHERE account_id = ?1",
                rusqlite::params![account_id],
                |row| row.get(0),
            )?;
            if dependents > 0 {
                return Err(crate::Error::Other(format!(
                    "Cannot remove account: {dependents} sync pair(s) still use it. Delete those sync pairs first."
                )));
            }
            Account::delete(conn, account_id)
        })
    }
}

/// Listen for the OAuth callback on the loopback server.
fn receive_callback(
    listener: TcpListener,
    expected_state: &CsrfToken,
) -> anyhow::Result<(String, String)> {
    let (stream, _) = listener.accept()?;
    let mut reader = BufReader::new(&stream);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;

    // Parse the authorization code from the query string
    let redirect_url = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| crate::Error::Auth("Invalid callback request".into()))?;

    let url = url::Url::parse(&format!("http://localhost{redirect_url}"))?;
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();

    let code = params
        .get("code")
        .ok_or_else(|| crate::Error::Auth("No code in callback".into()))?
        .to_string();

    let state = params
        .get("state")
        .ok_or_else(|| crate::Error::Auth("No state in callback".into()))?
        .to_string();

    if state != expected_state.secret().as_str() {
        return Err(crate::Error::Auth("CSRF state mismatch".into()).into());
    }

    // Send a success response to the browser
    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
        <html><body><h2>InSyncBee</h2><p>Authentication successful! You can close this tab.</p></body></html>";
    let mut stream = stream;
    stream.write_all(response.as_bytes())?;

    Ok((code, state))
}

struct OAuthFlowResult {
    access_token: String,
    refresh_token: String,
    expiry: chrono::DateTime<chrono::Utc>,
    user_info: UserInfo,
}

#[derive(serde::Deserialize)]
struct UserInfo {
    email: String,
    name: Option<String>,
}

async fn fetch_user_info(access_token: &str) -> anyhow::Result<UserInfo> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://www.googleapis.com/oauth2/v2/userinfo")
        .bearer_auth(access_token)
        .send()
        .await?
        .error_for_status()?;
    let info: UserInfo = resp.json().await?;
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Deliberately no test that mutates INSYNCBEE_* env vars: cargo runs
    // tests in threads of one process, so an env write in one test is
    // visible to every other, and `load`'s precedence would flake.
    // The env arm is a one-line `from_env` call; what's worth pinning down
    // is the file arm that the desktop app depends on.

    #[test]
    fn saved_credentials_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("credentials.json");

        let creds = OAuthCredentials {
            client_id: "id.apps.googleusercontent.com".into(),
            client_secret: "secret".into(),
        };
        creds.save(&path).unwrap();

        let loaded = OAuthCredentials::from_file(&path).unwrap();
        assert_eq!(loaded.client_id, creds.client_id);
        assert_eq!(loaded.client_secret, creds.client_secret);
    }

    #[cfg(unix)]
    #[test]
    fn saved_credentials_are_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        OAuthCredentials {
            client_id: "id".into(),
            client_secret: "secret".into(),
        }
        .save(&path)
        .unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "credentials should be owner-only, got {mode:o}");
    }

    #[test]
    fn missing_file_and_no_env_names_both_ways_to_fix_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");

        // Only meaningful when the env is not configured, which is the case
        // in CI and for any developer who uses the config file instead.
        if OAuthCredentials::from_env().is_ok() {
            return;
        }

        let err = OAuthCredentials::load(&path).unwrap_err().to_string();
        assert!(err.contains("insyncbee configure"), "got: {err}");
        assert!(err.contains("INSYNCBEE_CLIENT_ID"), "got: {err}");
    }
}
