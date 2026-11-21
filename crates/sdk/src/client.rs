use std::time::Duration;

use anyhow::{Context, Result};
use bytes::Bytes;
use condensate_core::api::*;
use reqwest::{Method, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::Serialize;

/// An error response from the server.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: u16,
    pub code: String,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (HTTP {})", self.message, self.status)
    }
}

impl std::error::Error for ApiError {}

/// Typed client for the Condensate HTTP API.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: Option<String>,
}

impl Client {
    pub fn new(server: &str, token: Option<String>) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("condensate/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(60))
            .build()?;
        Ok(Self { http, base: server.trim_end_matches('/').to_string(), token })
    }

    pub fn server(&self) -> &str {
        &self.base
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn set_token(&mut self, token: Option<String>) {
        self.token = token;
    }

    fn req(&self, method: Method, path: &str) -> RequestBuilder {
        let rb = self.http.request(method, format!("{}/api/v1{}", self.base, path));
        match &self.token {
            Some(t) => rb.bearer_auth(t),
            None => rb,
        }
    }

    async fn send_raw(&self, rb: RequestBuilder) -> Result<reqwest::Response> {
        let resp = rb.send().await.with_context(|| format!("could not reach {}", self.base))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let text = resp.text().await.unwrap_or_default();
        let err = match serde_json::from_str::<ApiErrorBody>(&text) {
            Ok(b) => ApiError { status: status.as_u16(), code: b.error, message: b.message },
            Err(_) => ApiError {
                status: status.as_u16(),
                code: "http".into(),
                message: if text.trim().is_empty() { status.to_string() } else { text },
            },
        };
        Err(err.into())
    }

    async fn send<T: DeserializeOwned>(&self, rb: RequestBuilder) -> Result<T> {
        self.send_raw(rb).await?.json::<T>().await.context("unexpected response from server")
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.req(Method::GET, path)).await
    }

