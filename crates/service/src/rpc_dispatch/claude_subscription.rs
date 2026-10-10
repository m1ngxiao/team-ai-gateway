use codexmanager_core::rpc::types::{JsonRpcRequest, JsonRpcResponse};

use crate::claude_subscription_auth;
use crate::RpcActor;

pub(super) fn try_handle(req: &JsonRpcRequest, actor: &RpcActor) -> Option<JsonRpcResponse> {
    let result = match req.method.as_str() {
        "claudeAccount/loginStart" => {
            if !actor.is_admin() {
                super::value_or_error::<()>(Err(super::permission_denied(&req.method)))
            } else {
                super::value_or_error(claude_subscription_auth::login_start())
            }
        }
        "claudeAccount/loginComplete" => {
            if !actor.is_admin() {
                super::value_or_error::<()>(Err(super::permission_denied(&req.method)))
            } else {
                let login_id = super::str_param(req, "loginId").unwrap_or("");
                let code = super::str_param(req, "code").unwrap_or("");
                super::value_or_error(claude_subscription_auth::login_complete(login_id, code))
            }
        }
        "claudeAccount/list" => {
            if !actor.is_admin() {
                super::value_or_error::<()>(Err(super::permission_denied(&req.method)))
            } else {
                super::value_or_error(claude_subscription_auth::list_accounts())
            }
        }
        "claudeAccount/usageRefresh" => {
            if !actor.is_admin() {
                super::value_or_error::<()>(Err(super::permission_denied(&req.method)))
            } else {
                let account_id = super::str_param(req, "accountId").unwrap_or("");
                super::value_or_error(claude_subscription_auth::refresh_usage_for_account(account_id))
            }
        }
        "claudeAccount/updateStatus" => {
            if !actor.is_admin() {
                super::value_or_error::<()>(Err(super::permission_denied(&req.method)))
            } else {
                let account_id = super::str_param(req, "accountId").unwrap_or("");
                let status = super::str_param(req, "status").unwrap_or("");
                super::ok_or_error(claude_subscription_auth::set_account_status(account_id, status))
            }
        }
        "claudeAccount/delete" => {
            if !actor.is_admin() {
                super::value_or_error::<()>(Err(super::permission_denied(&req.method)))
            } else {
                let account_id = super::str_param(req, "accountId").unwrap_or("");
                super::ok_or_error(claude_subscription_auth::delete_account(account_id))
            }
        }
        _ => return None,
    };
    Some(super::response(req, result))
}
