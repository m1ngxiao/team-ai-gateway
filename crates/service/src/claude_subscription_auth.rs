use codexmanager_core::auth::{generate_pkce, generate_state};
use codexmanager_core::storage::{
    now_ts, ClaudeSubscriptionAccount, ClaudeSubscriptionLoginSession, Storage,
};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;
use url::Url;

use crate::storage_helpers::open_storage;

const AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const REDIRECT_URI: &str = "https://platform.claude.com/oauth/code/callback";
const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const SCOPES: &str = "org:create_api_key user:profile user:inference user:sessions:claude_code user:mcp_servers user:file_upload user:plugins";

static CLAUDE_ACCOUNT_REFRESH_LOCKS: OnceLock<Mutex<HashMap<String, Weak<Mutex<()>>>>> =
    OnceLock::new();

fn account_refresh_lock(account_id: &str) -> Arc<Mutex<()>> {
    let table = CLAUDE_ACCOUNT_REFRESH_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut entries = crate::lock_utils::lock_recover(table, "claude_account_refresh_locks");
    if let Some(lock) = entries.get(account_id).and_then(Weak::upgrade) {
        return lock;
    }
    entries.retain(|_, lock| lock.strong_count() > 0);
    let lock = Arc::new(Mutex::new(()));
    entries.insert(account_id.to_string(), Arc::downgrade(&lock));
    lock
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeLoginStart {
    login_id: String,
    auth_url: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeLoginComplete {
    account_id: String,
    status: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeAccountSummary {
    id: String,
    label: String,
    email: Option<String>,
    organization_uuid: Option<String>,
    subscription_type: Option<String>,
    status: String,
    sort: i64,
    expires_at: i64,
    last_error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthIdentity {
    uuid: String,
    #[serde(default)]
    email_address: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthOrganization {
    uuid: String,
    #[serde(default)]
    organization_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    expires_in: i64,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    account: Option<OAuthIdentity>,
    #[serde(default)]
    organization: Option<OAuthOrganization>,
}

#[derive(Debug, Deserialize)]
struct OAuthProfile {
    account: OAuthIdentity,
    organization: OAuthOrganization,
}

fn oauth_http_client(account_id: Option<&str>) -> Result<Client, String> {
    // Login has no account identity yet, so use a stable pool assignment for its
    // token exchange and profile request. Refresh uses the actual account ID.
    crate::gateway::upstream_client_for_account(account_id.unwrap_or("claude:oauth-login"))
        .map_err(|_| "Claude upstream HTTP client unavailable; check proxy configuration".to_string())
}

fn build_authorize_url(state: &str, code_challenge: &str) -> Result<String, String> {
    let mut url = Url::parse(AUTHORIZE_URL).map_err(|err| err.to_string())?;
    url.query_pairs_mut()
        .append_pair("code", "true")
        .append_pair("client_id", CLIENT_ID)
        .append_pair("response_type", "code")
        .append_pair("redirect_uri", REDIRECT_URI)
        .append_pair("scope", SCOPES)
        .append_pair("code_challenge", code_challenge)
        .append_pair("code_challenge_method", "S256")
        .append_pair("state", state);
    Ok(url.into())
}

pub(crate) fn login_start() -> Result<ClaudeLoginStart, String> {
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let state = generate_state();
    let pkce = generate_pkce();
    let now = now_ts();
    storage
        .insert_claude_subscription_login_session(&ClaudeSubscriptionLoginSession {
            id: state.clone(),
            state: state.clone(),
            code_verifier: pkce.code_verifier,
            status: "pending".to_string(),
            error: None,
            account_id: None,
            created_at: now,
            updated_at: now,
        })
        .map_err(|err| format!("save Claude login session failed: {err}"))?;
    Ok(ClaudeLoginStart {
        login_id: state.clone(),
        auth_url: build_authorize_url(&state, &pkce.code_challenge)?,
    })
}

fn parse_code_and_state(input: &str) -> Result<(&str, &str), String> {
    let (code, state) = input.trim().split_once('#').ok_or_else(|| {
        "Claude authorization code must include the #state suffix shown by the browser"
            .to_string()
    })?;
    if code.trim().is_empty() || state.trim().is_empty() {
        return Err("Claude authorization code or state is empty".to_string());
    }
    Ok((code.trim(), state.trim()))
}

fn exchange_code(client: &Client, code: &str, session: &ClaudeSubscriptionLoginSession) -> Result<OAuthTokenResponse, String> {
    let response = client
        .post(TOKEN_URL)
        .timeout(Duration::from_secs(30))
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": REDIRECT_URI,
            "client_id": CLIENT_ID,
            "code_verifier": session.code_verifier,
            "state": session.state,
        }))
        .send()
        .map_err(|err| format!("Claude token exchange failed: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("Claude token exchange returned HTTP {}", response.status()));
    }
    response
        .json()
        .map_err(|err| format!("invalid Claude token response: {err}"))
}

fn fetch_profile(client: &Client, access_token: &str) -> Result<OAuthProfile, String> {
    let response = client
        .get(PROFILE_URL)
        .timeout(Duration::from_secs(30))
        .bearer_auth(access_token)
        .header("Cache-Control", "no-cache")
        .send()
        .map_err(|err| format!("Claude profile request failed: {err}"))?;
    if !response.status().is_success() {
        return Err(format!("Claude profile returned HTTP {}", response.status()));
    }
    response
        .json()
        .map_err(|err| format!("invalid Claude profile response: {err}"))
}

pub(crate) fn login_complete(login_id: &str, pasted_code: &str) -> Result<ClaudeLoginComplete, String> {
    let (code, state) = parse_code_and_state(pasted_code)?;
    if state != login_id.trim() {
        return Err("Claude authorization state mismatch".to_string());
    }
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    let session = storage
        .claim_claude_subscription_login_session(login_id, state, now_ts())
        .map_err(|err| format!("claim Claude login session failed: {err}"))?
        .ok_or_else(|| "Claude login session expired or was already used".to_string())?;
    let completed = (|| {
        let client = oauth_http_client(None)?;
        let token = exchange_code(&client, code, &session)?;
        if token.access_token.trim().is_empty() || token.expires_in <= 0 {
            return Err("Claude token exchange returned unusable credentials".to_string());
        }
        let refresh_token = token.refresh_token.as_deref().filter(|value| !value.is_empty())
            .ok_or_else(|| "Claude login did not provide a refresh token".to_string())?;
        let profile = fetch_profile(&client, &token.access_token)?;
        let account_uuid = profile.account.uuid.trim();
        let organization_uuid = profile.organization.uuid.trim();
        if account_uuid.is_empty() || organization_uuid.is_empty() {
            return Err("Claude profile is missing account or organization identity".to_string());
        }
        let account_id = format!("claude:{account_uuid}:{organization_uuid}");
        let now = now_ts();
        let label = profile.account.email_address.as_deref().filter(|value| !value.trim().is_empty())
            .unwrap_or(account_uuid).to_string();
        let existing = storage.find_claude_subscription_account(&account_id)
            .map_err(|err| format!("read Claude account failed: {err}"))?;
        storage.upsert_claude_subscription_account(&ClaudeSubscriptionAccount {
            id: account_id.clone(),
            label,
            email: profile.account.email_address.or_else(|| token.account.and_then(|identity| identity.email_address)),
            account_uuid: Some(account_uuid.to_string()),
            organization_uuid: Some(organization_uuid.to_string()),
            subscription_type: profile.organization.organization_type.or_else(|| token.organization.and_then(|organization| organization.organization_type)),
            status: "disabled".to_string(),
            sort: existing.as_ref().map_or(0, |account| account.sort),
            access_token: token.access_token,
            refresh_token: refresh_token.to_string(),
            scopes: token.scope.unwrap_or_else(|| SCOPES.to_string()),
            expires_at: now.saturating_add(token.expires_in),
            last_error: None,
            created_at: existing.as_ref().map_or(now, |account| account.created_at),
            updated_at: now,
        }).map_err(|err| format!("save Claude account failed: {err}"))?;
        let status = storage.find_claude_subscription_account(&account_id)
            .map_err(|err| format!("read Claude account after login failed: {err}"))?
            .ok_or_else(|| "Claude account disappeared after login".to_string())?
            .status;
        if !storage.finish_claude_subscription_login_session(login_id, "completed", None, Some(&account_id), now)
            .map_err(|err| format!("finish Claude login failed: {err}"))? {
            return Err("Claude login session expired during completion".to_string());
        }
        Ok(ClaudeLoginComplete { account_id, status })
    })();
    if let Err(error) = &completed {
        let _ = storage.finish_claude_subscription_login_session(login_id, "failed", Some(error), None, now_ts());
    }
    completed
}

pub(crate) fn list_accounts() -> Result<Vec<ClaudeAccountSummary>, String> {
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    storage.list_claude_subscription_accounts()
        .map_err(|err| format!("list Claude accounts failed: {err}"))
        .map(|accounts| accounts.into_iter().map(|account| ClaudeAccountSummary {
            id: account.id,
            label: account.label,
            email: account.email,
            organization_uuid: account.organization_uuid,
            subscription_type: account.subscription_type,
            status: account.status,
            sort: account.sort,
            expires_at: account.expires_at,
            last_error: account.last_error,
        }).collect())
}

pub(crate) fn set_account_status(account_id: &str, status: &str) -> Result<(), String> {
    if !matches!(status, "active" | "disabled") {
        return Err("invalid Claude account status".to_string());
    }
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    if status == "active" {
        let account = storage.find_claude_subscription_account(account_id)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| "Claude account not found".to_string())?;
        if account.access_token.is_empty() || account.refresh_token.is_empty() {
            return Err("Claude account has no usable credentials".to_string());
        }
        if account.status == "needs_login" {
            return Err("Claude account needs a new login before activation".to_string());
        }
    }
    if !storage.update_claude_subscription_account_status(account_id, status, now_ts())
        .map_err(|err| err.to_string())? {
        return Err("Claude account not found".to_string());
    }
    Ok(())
}

pub(crate) fn delete_account(account_id: &str) -> Result<(), String> {
    let storage = open_storage().ok_or_else(|| "storage unavailable".to_string())?;
    if !storage.delete_claude_subscription_account(account_id).map_err(|err| err.to_string())? {
        return Err("Claude account not found".to_string());
    }
    Ok(())
}

enum RefreshFailure {
    Authentication(u16),
    Other(String),
}

fn request_refreshed_token(account: &ClaudeSubscriptionAccount) -> Result<OAuthTokenResponse, RefreshFailure> {
    let response = oauth_http_client(Some(&account.id))
        .map_err(RefreshFailure::Other)?
        .post(TOKEN_URL)
        .timeout(Duration::from_secs(30))
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": account.refresh_token,
            "client_id": CLIENT_ID,
            "scope": account.scopes,
        }))
        .send()
        .map_err(|err| RefreshFailure::Other(format!("Claude token refresh failed: {err}")))?;
    if !response.status().is_success() {
        let status = response.status().as_u16();
        if matches!(status, 400 | 401) {
            return Err(RefreshFailure::Authentication(status));
        }
        return Err(RefreshFailure::Other(format!("Claude token refresh returned HTTP {status}")));
    }
    response.json()
        .map_err(|err| RefreshFailure::Other(format!("invalid Claude token refresh response: {err}")))
}

