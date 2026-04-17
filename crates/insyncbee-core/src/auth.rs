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
#[derive(Debug, Clone)]
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
        // Bind to a random port for the loopback redirect
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let redirect_url = format!("http://127.0.0.1:{port}");

        let client = oauth2::basic::BasicClient::new(ClientId::new(self.creds.client_id.clone()))
            .set_client_secret(ClientSecret::new(self.creds.client_secret.clone()))
            .set_auth_uri(AuthUrl::new(GOOGLE_AUTH_URL.to_string())?)
            .set_token_uri(TokenUrl::new(GOOGLE_TOKEN_URL.to_string())?)
            .set_redirect_uri(RedirectUrl::new(redirect_url)?);

        // Generate PKCE challenge
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();

        // Build authorization URL.
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

        // Wait for the OAuth callback
        let (code, _state) = receive_callback(listener, &csrf_state)?;

        // Exchange code for tokens
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

        // Fetch user info
        let user_info = fetch_user_info(&access_token).await?;

        let account = Account {
            id: uuid::Uuid::new_v4().to_string(),
            email: user_info.email,
            display_name: user_info.name,
            access_token,
            refresh_token,
            token_expiry: expiry.to_rfc3339(),
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
        self.db.with_conn(|conn| Account::delete(conn, account_id))
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