    async fn post<B: Serialize + ?Sized, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.send(self.req(Method::POST, path).json(body)).await
    }

    async fn patch<B: Serialize + ?Sized, T: DeserializeOwned>(&self, path: &str, body: &B) -> Result<T> {
        self.send(self.req(Method::PATCH, path).json(body)).await
    }

    async fn put_empty<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.req(Method::PUT, path)).await
    }

    async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.send(self.req(Method::DELETE, path)).await
    }

    // ---- server & accounts ---------------------------------------------------------------------

    pub async fn server_info(&self) -> Result<ServerInfo> {
        self.get("/server").await
    }

    pub async fn register(&self, req: &RegisterRequest) -> Result<AuthResponse> {
        self.post("/auth/register", req).await
    }

    pub async fn login(&self, req: &LoginRequest) -> Result<AuthResponse> {
        self.post("/auth/login", req).await
    }

    pub async fn logout(&self) -> Result<()> {
        self.post::<_, OkResponse>("/auth/logout", &serde_json::json!({})).await.map(|_| ())
    }

    pub async fn me(&self) -> Result<User> {
        self.get("/me").await
    }

    pub async fn update_me(&self, req: &UpdateProfileRequest) -> Result<User> {
        self.patch("/me", req).await
    }

    pub async fn change_password(&self, req: &ChangePasswordRequest) -> Result<()> {
        self.post::<_, OkResponse>("/me/password", req).await.map(|_| ())
    }

    pub async fn profile(&self, username: &str) -> Result<UserProfile> {
        self.get(&format!("/users/{username}")).await
    }

    pub async fn create_invite(&self) -> Result<Invite> {
        self.post("/invites", &serde_json::json!({})).await
    }

    pub async fn list_invites(&self) -> Result<Vec<Invite>> {
        self.get("/invites").await
    }

    pub async fn create_token(&self, req: &CreateTokenRequest) -> Result<CreatedToken> {
        self.post("/tokens", req).await
    }

    pub async fn list_tokens(&self) -> Result<Vec<TokenInfo>> {
        self.get("/tokens").await
    }

    pub async fn revoke_token(&self, id: i64) -> Result<()> {
        self.delete::<OkResponse>(&format!("/tokens/{id}")).await.map(|_| ())
    }

    // ---- games -----------------------------------------------------------------------------------

    pub async fn search_games(&self, query: Option<&str>) -> Result<Vec<GameSummary>> {
        let rb = self.req(Method::GET, "/games").query(&[("q", query.unwrap_or(""))]);
        self.send(rb).await
    }

    pub async fn create_game(&self, req: &CreateGameRequest) -> Result<GameDetail> {
        self.post("/games", req).await
    }

    pub async fn game(&self, slug: &str) -> Result<GameDetail> {
        self.get(&format!("/games/{slug}")).await
    }

    pub async fn update_game(&self, slug: &str, req: &UpdateGameRequest) -> Result<GameDetail> {
        self.patch(&format!("/games/{slug}"), req).await
    }

    pub async fn versions(&self, slug: &str) -> Result<Vec<VersionInfo>> {
        self.get(&format!("/games/{slug}/versions")).await
    }

    pub async fn publish(&self, slug: &str, req: &PublishRequest) -> Result<PublishResponse> {
        self.post(&format!("/games/{slug}/versions"), req).await
    }

    pub async fn latest_build(&self, slug: &str, platform: &str) -> Result<BuildInfo> {
        self.get(&format!("/games/{slug}/builds/{platform}")).await
    }

    pub async fn library(&self, platform: Option<&str>) -> Result<Vec<LibraryEntry>> {
        let mut rb = self.req(Method::GET, "/library");
        if let Some(p) = platform {
            rb = rb.query(&[("platform", p)]);
        }
        self.send(rb).await
    }

    pub async fn library_add(&self, slug: &str) -> Result<()> {
        self.put_empty::<OkResponse>(&format!("/library/{slug}")).await.map(|_| ())
    }

    pub async fn library_remove(&self, slug: &str) -> Result<()> {
        self.delete::<OkResponse>(&format!("/library/{slug}")).await.map(|_| ())
    }

    pub async fn my_games(&self) -> Result<Vec<GameSummary>> {
        self.get("/me/games").await
    }

    pub async fn delete_version(&self, slug: &str, version: &str) -> Result<()> {
        self.delete::<OkResponse>(&format!("/games/{slug}/versions/{version}")).await.map(|_| ())
    }

    pub async fn report_playtime(&self, slug: &str, seconds: i64) -> Result<()> {
        self.post::<_, OkResponse>(&format!("/games/{slug}/playtime"), &PlaytimeReport { seconds }).await.map(|_| ())
    }

    /// Upload a PNG/JPEG/GIF/WebP image; returns its id.
    pub async fn upload_image(&self, bytes: Vec<u8>) -> Result<String> {
        let rb = self.req(Method::PUT, "/images").body(bytes);
        Ok(self.send::<ImageUploaded>(rb).await?.id)
    }

    /// Public URL of an uploaded image.
    pub fn image_url(&self, id: &str) -> String {
        format!("{}/images/{id}", self.base)
    }

    // ---- social ------------------------------------------------------------------------------

    pub async fn search_users(&self, query: &str) -> Result<Vec<User>> {
        self.send(self.req(Method::GET, "/users").query(&[("q", query)])).await
    }

    pub async fn friends(&self) -> Result<FriendsResponse> {
        self.get("/friends").await
    }

    pub async fn add_friend(&self, username: &str) -> Result<()> {
        self.put_empty::<OkResponse>(&format!("/friends/{username}")).await.map(|_| ())
    }

    pub async fn remove_friend(&self, username: &str) -> Result<()> {
        self.delete::<OkResponse>(&format!("/friends/{username}")).await.map(|_| ())
    }

    pub async fn conversations(&self) -> Result<Vec<Conversation>> {
        self.get("/conversations").await
    }

    pub async fn messages(&self, username: &str, before: Option<i64>) -> Result<Vec<ChatMessage>> {
        let mut rb = self.req(Method::GET, &format!("/messages/{username}"));
        if let Some(b) = before {
            rb = rb.query(&[("before", b)]);
        }
        self.send(rb).await
    }

    pub async fn send_message(&self, username: &str, body: &str) -> Result<ChatMessage> {
        self.post(&format!("/messages/{username}"), &SendMessageRequest { body: body.into() }).await
    }

    pub async fn mark_read(&self, username: &str) -> Result<()> {
        self.post::<_, OkResponse>(&format!("/messages/{username}/read"), &serde_json::json!({})).await.map(|_| ())
    }

    /// WebSocket URL for live events (presence, messages).
    pub fn ws_url(&self) -> Option<String> {
        let token = self.token.as_deref()?;
        let base = self.base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
        Some(format!("{base}/api/v1/ws?token={token}"))
    }

    // ---- chunks ----------------------------------------------------------------------------------

    pub async fn missing_chunks(&self, hashes: &[String]) -> Result<Vec<String>> {
        let resp: MissingChunksResponse =
            self.post("/chunks/missing", &MissingChunksRequest { hashes: hashes.to_vec() }).await?;
        Ok(resp.missing)
    }

    /// Upload one zstd-compressed chunk.
    pub async fn upload_chunk(&self, hash: &str, compressed: Vec<u8>) -> Result<()> {
        let rb = self
            .req(Method::PUT, &format!("/chunks/{hash}"))
            .header(reqwest::header::CONTENT_TYPE, "application/zstd")
            .body(compressed);
        self.send::<OkResponse>(rb).await.map(|_| ())
    }

    /// Download one chunk, still zstd-compressed.
    pub async fn download_chunk(&self, hash: &str) -> Result<Bytes> {
        let resp = self.send_raw(self.req(Method::GET, &format!("/chunks/{hash}"))).await?;
        Ok(resp.bytes().await?)
    }
}
