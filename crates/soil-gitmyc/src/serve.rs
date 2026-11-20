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
    pulls::{Pulls, Stage},
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
    /// Proposals to move a base ref, and the verdicts on them. The landing itself
    /// is still a refs compare-and-swap.
    pub pulls: Arc<tokio::sync::Mutex<Pulls>>,
    pub admin_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PullRequest {
    pub repo: String,
    pub head: String,
    pub base: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub draft: bool,
}

#[derive(Debug, Deserialize)]
pub struct MergeRequest {
    /// The head commit the caller believes it is landing. A stale value is a 409,
    /// exactly like a ref update.
    pub head_sha: String,
}

#[derive(Debug, Deserialize)]
pub struct ReviewRequest {
    /// `approved`, `changes_requested` or `commented`.
    pub state: String,
    #[serde(default)]
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct CheckRequest {
    pub name: String,
    /// `pending`, `success` or `failure`.
    pub state: String,
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
        .route("/pulls/{owner}/{repo}", get(list_pulls).post(open_pull))
        .route(
            "/pulls/{owner}/{repo}/{number}",
            get(get_pull).patch(update_pull),
        )
        .route("/pulls/{owner}/{repo}/{number}/reviews", post(review_pull))
        .route("/pulls/{owner}/{repo}/{number}/checks", post(check_pull))
        .route("/pulls/{owner}/{repo}/{number}/merge", post(merge_pull))
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
    // A caller's `old` must be checked against what git actually shows, not
    // against "we have never heard of this branch".
    if request.old.is_some() {
        if let Err(response) = adopt_baseline(&state, &request.repo, &request.name).await {
            return response;
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

async fn list_pulls(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo)): AxumPath<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let repo = repo_name(&owner, &repo);
    let claims = match authorize(&state, &headers, &repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    let pulls = state.pulls.lock().await;
    match pulls.list(&repo, 100) {
        Ok(values) => {
            let visible: Vec<_> = values
                .into_iter()
                .filter(|pull| {
                    claims.allows_read(&pull.head_ref) || claims.allows_read(&pull.base_ref)
                })
                .collect();
            Json(json!({ "repo": repo, "pulls": visible })).into_response()
        }
        Err(failure) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}")),
    }
}

async fn get_pull(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
) -> Response {
    let repo = repo_name(&owner, &repo);
    if let Err(response) = authorize(&state, &headers, &repo) {
        return response;
    }
    match pull_status(&state, &repo, number).await {
        Ok((_, status)) => Json(status).into_response(),
        Err(response) => response,
    }
}

async fn open_pull(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo)): AxumPath<(String, String)>,
    headers: HeaderMap,
    Json(request): Json<PullRequest>,
) -> Response {
    let repo = repo_name(&owner, &repo);
    let claims = match authorize(&state, &headers, &repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if !claims.allows_write(&request.head) {
        return error(
            StatusCode::FORBIDDEN,
            &format!("capability does not write {}", request.head),
        );
    }
    if let Err(response) = adopt_baseline(&state, &repo, &request.head).await {
        return response;
    }
    if let Err(response) = adopt_baseline(&state, &repo, &request.base).await {
        return response;
    }
    let (head_sha, base_sha) = {
        let refs = state.refs.lock().await;
        let head = refs.get(&repo, &request.head).ok().flatten();
        let base = refs.get(&repo, &request.base).ok().flatten();
        match (head, base) {
            (Some(head), Some(base)) => (head, base),
            (None, _) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    &format!("{} does not exist; move the ref first", request.head),
                );
            }
            (_, None) => {
                return error(
                    StatusCode::BAD_REQUEST,
                    &format!("{} does not exist; nothing to land onto", request.base),
                );
            }
        }
    };
    let mut pulls = state.pulls.lock().await;
    match pulls.open_pull(
        &repo,
        &request.head,
        &request.base,
        &request.title,
        &request.body,
        &claims.sub,
        &head_sha,
        &base_sha,
        request.draft,
        capability::now_secs(),
    ) {
        Ok(pull) => {
            let status = pulls.status(&pull, Some(&head_sha), Some(&base_sha));
            match status {
                Ok(status) => (StatusCode::CREATED, Json(status)).into_response(),
                Err(failure) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}")),
            }
        }
        Err(failure) => error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}")),
    }
}