fn newer_active_account(
    storage: &Storage,
    previous: &ClaudeSubscriptionAccount,
) -> Result<Option<ClaudeSubscriptionAccount>, String> {
    let latest = storage.find_claude_subscription_account(&previous.id)
        .map_err(|err| format!("read Claude account after refresh failed: {err}"))?;
    Ok(latest.filter(|account| {
        account.status == "active"
            && (account.access_token != previous.access_token
                || account.refresh_token != previous.refresh_token)
    }))
}

fn refresh_account_token_with<F>(
    storage: &Storage,
    account: &ClaudeSubscriptionAccount,
    fetch_token: F,
) -> Result<ClaudeSubscriptionAccount, String>
where
    F: FnOnce(&ClaudeSubscriptionAccount) -> Result<OAuthTokenResponse, RefreshFailure>,
{
    let lock = account_refresh_lock(&account.id);
    let _guard = crate::lock_utils::lock_recover(lock.as_ref(), "claude_account_refresh_lock");
    let current = storage.find_claude_subscription_account(&account.id)
        .map_err(|err| format!("read Claude account before refresh failed: {err}"))?
        .ok_or_else(|| "Claude account was deleted before refresh".to_string())?;
    if current.status == "disabled" || account.status != "active" {
        return Err("Claude account was disabled before refresh".to_string());
    }
    if current.access_token != account.access_token
        || current.refresh_token != account.refresh_token {
        return if current.status == "active" {
            Ok(current)
        } else {
            Err("Claude account needs a new login".to_string())
        };
    }
    if current.status != "active" && current.status != "needs_login" {
        return Err("Claude account is unavailable for refresh".to_string());
    }
    if current.status == "active" && current.expires_at > now_ts().saturating_add(60) {
        return Ok(current);
    }

    let token = match fetch_token(&current) {
        Ok(token) => token,
        Err(RefreshFailure::Authentication(status)) => {
            let _ = storage.mark_claude_subscription_account_needs_login(
                &current.id, &current.access_token, &current.refresh_token,
                "Claude login expired", now_ts(),
            ).map_err(|err| format!("update Claude login status failed: {err}"))?;
            if let Some(latest) = newer_active_account(storage, &current)? {
                return Ok(latest);
            }
            return Err(format!("Claude token refresh returned HTTP {status}"));
        }
        Err(RefreshFailure::Other(error)) => {
            if let Some(latest) = newer_active_account(storage, &current)? {
                return Ok(latest);
            }
            return Err(error);
        }
    };
    if token.access_token.is_empty() || token.expires_in <= 0 {
        return Err("Claude token refresh returned unusable credentials".to_string());
    }
    let now = now_ts();
    let next_refresh = token.refresh_token.as_deref().filter(|value| !value.is_empty())
        .unwrap_or(&current.refresh_token);
    let next_scopes = token.scope.as_deref().filter(|value| !value.is_empty())
        .unwrap_or(&current.scopes);
    let rotated = storage.rotate_claude_subscription_token(
        &current.id, &current.access_token, &current.refresh_token, &token.access_token,
        next_refresh, next_scopes, now.saturating_add(token.expires_in), now,
    ).map_err(|err| format!("save Claude token refresh failed: {err}"))?;
    if !rotated {
        if let Some(latest) = newer_active_account(storage, &current)? {
            return Ok(latest);
        }
        return Err("Claude account changed during refresh".to_string());
    }
    let current = storage.find_claude_subscription_account(&account.id)
        .map_err(|err| format!("read refreshed Claude account failed: {err}"))?
        .ok_or_else(|| "Claude account was deleted during refresh".to_string())?;
    if current.status != "active" {
        return Err("Claude account was disabled during refresh".to_string());
    }
    Ok(current)
}

