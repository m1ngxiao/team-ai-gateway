use super::{
    binding_status, derive_final_error, derive_status_for_log, is_client_disconnect_error,
    should_mark_stream_network_failure,
};
use crate::gateway::upstream::semantic_health::SemanticHealth;

#[test]
fn delivered_http_success_with_failed_body_cannot_rebind_the_conversation() {
    let logged = derive_status_for_log(200, Some(200), false, false, true, false);
    assert_eq!(logged, 200);
    assert_eq!(binding_status(logged, false), 502);
    assert_eq!(binding_status(200, true), 200);
    assert_eq!(binding_status(429, false), 429);
    assert_eq!(binding_status(499, false), 499);
}

#[test]
fn derive_final_error_prefers_upstream_hint_then_http_error_then_bridge_error() {
    assert_eq!(
        derive_final_error(
            429,
            Some("last attempt"),
            Some("upstream hint"),
            Some("bridge error".to_string()),
        )
        .as_deref(),
        Some("upstream hint")
    );
    assert_eq!(
        derive_final_error(
            429,
            Some("last attempt"),
            None,
            Some("bridge error".to_string())
        )
        .as_deref(),
        Some("last attempt")
    );
    assert_eq!(
        derive_final_error(200, None, None, Some("bridge error".to_string())).as_deref(),
        Some("bridge error")
    );
}

#[test]
fn derive_status_for_log_respects_disconnect_delivery_and_bridge_fallbacks() {
    assert_eq!(
        derive_status_for_log(200, None, true, false, false, true),
        499
    );
    assert_eq!(
        derive_status_for_log(200, Some(207), true, false, false, false),
        207
    );
    assert_eq!(
        derive_status_for_log(404, None, true, false, false, false),
        404
    );
    assert_eq!(
        derive_status_for_log(200, None, true, true, false, false),
        502
    );
    assert_eq!(
        derive_status_for_log(200, None, false, false, false, false),
        502
    );
    assert_eq!(
        derive_status_for_log(200, None, true, false, false, false),
        200
    );
}

#[test]
fn client_disconnect_error_matches_common_socket_messages() {
    assert!(is_client_disconnect_error("broken pipe"));
    assert!(is_client_disconnect_error("connection reset by peer"));
    assert!(!is_client_disconnect_error("upstream timeout"));
}

#[test]
fn downstream_disconnect_before_terminal_does_not_penalize_account() {
    for message in [
        "Broken pipe (os error 32)",
        "Connection reset by peer (os error 104)",
        "Connection aborted",
        "connection was forcibly closed",
    ] {
        let client_delivery_failed = is_client_disconnect_error(message);
        assert!(client_delivery_failed, "{message}");
        // The bridge has stopped reading after the downstream write failed, so
        // no terminal event was observed. This still logs 499 and cannot rebind.
        let status = derive_status_for_log(
            200, None, false, false, true, client_delivery_failed,
        );
        assert_eq!(status, 499, "{message}");
        assert_eq!(binding_status(status, false), 499, "{message}");
        for health in [None, Some(SemanticHealth::default())] {
            assert!(!should_mark_stream_network_failure(
                true, None, client_delivery_failed, health,
            ), "{message}");
        }
    }
}

#[test]
fn incomplete_upstream_without_downstream_disconnect_still_penalizes_account() {
    for terminal_error in [None, Some("upstream stream read failed")] {
        assert!(should_mark_stream_network_failure(
            true, terminal_error, false, Some(SemanticHealth::default()),
        ));
    }
    // Unknown delivery failures remain conservative; only recognized downstream
    // disconnects can excuse an otherwise unexplained missing terminal event.
    assert!(should_mark_stream_network_failure(
        true, None, is_client_disconnect_error("upstream timeout"), None,
    ));
    assert_eq!(derive_status_for_log(200, None, false, false, true, false), 502);
}

#[test]
fn observed_upstream_failure_still_penalizes_after_downstream_disconnect() {
    assert!(should_mark_stream_network_failure(
        true, Some("upstream stream read failed"), true, None,
    ));
    // The raw health observer can see a structured failure before the bridge's
    // event collector catches up. A concurrent disconnect must not hide it.
    assert!(should_mark_stream_network_failure(
        true, None, true, Some(SemanticHealth { failed: true, ..Default::default() }),
    ));
}

#[test]
fn capacity_failure_does_not_also_trigger_account_network_cooldown() {
    let overloaded = SemanticHealth { overloaded: true, failed: true, ..Default::default() };
    for client_delivery_failed in [false, true] {
        assert!(!should_mark_stream_network_failure(
            true, Some("server_is_overloaded"), client_delivery_failed, Some(overloaded),
        ));
    }
}

#[test]
fn successful_stream_or_non_stream_delivery_does_not_penalize_account() {
    let completed = SemanticHealth { completed: true, ..Default::default() };
    assert!(!should_mark_stream_network_failure(false, None, false, Some(completed)));
    assert!(!should_mark_stream_network_failure(false, None, true, Some(completed)));
    // A non-streaming response never enters the stream-network penalty path.
    assert!(!should_mark_stream_network_failure(false, None, true, None));
}
