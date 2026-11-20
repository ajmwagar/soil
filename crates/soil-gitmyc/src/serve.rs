//! The network edge: capability in, forge access out.
//!
//! Two ways to consume a repository, both authorized the same way and both
//! leaving the forge's credential on this host:
//!
//! * `git clone` — smart-HTTP. The reference advertisement is filtered by the
//!   capability (a branch-scoped token cannot see other branches, and `HEAD` is
//!   authorized by the identity of its tip rather than by a pattern), and pack
//!   negotiation is proxied. Git computes packs; this is an access layer.
//! * the VFS projection — `/tree` and `/blob` serve a commit as content with the
//!   VFS's own content id in `x-content-id`, which is what a build verifies.
//!
//! `POST /mint` is the only route that needs the signing key, and it is disabled
//! unless an admin token is configured.

use std::{collections::BTreeMap, sync::Arc};

use axum::{
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{SigningKey, VerifyingKey};
use fungos_vfs::ContentId;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::{
    capability::{
        self, CAPABILITY_VERSION, CapError, Claims, MAX_READ_TTL_SECS, MAX_WRITE_TTL_SECS,
        filter_ref_advertisement_with,
    },
    project::{Forge, is_commit, is_repo, is_tree_path},
    refs::{Outcome, Refs},
};

/// Longest capability we will even look at: it travels in a URL.
const MAX_CAPABILITY_BYTES: usize = 4096;

#[derive(Clone)]
pub struct ForgeState {
    pub key: SigningKey,
    pub forge: Forge,
    /// The refs authority. One process, one lock: an update is serialized so the
    /// compare-and-swap cannot be raced by a second request in flight.
    pub refs: Arc<tokio::sync::Mutex<Refs>>,
    pub admin_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct RefUpdateRequest {
    pub repo: String,
    #[serde(rename = "ref")]
    pub name: String,
    /// `null` means "this ref must not exist yet".
    #[serde(default)]
    pub old: Option<String>,
    pub new: String,
}

#[derive(Debug, Deserialize)]
pub struct MintRequest {
    pub repo: String,
    pub sub: String,
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub write: Vec<String>,
    #[serde(default)]
    pub commit: Option<String>,
    #[serde(default)]
    pub ttl_secs: Option<i64>,
    #[serde(default)]
    pub kind: Option<String>,
}

pub fn router(state: ForgeState) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/mint", post(mint))
        .route("/git/{owner}/{repo}/info/refs", get(info_refs))
        .route("/git/{owner}/{repo}/git-upload-pack", post(upload_pack))
        .route("/git/{owner}/{repo}/git-receive-pack", post(receive_pack))
        .route("/refs/{owner}/{repo}", get(list_refs))
        .route("/refs/update", post(update_ref))
        .route("/audit/{owner}/{repo}", get(audit))
        .route("/tree/{owner}/{repo}/{commit}", get(tree))
        .route("/blob/{owner}/{repo}/{commit}/{*path}", get(blob))
        .with_state(Arc::new(state))
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "capability": CAPABILITY_VERSION }))
}

async fn mint(
    State(state): State<Arc<ForgeState>>,
    headers: HeaderMap,
    Json(request): Json<MintRequest>,
) -> Response {
    let Some(admin) = state.admin_token.as_deref() else {
        return error(
            StatusCode::NOT_IMPLEMENTED,
            "minting is not enabled on this node",
        );
    };
    if headers.get("x-gitmyc-admin").and_then(|value| value.to_str().ok()) != Some(admin) {
        return error(StatusCode::UNAUTHORIZED, "admin token rejected");
    }
    if !is_repo(&request.repo) {
        return error(StatusCode::BAD_REQUEST, "repo must be owner/name");
    }
    let kind = request
        .kind
        .as_deref()
        .unwrap_or(if request.write.is_empty() { "read" } else { "write" });
    let ceiling = if request.write.is_empty() {
        MAX_READ_TTL_SECS
    } else {
        MAX_WRITE_TTL_SECS
    };
    let ttl = request.ttl_secs.unwrap_or(ceiling).clamp(1, ceiling);
    let now = capability::now_secs();
    let claims = Claims {
        v: 1,
        repo: request.repo,
        commit: request.commit,
        read: request.read,
        write: request.write,
        sub: request.sub,
        iat: now,
        exp: now + ttl,
        nonce: capability::nonce(),
        kind: kind.to_string(),
    };
    Json(json!({ "capability": claims.mint(&state.key), "claims": claims })).into_response()
}