async fn review_pull(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    Json(request): Json<ReviewRequest>,
) -> Response {
    let repo = repo_name(&owner, &repo);
    let claims = match authorize(&state, &headers, &repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if !matches!(
        request.state.as_str(),
        "approved" | "changes_requested" | "commented"
    ) {
        return error(StatusCode::BAD_REQUEST, "unknown review state");
    }
    // A reviewer may not review their own work: the author's verdict is not a
    // second opinion, and the stage would read as approved with one participant.
    let author = {
        let pulls = state.pulls.lock().await;
        match pulls.pull(&repo, number) {
            Ok(Some(pull)) => pull.author,
            Ok(None) => return error(StatusCode::NOT_FOUND, "no such pull request"),
            Err(failure) => return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}")),
        }
    };
    if author == claims.sub {
        return error(
            StatusCode::FORBIDDEN,
            "a pull request cannot be reviewed by its author",
        );
    }
    {
        let mut pulls = state.pulls.lock().await;
        if let Err(failure) = pulls.add_review(
            &repo,
            number,
            &claims.sub,
            &request.state,
            &request.body,
            capability::now_secs(),
        ) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}"));
        }
    }
    match pull_status(&state, &repo, number).await {
        Ok((_, status)) => Json(status).into_response(),
        Err(response) => response,
    }
}

async fn check_pull(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    Json(request): Json<CheckRequest>,
) -> Response {
    let repo = repo_name(&owner, &repo);
    if let Err(response) = authorize(&state, &headers, &repo) {
        return response;
    }
    if !matches!(request.state.as_str(), "pending" | "success" | "failure") {
        return error(StatusCode::BAD_REQUEST, "unknown check state");
    }
    {
        let mut pulls = state.pulls.lock().await;
        if let Err(failure) = pulls.set_check(
            &repo,
            number,
            &request.name,
            &request.state,
            capability::now_secs(),
        ) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}"));
        }
    }
    match pull_status(&state, &repo, number).await {
        Ok((_, status)) => Json(status).into_response(),
        Err(response) => response,
    }
}

/// Take the forge's tip as our baseline for a ref we have not touched yet.
///
/// A PR's base is usually a branch this service has never moved (`main`), so
/// "does not exist" would be a lie about a branch git happily shows.
async fn adopt_baseline(state: &ForgeState, repo: &str, name: &str) -> Result<(), Response> {
    let known = {
        let refs = state.refs.lock().await;
        refs.get(repo, name).ok().flatten()
    };
    if known.is_some() {
        return Ok(());
    }
    match state.forge.current_ref(repo, name).await {
        Ok(Some(tip)) => {
            let mut refs = state.refs.lock().await;
            refs.adopt(repo, name, &tip, capability::now_secs())
                .map_err(|failure| {
                    error(StatusCode::INTERNAL_SERVER_ERROR, &format!("refs: {failure:#}"))
                })
                .map(|_| ())
        }
        Ok(None) => Ok(()),
        Err(failure) => Err(error(StatusCode::BAD_GATEWAY, &format!("{failure:#}"))),
    }
}

