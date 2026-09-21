use codexmanager_core::rpc::types::{JsonRpcRequest, JsonRpcResponse};

/// 函数 `try_handle`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-02
///
/// # 参数
/// - req: 参数 req
///
/// # 返回
/// 返回函数执行结果
pub(super) fn try_handle(req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
    if let Some(response) = server_only_response(req) { return Some(response); }
    let result = match req.method.as_str() {
        "codexProfile/get" => super::value_or_error(crate::codex_profile::get_status(
            super::str_param(req, "codexHome"),
        )),
        "codexProfile/setConfig" => super::value_or_error(crate::codex_profile::set_config(
            super::str_param(req, "codexHome"),
        )),
        "codexProfile/listCandidates" => {
            super::value_or_error(crate::codex_profile::list_candidates())
        }
        "codexProfile/applyDirectAccount" => {
            super::value_or_error(crate::codex_profile::apply_direct_account(
                super::str_param(req, "accountId"),
                super::str_param(req, "codexHome"),
                super::bool_param(req, "reloadAfterSwitch").unwrap_or(false),
            ))
        }
        "codexProfile/applyGateway" => super::value_or_error(crate::codex_profile::apply_gateway(
            super::str_param(req, "apiKeyId"),
            super::str_param(req, "codexHome"),
            super::str_param(req, "baseUrl"),
            super::bool_param(req, "supportsWebsockets"),
            super::bool_param(req, "reloadAfterSwitch").unwrap_or(false),
        )),
        "codexProfile/restore" => super::value_or_error(crate::codex_profile::restore(
            super::str_param(req, "codexHome"),
        )),
        "codexProfile/repairHistory" => super::value_or_error(
            crate::codex_profile::repair_history(super::str_param(req, "codexHome")),
        ),
        "codexProfile/pruneHistoryBackups" => super::value_or_error(
            crate::codex_profile::prune_history_backups(super::str_param(req, "codexHome")),
        ),
        _ => return None,
    };

    Some(super::response(req, result))
}

// Docker-only overlay. The shared gateway does not own client configuration.
// Block reads too: upstream get_status may migrate profile files and backups.
fn server_only_response(req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
    if !req.method.starts_with("codexProfile/") {
        return None;
    }
    Some(super::response(
        req,
        super::value_or_error::<serde_json::Value>(Err(
            "permission_denied: client profile management is disabled in this Docker server deployment".to_string(),
        )),
    ))
}

#[cfg(test)]
mod server_only_tests {
    use super::*;
    use codexmanager_core::storage::Storage;

    #[test]
    fn server_only_rejects_all_profile_operations_without_using_parameters() {
        for method in ["get", "setConfig", "listCandidates", "applyDirectAccount", "applyGateway",
            "restore", "repairHistory", "pruneHistoryBackups", "futureOperation"] {
            let req = JsonRpcRequest {
                id: 42.into(), method: format!("codexProfile/{method}"),
                params: Some(serde_json::json!({"codexHome": "/must-not-be-created", "reloadAfterSwitch": true})),
                trace: None,
            };
            let response = try_handle(&req).expect("profile operation must be blocked");
            assert_eq!(response.id, req.id);
            assert!(response.result.to_string().contains("permission_denied"));
        }
    }

    #[test]
    fn server_only_does_not_intercept_gateway_or_account_operations() {
        for method in ["account/list", "apikey/list", "model/list", "initialize", "codexProfileOther/get"] {
            let req = JsonRpcRequest { id: 1.into(), method: method.to_string(), params: None, trace: None };
            assert!(try_handle(&req).is_none());
        }
    }

    #[test]
    fn server_only_never_resolves_client_paths_or_syncs_imported_profiles() {
        for path in [None, Some(r"C:\Users\example\.codex"), Some("/data/.codex")] {
            assert!(crate::codex_profile::resolve_profile_dir(path).unwrap_err().contains("permission_denied"));
        }
        let storage = Storage::open_in_memory().unwrap();
        assert!(!crate::codex_profile::sync_active_gateway_profile_from_storage(&storage).unwrap());
        assert!(!crate::codex_profile::sync_active_gateway_profile_for_api_key(&storage, "missing-key").unwrap());
    }
}