fn capability_from(headers: &HeaderMap) -> Result<String, Response> {
    let Some(raw) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "present a capability: Authorization: Bearer <gitmyc1...>",
        ));
    };
    if let Some(token) = raw.strip_prefix("Bearer ") {
        return Ok(token.trim().to_string());
    }
    if let Some(basic) = raw.strip_prefix("Basic ") {
        // git puts the capability where a web server puts a password; the
        // username is decoration.
        let decoded = STANDARD
            .decode(basic.trim())
            .map_err(|_| error(StatusCode::UNAUTHORIZED, "basic auth is not base64"))?;
        let decoded = String::from_utf8(decoded)
            .map_err(|_| error(StatusCode::UNAUTHORIZED, "basic auth is not utf-8"))?;
        let (_, password) = decoded
            .split_once(':')
            .ok_or_else(|| error(StatusCode::UNAUTHORIZED, "basic auth has no password"))?;
        return Ok(password.to_string());
    }
    Err(error(
        StatusCode::UNAUTHORIZED,
        "unsupported authorization scheme",
    ))
}

fn authorize(state: &ForgeState, headers: &HeaderMap, repo: &str) -> Result<Claims, Response> {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.len() > MAX_CAPABILITY_BYTES + 32)
    {
        return Err(error(StatusCode::UNAUTHORIZED, "capability is too long"));
    }
    let token = capability_from(headers)?;
    let verifying: VerifyingKey = state.key.verifying_key();
    Claims::verify(&token, &verifying, capability::now_secs(), repo)
        .map_err(|failure: CapError| error(failure.status(), &failure.to_string()))
}

async fn info_refs(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo)): AxumPath<(String, String)>,
    Query(query): Query<BTreeMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let repo = repo_name(&owner, &repo);
    let claims = match authorize(&state, &headers, &repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let service = query
        .get("service")
        .cloned()
        .unwrap_or_else(|| "git-upload-pack".to_string());
    if service != "git-upload-pack" {
        return error(StatusCode::FORBIDDEN, "only git-upload-pack is served");
    }
    let url = format!(
        "{}/{}.git/info/refs?service={service}",
        state.forge.base.trim_end_matches('/'),
        repo
    );
    let response = match state
        .forge
        .client
        .get(url)
        .basic_auth(&state.forge.user, Some(&state.forge.token))
        .header(header::ACCEPT, "application/x-git-upload-pack-advertisement")
        .send()
        .await
    {
        Ok(response) => response,
        Err(failure) => {
            return error(
                StatusCode::BAD_GATEWAY,
                &format!("upstream unreachable: {failure}"),
            );
        }
    };
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return error(
            StatusCode::BAD_GATEWAY,
            &format!("upstream {status}: {}", body.chars().take(300).collect::<String>()),
        );
    }
    let advertisement = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(failure) => {
            return error(StatusCode::BAD_GATEWAY, &format!("upstream body: {failure}"));
        }
    };
    let overlay: Vec<(String, String)> = {
        let refs = state.refs.lock().await;
        match refs.list(&repo) {
            Ok(values) => values
                .into_iter()
                .map(|value| (value.name, value.sha))
                .collect(),
            Err(failure) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("refs: {failure:#}"),
                );
            }
        }
    };
    let Some((_, _, filtered)) =
        filter_ref_advertisement_with(&advertisement, &claims, &overlay)
    else {
        return error(
            StatusCode::FORBIDDEN,
            "capability covers none of this repository's refs",
        );
    };
    let mut out = Response::new(Body::from(filtered));
    out.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/x-git-upload-pack-advertisement"),
    );
    out
}

async fn upload_pack(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo)): AxumPath<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let repo = repo_name(&owner, &repo);
    if let Err(response) = authorize(&state, &headers, &repo) {
        return response;
    }
    let url = format!(
        "{}/{}.git/git-upload-pack",
        state.forge.base.trim_end_matches('/'),
        repo
    );
    let response = match state
        .forge
        .client
        .post(url)
        .basic_auth(&state.forge.user, Some(&state.forge.token))
        .header(
            header::CONTENT_TYPE,
            "application/x-git-upload-pack-request",
        )
        .header(header::ACCEPT, "application/x-git-upload-pack-result")
        .body(body)
        .send()
        .await
    {
        Ok(response) => response,
        Err(failure) => {
            return error(
                StatusCode::BAD_GATEWAY,
                &format!("upstream unreachable: {failure}"),
            );
        }
    };
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return error(
            StatusCode::BAD_GATEWAY,
            &format!("upstream {status}: {}", body.chars().take(300).collect::<String>()),
        );
    }
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/x-git-upload-pack-result")
        .to_string();
    let mut out = Response::new(Body::from_stream(response.bytes_stream()));
    if let Ok(value) = header::HeaderValue::from_str(&content_type) {
        out.headers_mut().insert(header::CONTENT_TYPE, value);
    }
    out
}