/// Land a pull request: the same compare-and-swap, with the same policy.
///
/// The stage must be `merge` — approved, green, and not stale — so a merge cannot
/// quietly land a diff nobody reviewed or one that moved under the reviewer.
async fn merge_pull(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    Json(request): Json<MergeRequest>,
) -> Response {
    let repo = repo_name(&owner, &repo);
    let claims = match authorize(&state, &headers, &repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    if !claims.write.iter().any(|pattern| pattern == "*" || pattern.starts_with("refs/heads/")) {
        return error(StatusCode::FORBIDDEN, "this capability is read-only");
    }
    // The base may be forge-backed on the first landing.
    let base_ref = {
        let pulls = state.pulls.lock().await;
        match pulls.pull(&repo, number) {
            Ok(Some(pull)) => Some(pull.base_ref),
            Ok(None) => None,
            Err(failure) => {
                return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}"));
            }
        }
    };
    if let Some(base_ref) = &base_ref {
        if let Err(response) = adopt_baseline(&state, &repo, base_ref).await {
            return response;
        }
    }
    let (pull, status) = match pull_status(&state, &repo, number).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    if status.stage != Stage::Merge {
        return error(
            StatusCode::CONFLICT,
            &format!("stage is {}: {}", stage_name(status.stage), status.reason),
        );
    }
    if pull.head_sha != request.head_sha {
        return error(
            StatusCode::CONFLICT,
            &format!("pull request is about {}, not {}", pull.head_sha, request.head_sha),
        );
    }
    // The landing is a ref update, so policy applies here too: an agent subject
    // may not move a protected base ref even with a green, approved PR.
    let mut refs = state.refs.lock().await;
    let outcome = refs.update(
        &repo,
        &pull.base_ref,
        Some(&pull.base_sha),
        &pull.head_sha,
        &claims.sub,
        &claims.nonce,
        capability::now_secs(),
    );
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(failure) => return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}")),
    };
    match outcome {
        Outcome::Applied { .. } => {
            let mut pulls = state.pulls.lock().await;
            if let Err(failure) = pulls.mark_merged(
                &repo,
                number,
                &claims.sub,
                &pull.head_sha,
                capability::now_secs(),
            ) {
                return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}"));
            }
            drop(pulls);
            drop(refs);
            match pull_status(&state, &repo, number).await {
                Ok((_, status)) => Json(status).into_response(),
                Err(response) => response,
            }
        }
        Outcome::Stale { current, detail } => (
            StatusCode::CONFLICT,
            Json(json!({ "error": detail, "current": current })),
        )
            .into_response(),
        Outcome::Denied { detail } => {
            (StatusCode::FORBIDDEN, Json(json!({ "error": detail }))).into_response()
        }
    }
}

/// Mark a draft ready, or back. Only the author moves the draft flag: it is the
/// author's statement about their own work.
async fn update_pull(
    State(state): State<Arc<ForgeState>>,
    AxumPath((owner, repo, number)): AxumPath<(String, String, i64)>,
    headers: HeaderMap,
    Json(request): Json<DraftRequest>,
) -> Response {
    let repo = repo_name(&owner, &repo);
    let claims = match authorize(&state, &headers, &repo) {
        Ok(claims) => claims,
        Err(response) => return response,
    };
    {
        let pulls = state.pulls.lock().await;
        let pull = match pulls.pull(&repo, number) {
            Ok(Some(pull)) => pull,
            Ok(None) => return error(StatusCode::NOT_FOUND, "no such pull request"),
            Err(failure) => {
                return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}"));
            }
        };
        if pull.author != claims.sub {
            return error(
                StatusCode::FORBIDDEN,
                "only the author moves a pull request between draft and ready",
            );
        }
        if pull.state != "open" {
            return error(StatusCode::CONFLICT, "pull request is not open");
        }
    }
    {
        let mut pulls = state.pulls.lock().await;
        if let Err(failure) = pulls.set_draft(&repo, number, request.draft) {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &format!("{failure:#}"));
        }
    }
    match pull_status(&state, &repo, number).await {
        Ok((_, status)) => Json(status).into_response(),
        Err(response) => response,
    }
}

#[derive(Debug, Deserialize)]
pub struct DraftRequest {
    pub draft: bool,
}

async fn pull_status(
    state: &ForgeState,
    repo: &str,
    number: i64,
) -> Result<(crate::pulls::Pull, crate::pulls::PullStatus), Response> {
    let (pull, head_now, base_now) = {
        let pulls = state.pulls.lock().await;
        let pull = match pulls.pull(repo, number) {
            Ok(Some(pull)) => pull,
            Ok(None) => return Err(error(StatusCode::NOT_FOUND, "no such pull request")),
            Err(failure) => {
                return Err(error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("{failure:#}"),
                ));
            }
        };
        let refs = state.refs.lock().await;
        let head_now = refs.get(repo, &pull.head_ref).ok().flatten();
        let base_now = refs.get(repo, &pull.base_ref).ok().flatten();
        (pull, head_now, base_now)
    };
    let pulls = state.pulls.lock().await;
    match pulls.status(&pull, head_now.as_deref(), base_now.as_deref()) {
        Ok(status) => Ok((pull, status)),
        Err(failure) => Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("{failure:#}"),
        )),
    }
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Agent => "agent",
        Stage::Draft => "draft",
        Stage::Review => "review",
        Stage::Changes => "changes",
        Stage::Ci => "ci",
        Stage::Merge => "merge",
        Stage::Merged => "merged",
        Stage::Closed => "closed",
        Stage::Stale => "stale",
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
