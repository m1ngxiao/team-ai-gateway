use crate::commands::shared::rpc_call_in_background;

#[tauri::command]
pub async fn service_claude_account_list(addr: Option<String>) -> Result<serde_json::Value, String> {
    rpc_call_in_background("claudeAccount/list", addr, None).await
}

#[tauri::command]
pub async fn service_claude_account_login_start(addr: Option<String>) -> Result<serde_json::Value, String> {
    rpc_call_in_background("claudeAccount/loginStart", addr, None).await
}

#[tauri::command]
pub async fn service_claude_account_login_complete(
    addr: Option<String>, login_id: String, code: String,
) -> Result<serde_json::Value, String> {
    rpc_call_in_background("claudeAccount/loginComplete", addr, Some(serde_json::json!({
        "loginId": login_id, "code": code,
    }))).await
}

#[tauri::command]
pub async fn service_claude_account_update_status(
    addr: Option<String>, account_id: String, status: String,
) -> Result<serde_json::Value, String> {
    rpc_call_in_background("claudeAccount/updateStatus", addr, Some(serde_json::json!({
        "accountId": account_id, "status": status,
    }))).await
}

#[tauri::command]
pub async fn service_claude_account_delete(
    addr: Option<String>, account_id: String,
) -> Result<serde_json::Value, String> {
    rpc_call_in_background("claudeAccount/delete", addr, Some(serde_json::json!({
        "accountId": account_id,
    }))).await
}