async fn receive_pack(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let repo = repo_name(&owner, &repo);
    let claims = match authorize(&state, &headers, &repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if claims.write.is_empty() {
        return error(
            StatusCode::FORBIDDEN,
            "this capability is read-only; a write capability is not implemented yet",
        );
    }
    error(
        StatusCode::NOT_IMPLEMENTED,
        "ref updates go through POST /refs/update (compare-and-swap), not git-receive-pack",
    )
}

async fn tree(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo, commit)): AxumPath<(String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let repo = repo_name(&owner, &repo);
    let claims = match authorize(&state, &headers, &repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let resolved = match resolve(&state, &repo, &commit, &claims).await {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };
    match state.forge.tree(&repo, &resolved.commit).await {
        Ok(entries) => Json(json!({
            "repo": repo,
            "commit": resolved.commit,
            "schema": "gitmyc.tree/1",
            "entries": entries,
        }))
        .into_response(),
        Err(failure) => error(StatusCode::BAD_GATEWAY, &format!("{failure:#}")),
    }
}

async fn blob(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo, commit, path)): AxumPath<(String, String, String, String)>,
    headers: HeaderMap,
) -> Response {
    let repo = repo_name(&owner, &repo);
    let claims = match authorize(&state, &headers, &repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if !is_tree_path(&path) {
        return error(StatusCode::BAD_REQUEST, "path escapes the tree");
    }
    let resolved = match resolve(&state, &repo, &commit, &claims).await {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };
    match state.forge.blob(&repo, &resolved.commit, &path).await {
        Ok(bytes) => {
            let id = ContentId::from_bytes(&bytes);
            let mut out = Response::new(Body::from(bytes));
            out.headers_mut().insert(
                header::CONTENT_TYPE,
                header::HeaderValue::from_static(crate::project::media_type(&path)),
            );
            if let Ok(value) = header::HeaderValue::from_str(&id.to_string()) {
                // The VFS's own identity, so a caller can verify what it got and
                // so a build can ask for the same bytes again by name.
                out.headers_mut().insert("x-content-id", value);
            }
            if let Ok(value) = header::HeaderValue::from_str(&resolved.commit) {
                out.headers_mut().insert("x-commit", value);
            }
            out
        }
        Err(failure) => error(StatusCode::BAD_GATEWAY, &format!("{failure:#}")),
    }
}

/// Resolve once, honor a pinned capability, and normalize a bare branch name the
/// way a human writes it before authorizing (never after).
async fn resolve(
    state: &ForgeState,
    repo: &str,
    requested: &str,
    claims: &Claims,
) -> Result<crate::project::Resolved, Response> {
    if let Some(pinned) = &claims.commit {
        if requested != pinned.as_str() && requested != "HEAD" {
            return Err(error(
                StatusCode::FORBIDDEN,
                "this capability is pinned to one commit",
            ));
        }
        return Ok(crate::project::Resolved {
            repo: repo.to_string(),
            requested: "pinned".to_string(),
            commit: pinned.clone(),
        });
    }
    let full_ref = if requested.starts_with("refs/") || crate::project::is_commit(requested) {
        requested.to_string()
    } else {
        format!("refs/heads/{requested}")
    };
    if !crate::project::is_commit(&full_ref) && !claims.allows_read(&full_ref) {
        return Err(error(
            StatusCode::FORBIDDEN,
            &format!("capability does not cover {full_ref}"),
        ));
    }
    state
        .forge
        .resolve(repo, &full_ref)
        .await
        .map_err(|failure| error(StatusCode::BAD_GATEWAY, &format!("{failure:#}")))
}

async fn list_refs(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let repo = repo_name(&owner, &repo);
    let claims = match authorize(&state, &headers, &repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let refs = state.refs.lock().await;
    match refs.list(&repo) {
        Ok(mut values) => {
            values.retain(|value| claims.allows_read(&value.name));
            Json(json!({ "repo": repo, "refs": values })).into_response()
        }
        Err(failure) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("refs: {failure:#}"),
        ),
    }
}

async fn audit(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let repo = repo_name(&owner, &repo);
    if let Err(response) = authorize(&state, &headers, &repo) {
        return response;
    }
    let refs = state.refs.lock().await;
    match refs.audit(&repo, 500) {
        Ok(entries) => Json(json!({ "repo": repo, "audit": entries })).into_response(),
        Err(failure) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("audit: {failure:#}"),
        ),
    }
}