pub(crate) fn refresh_account_token(
    storage: &Storage,
    account: &ClaudeSubscriptionAccount,
) -> Result<ClaudeSubscriptionAccount, String> {
    refresh_account_token_with(storage, account, request_refreshed_token)
}

pub(crate) fn mark_account_needs_login_after_401(
    storage: &Storage,
    account_id: &str,
    expected_access_token: &str,
    expected_refresh_token: &str,
) -> Result<bool, String> {
    let lock = account_refresh_lock(account_id);
    let _guard = crate::lock_utils::lock_recover(lock.as_ref(), "claude_account_refresh_lock");
    let current = storage.find_claude_subscription_account(account_id)
        .map_err(|err| format!("read Claude account after HTTP 401 failed: {err}"))?;
    let Some(current) = current else { return Ok(false); };
    if current.status != "active"
        || current.access_token != expected_access_token
        || current.refresh_token != expected_refresh_token {
        return Ok(false);
    }
    storage.mark_claude_subscription_account_needs_login(
        account_id, expected_access_token, expected_refresh_token,
        "Claude upstream rejected subscription login", now_ts(),
    ).map_err(|err| format!("mark Claude account after HTTP 401 failed: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expired_account(id: &str) -> ClaudeSubscriptionAccount {
        ClaudeSubscriptionAccount {
            id: id.to_string(),
            label: id.to_string(),
            email: None,
            account_uuid: None,
            organization_uuid: None,
            subscription_type: None,
            status: "active".to_string(),
            sort: 0,
            access_token: "access-old".to_string(),
            refresh_token: "refresh-old".to_string(),
            scopes: "user:inference".to_string(),
            expires_at: 1,
            last_error: None,
            created_at: 1,
            updated_at: 1,
        }
    }

    fn refreshed_token() -> OAuthTokenResponse {
        OAuthTokenResponse {
            access_token: "access-new".to_string(),
            refresh_token: Some("refresh-new".to_string()),
            expires_in: 3600,
            scope: None,
            account: None,
            organization: None,
        }
    }

    #[test]
    fn manual_code_must_match_session_state() {
        assert_eq!(parse_code_and_state("authorization#state").unwrap(), ("authorization", "state"));
        assert!(parse_code_and_state("authorization").is_err());
        assert!(parse_code_and_state("authorization#").is_err());
    }

    #[test]
    fn authorize_url_contains_pkce_and_manual_callback() {
        let url = Url::parse(&build_authorize_url("state-value", "challenge-value").unwrap()).unwrap();
        let params = url.query_pairs().collect::<std::collections::HashMap<_, _>>();
        assert_eq!(params.get("state").map(|value| value.as_ref()), Some("state-value"));
        assert_eq!(params.get("code_challenge").map(|value| value.as_ref()), Some("challenge-value"));
        assert_eq!(params.get("redirect_uri").map(|value| value.as_ref()), Some(REDIRECT_URI));
    }

    #[test]
    fn concurrent_refreshes_use_new_token_after_first_refresh() {
        use std::sync::mpsc;
        use std::thread;

        let path = std::env::temp_dir().join(format!("team-ai-claude-refresh-{}.db", generate_state()));
        let account_id = format!("claude:test:{}", generate_state());
        let original = expired_account(&account_id);
        let storage = Storage::open(&path).expect("open test database");
        storage.init().expect("initialize test database");
        storage.upsert_claude_subscription_account(&original).expect("insert account");
        drop(storage);

        let (first_started_tx, first_started_rx) = mpsc::channel();
        let (release_first_tx, release_first_rx) = mpsc::channel();
        let first_path = path.clone();
        let first_original = original.clone();
        let first = thread::spawn(move || {
            let storage = Storage::open(first_path).expect("open first connection");
            refresh_account_token_with(&storage, &first_original, |_| {
                first_started_tx.send(()).expect("signal first refresh");
                release_first_rx.recv().expect("release first refresh");
                Ok(refreshed_token())
            })
        });
        first_started_rx.recv().expect("wait for first refresh to hold account lock");

        let second_path = path.clone();
        let second_original = original.clone();
        let second = thread::spawn(move || {
            let storage = Storage::open(second_path).expect("open second connection");
            refresh_account_token_with(&storage, &second_original, |_| {
                Err(RefreshFailure::Authentication(400))
            })
        });
        release_first_tx.send(()).expect("complete first refresh");
        let first_result = first.join().expect("join first request").expect("first refresh succeeds");
        let second_result = second.join().expect("join second request").expect("second reuses fresh token");
        assert_eq!(first_result.access_token, "access-new");
        assert_eq!(second_result.access_token, "access-new");
        assert_eq!(second_result.status, "active");
        let storage = Storage::open(&path).expect("reopen test database");
        assert_eq!(storage.find_claude_subscription_account(&account_id).unwrap().unwrap().status, "active");
        drop(storage);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unauthorized_before_refresh_can_be_recovered_but_admin_disable_cannot() {
        let storage = Storage::open_in_memory().expect("open database");
        storage.init().expect("initialize database");
        let original = expired_account(&format!("claude:test:{}", generate_state()));
        storage.upsert_claude_subscription_account(&original).expect("insert account");
        assert!(mark_account_needs_login_after_401(
            &storage, &original.id, &original.access_token, &original.refresh_token,
        ).expect("mark old token unauthorized"));

        let recovered = refresh_account_token_with(&storage, &original, |_| Ok(refreshed_token()))
            .expect("refresh after old-token 401");
        assert_eq!(recovered.status, "active");
        assert_eq!(recovered.access_token, "access-new");
        assert!(!mark_account_needs_login_after_401(
            &storage, &original.id, &original.access_token, &original.refresh_token,
        ).expect("stale old-token 401"));
        assert!(storage.update_claude_subscription_account_status(&original.id, "disabled", now_ts())
            .expect("admin disables account"));
        assert!(refresh_account_token_with(&storage, &recovered, |_| Ok(refreshed_token())).is_err());
        assert_eq!(storage.find_claude_subscription_account(&original.id).unwrap().unwrap().status, "disabled");
    }
}