/// Move one ref. The capability decides *which* refs; policy decides *what kind*
/// of subject may; this handler decides nothing on its own.
async fn update_ref(
    State(state): State<Arc<ForgeState>>,
    headers: HeaderMap,
    Json(request): Json<RefUpdateRequest>,
) -> Response {
    if !is_repo(&request.repo) {
        return error(StatusCode::BAD_REQUEST, "repo must be owner/name");
    }
    let claims = match authorize(&state, &headers, &request.repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if claims.write.is_empty() {
        return error(StatusCode::FORBIDDEN, "this capability is read-only");
    }
    if !claims.allows_write(&request.name) {
        return error(
            StatusCode::FORBIDDEN,
            &format!(
                "capability for {} does not cover {:?} (writes: {:?})",
                claims.repo, request.name, claims.write
            ),
        );
    }
    if !is_commit(&request.new) {
        return error(StatusCode::BAD_REQUEST, "new must be a full commit id");
    }
    if let Some(old) = &request.old
        && !is_commit(old)
    {
        return error(StatusCode::BAD_REQUEST, "old must be a full commit id or null");
    }
    // A ref may never point at content the forge cannot serve: every reader
    // would get a clone that cannot complete.
    match state.forge.commit_exists(&request.repo, &request.new).await {
        Ok(true) => {}
        Ok(false) => {
            return error(
                StatusCode::BAD_REQUEST,
                "new commit is not present at the forge; push it before moving a ref to it",
            );
        }
        Err(failure) => {
            return error(StatusCode::BAD_GATEWAY, &format!("{failure:#}"));
        }
    }
    // If this is the first time we touch a ref, take the forge's tip as the
    // baseline: a caller's `old` must be checked against what git actually shows,
    // not against "we have never heard of this branch".
    if request.old.is_some() {
        let known = {
            let refs = state.refs.lock().await;
            refs.get(&request.repo, &request.name).ok().flatten()
        };
        if known.is_none() {
            match state
                .forge
                .current_ref(&request.repo, &request.name)
                .await
            {
                Ok(Some(tip)) => {
                    let mut refs = state.refs.lock().await;
                    if let Err(failure) =
                        refs.adopt(&request.repo, &request.name, &tip, capability::now_secs())
                    {
                        return error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            &format!("refs: {failure:#}"),
                        );
                    }
                }
                Ok(None) => {}
                Err(failure) => {
                    return error(StatusCode::BAD_GATEWAY, &format!("{failure:#}"));
                }
            }
        }
    }
    let mut refs = state.refs.lock().await;
    let outcome = refs.update(
        &request.repo,
        &request.name,
        request.old.as_deref(),
        &request.new,
        &claims.sub,
        &claims.nonce,
        capability::now_secs(),
    );
    match outcome {
        Err(failure) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("refs: {failure:#}"),
        ),
        Ok(Outcome::Applied { old, new }) => Json(json!({
            "repo": request.repo,
            "ref": request.name,
            "old": old,
            "new": new,
            "subject": claims.sub,
        }))
        .into_response(),
        Ok(Outcome::Stale { current, detail }) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": detail, "current": current })),
        )
            .into_response(),
        Ok(Outcome::Denied { detail }) => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": detail }))).into_response()
        }
    }
}

fn repo_name(owner: &str, repo: &str) -> String {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    format!("{owner}/{repo}")
}

fn error(status: StatusCode, message: &str) -> Response {
    let mut response = (status, Json(json!({ "error": message }))).into_response();
    if status == StatusCode::UNAUTHORIZED {
        // Without a challenge git never retries with the credential the URL
        // carries: it reports "Authentication failed" and gives up.
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            header::HeaderValue::from_static("Basic realm=\"git.myc\""),
        );
    }
    response
}
