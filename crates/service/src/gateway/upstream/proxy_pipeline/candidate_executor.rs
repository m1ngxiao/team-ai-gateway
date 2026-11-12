use super::dynamic_failover::DynamicRequestPolicy;
use crate::gateway::dynamic_pool;
use bytes::Bytes;
use codexmanager_core::storage::{Account, Storage, Token, UsageSnapshotRecord};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;
use tiny_http::Request;

use super::super::attempt_flow::transport::UpstreamRequestContext;
use super::super::executor::CandidateUpstreamDecision;
use super::super::support::candidates::allow_openai_fallback_for_account_with_snapshot;
use super::super::support::deadline;
use super::candidate_attempt::{
    run_candidate_attempt, CandidateAttemptParams, CandidateAttemptTrace,
};
use super::candidate_state::CandidateExecutionState;
use super::execution_context::GatewayUpstreamExecutionContext;
use super::overload_retry::{
    aggressive_retry_enabled, request_is_portable, retry_delay, OverloadRetryBudget,
};
use super::request_setup::UpstreamRequestSetup;
use super::response_finalize::{
    finalize_terminal_candidate, finalize_upstream_response, respond_total_timeout,
    FinalizeUpstreamResponseOutcome,
};
use super::stream_preflight::{
    preflight_stream_response_with_dynamic_failover, StreamPreflightOutcome,
};

/// 函数 `extract_prompt_cache_key_for_trace`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-02
///
/// # 参数
/// - body: 参数 body
///
/// # 返回
/// 返回函数执行结果
fn extract_prompt_cache_key_for_trace(body: &[u8]) -> Option<String> {
    if body.is_empty() || body.len() > 64 * 1024 {
        return None;
    }
    let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    value
        .get("prompt_cache_key")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn should_forward_thread_anchor_as_prompt_cache_key(protocol_type: &str) -> bool {
    protocol_type != crate::apikey_profile::PROTOCOL_GEMINI_NATIVE
}

fn usage_snapshots_for_candidate_plans(
    storage: &Storage,
    candidates: &[(Account, Token)],
) -> HashMap<String, UsageSnapshotRecord> {
    let account_ids = candidates
        .iter()
        .filter(|(_, token)| crate::account_plan::resolve_token_account_plan(token).is_none())
        .map(|(account, _)| account.id.clone())
        .collect::<Vec<_>>();
    if account_ids.is_empty() {
        return HashMap::new();
    }

    match storage.latest_usage_snapshots_for_accounts(&account_ids) {
        Ok(snapshots) => snapshots
            .into_iter()
            .map(|snapshot| (snapshot.account_id.clone(), snapshot))
            .collect(),
        Err(err) => {
            log::warn!("gateway candidate usage snapshot prefetch failed: {err}");
            HashMap::new()
        }
    }
}

pub(in super::super) enum CandidateExecutionResult {
    Handled,
    Exhausted {
        request: Box<Request>,
        attempted_account_ids: Vec<String>,
        skipped_cooldown: usize,
        skipped_inflight: usize,
        last_attempt_url: Option<String>,
        last_attempt_error: Option<String>,
        temporary_unavailable: Option<CandidateTemporaryUnavailable>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(in super::super) struct CandidateTemporaryUnavailable {
    pub(in super::super) account_id: String,
    pub(in super::super) retry_after_secs: u64,
    pub(in super::super) reason: &'static str,
    pub(in super::super) low_quota_reset_at: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BoundAccountQuotaUnavailable {
    reset_at: Option<i64>,
}

pub(in super::super) struct CandidateExecutorParams<'a> {
    pub(in super::super) storage: &'a Storage,
    pub(in super::super) method: &'a reqwest::Method,
    pub(in super::super) incoming_headers: &'a super::super::super::IncomingHeaderSnapshot,
    pub(in super::super) body: &'a Bytes,
    pub(in super::super) original_body: &'a Bytes,
    pub(in super::super) path: &'a str,
    pub(in super::super) request_shape: Option<&'a str>,
    pub(in super::super) trace_id: &'a str,
    pub(in super::super) model_for_log: Option<&'a str>,
    pub(in super::super) response_adapter: super::super::super::ResponseAdapter,
    pub(in super::super) gemini_stream_output_mode:
        Option<super::super::super::GeminiStreamOutputMode>,
    pub(in super::super) tool_name_restore_map: &'a super::super::super::ToolNameRestoreMap,
    pub(in super::super) context: &'a GatewayUpstreamExecutionContext<'a>,
    pub(in super::super) setup: &'a UpstreamRequestSetup,
    pub(in super::super) request_deadline: Option<Instant>,
    pub(in super::super) started_at: Instant,
    pub(in super::super) client_is_stream: bool,
    pub(in super::super) upstream_is_stream: bool,
    pub(in super::super) debug: bool,
    pub(in super::super) allow_openai_fallback: bool,
    pub(in super::super) disable_challenge_stateless_retry: bool,
    pub(in super::super) has_anthropic_safeguards: bool,
    pub(in super::super) context_enrichment:
        Option<Arc<super::super::super::anthropic_context_management::AnthropicContextEnrichment>>,
}

fn record_failover_attempt(
    attempt_trace: &mut CandidateAttemptTrace,
    last_attempt_url: &mut Option<String>,
    last_attempt_error: &mut Option<String>,
) {
    super::super::super::record_gateway_failover_attempt();
    *last_attempt_url = attempt_trace.last_attempt_url.take();
    *last_attempt_error = attempt_trace.last_attempt_error.take();
}

fn prepare_next_account_candidate_client(
    ordered_account_ids: &[String],
    candidate_idx: usize,
    trace_id: &str,
) {
    let Some(next_account_id) = ordered_account_ids.get(candidate_idx + 1) else {
        return;
    };
    if let Err(err) =
        super::super::super::prepare_upstream_client_for_account(next_account_id.as_str())
    {
        log::warn!(
            "event=gateway_account_candidate_client_prepare_failed trace_id={} account_id={} err={}",
            trace_id,
            next_account_id,
            err
        );
    }
}

fn is_challenge_failover_error(error: Option<&str>) -> bool {
    error
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|value| {
            let normalized = value.to_ascii_lowercase();
            normalized.contains("challenge")
                || normalized.contains("cloudflare")
                || normalized.contains("cf_ray")
        })
}

fn should_force_strip_after_anthropic_challenge(
    context: &GatewayUpstreamExecutionContext<'_>,
    attempt_trace: &CandidateAttemptTrace,
) -> bool {
    context.protocol_type() == crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE
        && is_challenge_failover_error(attempt_trace.last_attempt_error.as_deref())
}

fn should_retry_same_account_after_failover(retry_count: u8) -> bool {
    retry_count == 0
}

fn account_has_inflight_capacity(account_id: &str, account_max_inflight: usize) -> bool {
    account_max_inflight == 0
        || super::super::super::account_inflight_count(account_id) < account_max_inflight
}

/// Keeps the one-send safety rule for non-replayable requests without treating
/// cooldown or preflight capacity failures as an attempted upstream request.
fn prepare_initial_candidates(
    candidates: Vec<(Account, Token)>,
    single_send_mode: bool,
    bound_account_id: Option<&str>,
    bound_account_quota: Option<BoundAccountQuotaUnavailable>,
    account_max_inflight: usize,
) -> (
    VecDeque<(Account, Token)>,
    Option<CandidateTemporaryUnavailable>,
) {
    if !single_send_mode {
        return (VecDeque::from(candidates), None);
    }

    if let Some(bound_account_id) = bound_account_id {
        let Some(bound_index) = candidates
            .iter()
            .position(|(account, _)| account.id == bound_account_id)
        else {
            let (reason, retry_after_secs, low_quota_reset_at) = bound_account_quota
                .map(|quota| {
                    (
                        "bound_account_low_quota",
                        quota
                            .reset_at
                            .map(|reset_at| {
                                reset_at
                                    .saturating_sub(codexmanager_core::storage::now_ts())
                                    .max(1) as u64
                            })
                            .unwrap_or(1),
                        quota.reset_at,
                    )
                })
                .unwrap_or(("bound_account_unavailable", 1, None));
            return (
                VecDeque::new(),
                Some(CandidateTemporaryUnavailable {
                    account_id: bound_account_id.to_string(),
                    retry_after_secs,
                    reason,
                    low_quota_reset_at,
                }),
            );
        };
        if let Some(retry_after_secs) =
            super::super::super::account_cooldown_remaining_secs(bound_account_id)
        {
            return (
                VecDeque::new(),
                Some(CandidateTemporaryUnavailable {
                    account_id: bound_account_id.to_string(),
                    retry_after_secs,
                    reason: "bound_account_cooldown",
                    low_quota_reset_at: None,
                }),
            );
        }
        if !account_has_inflight_capacity(bound_account_id, account_max_inflight) {
            return (
                VecDeque::new(),
                Some(CandidateTemporaryUnavailable {
                    account_id: bound_account_id.to_string(),
                    retry_after_secs: 5,
                    reason: "bound_account_inflight",
                    low_quota_reset_at: None,
                }),
            );
        }

        let mut remaining = VecDeque::from(candidates);
        remaining.rotate_left(bound_index);
        return (remaining, None);
    }

    let mut ranked = candidates
        .into_iter()
        .map(|candidate| {
            let cooldown =
                super::super::super::account_cooldown_remaining_secs(candidate.0.id.as_str());
            let ready =
                account_has_inflight_capacity(candidate.0.id.as_str(), account_max_inflight);
            (candidate, cooldown, ready)
        })
        .collect::<Vec<_>>();
    let first_ready = ranked
        .iter()
        .any(|(_, cooldown, ready)| cooldown.is_none() && *ready);
    if !first_ready {
        let cooldown = ranked
            .iter()
            .filter_map(|(_, remaining, _)| *remaining)
            .min();
        let account_id = ranked
            .iter()
            .find(|(_, remaining, _)| remaining.is_some())
            .or_else(|| ranked.first())
            .map(|((account, _), _, _)| account.id.clone())
            .unwrap_or_default();
        return (
            VecDeque::new(),
            Some(CandidateTemporaryUnavailable {
                account_id,
                retry_after_secs: cooldown.unwrap_or(1),
                reason: if cooldown.is_some() {
                    "account_pool_cooldown"
                } else {
                    "account_pool_inflight"
                },
                low_quota_reset_at: None,
            }),
        );
    }

    // Cooldown and hard in-flight checks are safe before the first upstream
    // send. Preserve routing order among ready accounts, while de-prioritizing
    // unavailable ones so a single-send request still targets a healthy account.
    ranked.sort_by_key(|(_, cooldown, ready)| (cooldown.is_some() || !*ready, *cooldown));
    (
        ranked
            .into_iter()
            .map(|(candidate, _, _)| candidate)
            .collect(),
        None,
    )
}

fn note_temporary_unavailable(
    temporary: &mut Option<CandidateTemporaryUnavailable>,
    account_id: &str,
    reason: &'static str,
) {
    if temporary.is_some() {
        return;
    }
    let mut retry_after_secs =
        super::super::super::account_cooldown_remaining_secs(account_id).unwrap_or(1);
    if reason == "bound_account_inflight" {
        retry_after_secs = retry_after_secs.max(5);
    }
    *temporary = Some(CandidateTemporaryUnavailable {
        account_id: account_id.to_string(),
        retry_after_secs,
        reason,
        low_quota_reset_at: None,
    });
}

pub(in super::super) fn account_model_override_for_request(
    storage: &Storage,
    model_for_log: Option<&str>,
) -> Option<String> {
    model_for_log
        .and_then(|model| {
            storage
                .get_enabled_model_v2(crate::models_v2::policy_catalog_slug(model))
                .ok()
                .flatten()
        })
        .and_then(|model| {
            model
                .routes
                .into_iter()
                .filter(|route| {
                    route.enabled
                        && route.source_kind == "account_pool"
                        && route.source_id == "default"
                })
                .max_by_key(|route| route.priority)
                .map(|route| route.upstream_model)
        })
        .filter(|configured_model| {
            !crate::models_v2::should_preserve_luna_reserve_alias(
                model_for_log,
                Some(configured_model.as_str()),
            )
        })
}

fn should_failover_terminal_gateway_error(
    context: &GatewayUpstreamExecutionContext<'_>,
    account_id: &str,
    has_more_candidates: bool,
    message: &str,
    attempt_trace: &mut CandidateAttemptTrace,
    last_attempt_url: &mut Option<String>,
    last_attempt_error: &mut Option<String>,
) -> bool {
    let gateway_error_follow_up =
        context.apply_gateway_error_follow_up(account_id, message, has_more_candidates);
    if !gateway_error_follow_up.should_failover {
        return false;
    }
    super::super::super::record_gateway_failover_attempt();
    *last_attempt_url = attempt_trace.last_attempt_url.take();
    *last_attempt_error = Some(message.to_string());
    true
}

#[allow(clippy::too_many_arguments)]
fn respond_terminal_attempt(
    request: Request,
    context: &GatewayUpstreamExecutionContext<'_>,
    account_id: &str,
    last_attempt_url: Option<&str>,
    status_code: u16,
    message: String,
    trace_id: &str,
    started_at: Instant,
    model_for_log: Option<&str>,
    attempted_account_ids: Option<&[String]>,
) -> Result<CandidateExecutionResult, String> {
    finalize_terminal_candidate(
        request,
        context,
        account_id,
        last_attempt_url,
        status_code,
        message,
        trace_id,
        started_at,
        model_for_log,
        attempted_account_ids,
    )?;
    Ok(CandidateExecutionResult::Handled)
}

/// 函数 `execute_candidate_sequence`
///
/// 作者: gaohongshun
///
/// 时间: 2026-04-02
///
/// # 参数
/// - in super: 参数 in super
///
/// # 返回
/// 返回函数执行结果
pub(in super::super) fn execute_candidate_sequence(
    request: Request,
    candidates: Vec<(Account, Token)>,
    params: CandidateExecutorParams<'_>,
) -> Result<CandidateExecutionResult, String> {
    let CandidateExecutorParams {
        storage,
        method,
        incoming_headers,
        body,
        original_body,
        path,
        request_shape,
        trace_id,
        model_for_log,
        response_adapter,
        gemini_stream_output_mode,
        tool_name_restore_map,
        context,
        setup,
        request_deadline,
        started_at,
        client_is_stream,
        upstream_is_stream,
        debug,
        allow_openai_fallback,
        disable_challenge_stateless_retry,
        has_anthropic_safeguards,
        context_enrichment,
    } = params;
    let mut request = Some(request);
    let mut state = CandidateExecutionState::default();
    let mut attempted_account_ids = Vec::new();
    let mut skipped_cooldown = 0usize;
    let mut skipped_inflight = 0usize;
    let mut last_attempt_url = None;
    let mut last_attempt_error = None;
    let mut force_strip_session_affinity_after_challenge = false;
    let account_model_override = account_model_override_for_request(storage, model_for_log);
    let usage_snapshots = usage_snapshots_for_candidate_plans(storage, &candidates);
    let dynamic_enabled =
        dynamic_pool::enabled() && path.split('?').next() == Some("/v1/responses");
    let aggressive_overload_failover = dynamic_enabled && aggressive_retry_enabled();
    let mut overload_budget =
        OverloadRetryBudget::with_max_backups(if aggressive_overload_failover { 2 } else { 1 });
    // Official upstream normalization may remove stored-history references. Check
    // the client payload as well so normalization cannot accidentally enable replay.
    // Aggressive mode is intentionally lossy for account-scoped state: the backup
    // body is rebased to stateless history before it is sent.
    let portable_for_overload_retry = aggressive_overload_failover
        || (request_is_portable(original_body) && request_is_portable(body));
    let single_send_mode = dynamic_enabled && !portable_for_overload_retry;
    let dynamic_policy = if dynamic_enabled {
        match DynamicRequestPolicy::load(storage, context.key_id()) {
            Ok(policy) => Some(policy),
            Err(err) => {
                log::warn!(
                    "event=gateway_dynamic_authorization_failed trace_id={} reason={}",
                    trace_id,
                    err
                );
                None
            }
        }
    } else {
        None
    };
    let effective_model = account_model_override
        .as_deref()
        .or(model_for_log)
        .unwrap_or("unknown");
    let generation_scope = super::super::generation_budget::enter(
        dynamic_enabled,
        dynamic_policy
            .as_ref()
            .map(|p| p.pool.as_str())
            .unwrap_or("unavailable"),
        effective_model,
        trace_id,
        request_deadline,
    );
    let mut reserved_backup = None;
    let ordered_account_ids = candidates
        .iter()
        .map(|(account, _)| account.id.clone())
        .collect::<Vec<_>>();
    let bound_account_id = setup
        .conversation_routing
        .as_ref()
        .and_then(|routing| routing.existing_binding.as_ref())
        .map(|binding| binding.account_id.clone());
    let bound_account_missing = bound_account_id.as_deref().is_some_and(|account_id| {
        !candidates
            .iter()
            .any(|(account, _)| account.id.as_str() == account_id)
    });
    let bound_account_quota = if bound_account_missing {
        bound_account_id
            .as_deref()
            .and_then(|account_id| {
                storage
                    .latest_usage_snapshot_for_account(account_id)
                    .ok()
                    .flatten()
            })
            .and_then(|snapshot| {
                crate::gateway::low_quota_recovery(&snapshot, codexmanager_core::storage::now_ts())
            })
            .map(|recovery| BoundAccountQuotaUnavailable {
                reset_at: recovery.reset_at,
            })
    } else {
        None
    };
    if portable_for_overload_retry && bound_account_quota.is_some() {
        log::warn!(
            "event=gateway_portable_bound_account_quota_rebind trace_id={} account_id={} model={}",
            trace_id,
            bound_account_id.as_deref().unwrap_or("-"),
            effective_model
        );
    }
    let (mut remaining, immediate_temporary_unavailable) = prepare_initial_candidates(
        candidates,
        single_send_mode,
        bound_account_id.as_deref(),
        single_send_mode.then_some(bound_account_quota).flatten(),
        context.effective_account_inflight_limit(),
    );
    if let Some(temporary_unavailable) = immediate_temporary_unavailable {
        let skipped_cooldown = usize::from(temporary_unavailable.reason.contains("cooldown"));
        let skipped_inflight = usize::from(temporary_unavailable.reason.contains("inflight"));
        return Ok(CandidateExecutionResult::Exhausted {
            request: Box::new(
                request
                    .take()
                    .ok_or_else(|| "request already consumed before preflight".to_string())?,
            ),
            attempted_account_ids,
            skipped_cooldown,
            skipped_inflight,
            last_attempt_url: None,
            last_attempt_error: None,
            temporary_unavailable: Some(temporary_unavailable),
        });
    }
    let mut temporary_unavailable = None;
    let mut candidate_idx = 0usize;
    while let Some((account, mut token)) = remaining.pop_front() {
        let idx = candidate_idx;
        candidate_idx += 1;
        // Non-replayable requests still probe every candidate for preflight
        // availability, but they may only send to the first one that reserves.
        if single_send_mode && !attempted_account_ids.is_empty() {
            break;
        }
        // Normal mode permits one backup. Aggressive mode may try every account
        // once, but a non-overload backup failure still remains terminal below.
        if attempted_account_ids.len() >= overload_budget.max_attempts() {
            break;
        }
        let has_more_candidates = !single_send_mode
            && overload_budget.has_more_candidates(if dynamic_enabled {
                attempted_account_ids.len() < overload_budget.max_attempts()
                    && ordered_account_ids.len() > attempted_account_ids.len()
            } else {
                context.has_more_candidates(idx)
            });
        let disable_challenge_stateless_retry =
            disable_challenge_stateless_retry || overload_budget.used();
        if deadline::is_expired(request_deadline) {
            let request = request
                .take()
                .ok_or_else(|| "request already consumed before timeout response".to_string())?;
            respond_total_timeout(
                request,
                context,
                trace_id,
                started_at,
                model_for_log,
                Some(attempted_account_ids.as_slice()),
            )?;
            return Ok(CandidateExecutionResult::Handled);
        }

        let strip_session_affinity = overload_budget.used()
            || force_strip_session_affinity_after_challenge
            || state.strip_session_affinity(&account, idx, setup.anthropic_has_thread_anchor);
        let attempt_thread = super::super::super::conversation_binding::resolve_attempt_thread(
            setup.conversation_routing.as_ref(),
            &account,
        );
        let attempt_headers = attempt_thread
            .as_ref()
            .map(|thread| {
                incoming_headers.with_thread_affinity_override(
                    Some(thread.thread_anchor.as_str()),
                    thread.reset_session_affinity,
                )
            })
            .unwrap_or_else(|| incoming_headers.clone());
        let attempt_model_override = account_model_override.as_deref();
        let attempt_allow_openai_fallback = !overload_budget.used()
            && allow_openai_fallback
            && allow_openai_fallback_for_account_with_snapshot(
                &token,
                usage_snapshots.get(account.id.as_str()),
            );
        let attempt_model_for_log = attempt_model_override.or(model_for_log);
        let safeguards_evaluator = has_anthropic_safeguards
            .then(|| {
                super::super::safeguards_classifier::build_gateway_safeguards_evaluator(
                    original_body,
                    &account,
                    &token,
                    incoming_headers,
                    setup.upstream_base.as_str(),
                    setup.upstream_fallback_base.as_deref(),
                    attempt_model_for_log,
                )
            })
            .flatten();
        let attempt_prompt_cache_key =
            if should_forward_thread_anchor_as_prompt_cache_key(context.protocol_type()) {
                attempt_thread
                    .as_ref()
                    .map(|thread| thread.thread_anchor.as_str())
            } else {
                None
            };
        let body_for_attempt = if aggressive_overload_failover && overload_budget.used() {
            state.aggressive_body_for_attempt(
                path,
                body,
                setup,
                attempt_model_override,
                attempt_prompt_cache_key,
            )
        } else {
            state.body_for_attempt(
                path,
                body,
                strip_session_affinity,
                setup,
                attempt_model_override,
                attempt_prompt_cache_key,
            )
        };
        context.log_candidate_start(&account.id, idx, strip_session_affinity);
        // The legacy path may try a cooling final candidate. A new overload
        // retry only selects a healthy backup, even when it is last in the pool.
        if overload_budget.used()
            && reserved_backup.is_none()
            && super::super::super::is_account_in_cooldown(&account.id)
        {
            super::super::super::record_gateway_candidate_skip(
                super::super::super::GatewayCandidateSkipReason::Cooldown,
            );
            context.log_candidate_skip(
                &account.id,
                idx,
                super::super::support::candidates::CandidateSkipReason::Cooldown,
            );
            skipped_cooldown += 1;
            if attempted_account_ids.is_empty() {
                let bound_account_unavailable =
                    single_send_mode && bound_account_id.as_deref() == Some(account.id.as_str());
                note_temporary_unavailable(
                    &mut temporary_unavailable,
                    account.id.as_str(),
                    if bound_account_unavailable {
                        "bound_account_cooldown"
                    } else {
                        "account_pool_cooldown"
                    },
                );
                if bound_account_unavailable {
                    break;
                }
            }
            continue;
        }
        if let Some(skip_reason) = if reserved_backup.is_some() {
            None
        } else {
            context.should_skip_candidate(&account.id, idx)
        } {
            context.log_candidate_skip(&account.id, idx, skip_reason);
            match skip_reason {
                super::super::support::candidates::CandidateSkipReason::Cooldown => {
                    skipped_cooldown += 1;
                    if attempted_account_ids.is_empty() {
                        let bound_account_unavailable = single_send_mode
                            && bound_account_id.as_deref() == Some(account.id.as_str());
                        note_temporary_unavailable(
                            &mut temporary_unavailable,
                            account.id.as_str(),
                            if bound_account_unavailable {
                                "bound_account_cooldown"
                            } else {
                                "account_pool_cooldown"
                            },
                        );
                        if bound_account_unavailable {
                            break;
                        }
                    }
                }
                super::super::support::candidates::CandidateSkipReason::Inflight => {
                    skipped_inflight += 1;
                    if attempted_account_ids.is_empty() {
                        let bound_account_unavailable = single_send_mode
                            && bound_account_id.as_deref() == Some(account.id.as_str());
                        note_temporary_unavailable(
                            &mut temporary_unavailable,
                            account.id.as_str(),
                            if bound_account_unavailable {
                                "bound_account_inflight"
                            } else {
                                "account_pool_inflight"
                            },
                        );
                        if bound_account_unavailable {
                            break;
                        }
                    }
                }
            }
            continue;
        }
        let mut dynamic_permit = if dynamic_enabled {
            reserved_backup.take().or_else(|| {
                dynamic_policy.as_ref().and_then(|policy| {
                    dynamic_pool::reserve_account(
                        &policy.pool,
                        effective_model,
                        &account.id,
                        context.effective_account_inflight_limit(),
                        true,
                    )
                })
            })
        } else {
            None
        };
        let account_guard = if dynamic_enabled {
            dynamic_permit
                .as_mut()
                .map(|permit| permit.take_inflight_guard())
        } else {
            super::super::super::try_acquire_account_inflight(
                &account.id,
                context.effective_account_inflight_limit(),
            )
        };
        let Some(account_guard) = account_guard else {
            let reason = super::super::support::candidates::CandidateSkipReason::Inflight;
            super::super::super::record_gateway_candidate_skip(
                super::super::super::GatewayCandidateSkipReason::Inflight,
            );
            context.log_candidate_skip(&account.id, idx, reason);
            skipped_inflight += 1;
            if attempted_account_ids.is_empty() {
                let bound_account_unavailable =
                    single_send_mode && bound_account_id.as_deref() == Some(account.id.as_str());
                note_temporary_unavailable(
                    &mut temporary_unavailable,
                    account.id.as_str(),
                    if bound_account_unavailable {
                        "bound_account_inflight"
                    } else {
                        "account_pool_inflight"
                    },
                );
                if bound_account_unavailable {
                    if let Some(temporary) = temporary_unavailable.as_mut() {
                        temporary.retry_after_secs = temporary.retry_after_secs.max(5);
                    }
                    break;
                }
            }
            continue;
        };
        let mut inflight_guard = Some(account_guard);
        prepare_next_account_candidate_client(ordered_account_ids.as_slice(), idx, trace_id);
        attempted_account_ids.push(account.id.clone());

        let request_ref = request
            .as_ref()
            .ok_or_else(|| "request already consumed".to_string())?;
        let request_ctx =
            UpstreamRequestContext::from_request(request_ref, context.protocol_type());
        let incoming_session_id = attempt_headers.session_id();
        let incoming_turn_state = attempt_headers.turn_state();
        let incoming_conversation_id = attempt_headers.conversation_id();
        let prompt_cache_key_for_trace =
            extract_prompt_cache_key_for_trace(body_for_attempt.as_ref());
        super::super::super::trace_log::log_attempt_profile(
            super::super::super::trace_log::AttemptProfileLog {
                trace_id,
                account_id: &account.id,
                candidate_index: idx,
                total: setup.candidate_count,
                strip_session_affinity,
                has_incoming_session: incoming_session_id.is_some()
                    || setup.has_sticky_fallback_session,
                has_incoming_turn_state: incoming_turn_state.is_some(),
                has_incoming_conversation: incoming_conversation_id.is_some()
                    || setup.has_sticky_fallback_conversation,
                prompt_cache_key: prompt_cache_key_for_trace.as_deref(),
                request_shape,
                body_len: body_for_attempt.len(),
                body_model: attempt_model_for_log,
            },
        );

        if let Some(permit) = dynamic_permit.as_ref() {
            log::info!("event=gateway_dynamic_account_reserved trace_id={} account_id={} model={} inflight_before={} generation={} phase={:?} backup={}",
                trace_id, account.id, effective_model, permit.inflight_before(), permit.generation(), permit.phase(), overload_budget.used());
        }
        let mut attempt_trace = CandidateAttemptTrace::default();
        let mut same_account_retry_count = 0u8;
        let mut decision = run_candidate_attempt(CandidateAttemptParams {
            storage,
            method,
            request_ctx,
            incoming_headers: &attempt_headers,
            body: &body_for_attempt,
            upstream_is_stream,
            path,
            request_deadline,
            account: &account,
            token: &mut token,
            strip_session_affinity,
            debug,
            allow_openai_fallback: attempt_allow_openai_fallback,
            disable_challenge_stateless_retry,
            has_more_candidates,
            context,
            setup,
            trace: &mut attempt_trace,
        });

        // A transient upstream error gets one retry on the same account. If that
        // retry also fails, the normal candidate failover path selects the next
        // account instead of repeatedly hammering the current one.
        if matches!(decision, CandidateUpstreamDecision::Failover)
            && !overload_budget.used()
            && should_retry_same_account_after_failover(same_account_retry_count)
        {
            same_account_retry_count += 1;
            log::warn!(
                "event=gateway_same_account_retry trace_id={} account_id={} retry={} ",
                trace_id,
                account.id,
                same_account_retry_count
            );
            attempt_trace = CandidateAttemptTrace::default();
            let request_ref = request
                .as_ref()
                .ok_or_else(|| "request already consumed before same-account retry".to_string())?;
            let retry_request_ctx =
                UpstreamRequestContext::from_request(request_ref, context.protocol_type());
            decision = run_candidate_attempt(CandidateAttemptParams {
                storage,
                method,
                request_ctx: retry_request_ctx,
                incoming_headers: &attempt_headers,
                body: &body_for_attempt,
                upstream_is_stream,
                path,
                request_deadline,
                account: &account,
                token: &mut token,
                strip_session_affinity,
                debug,
                allow_openai_fallback: attempt_allow_openai_fallback,
                disable_challenge_stateless_retry,
                has_more_candidates,
                context,
                setup,
                trace: &mut attempt_trace,
            });
        }

        match decision {
            CandidateUpstreamDecision::Failover => {
                if should_force_strip_after_anthropic_challenge(context, &attempt_trace) {
                    force_strip_session_affinity_after_challenge = true;
                    log::warn!(
                        "event=gateway_anthropic_challenge_strip_affinity trace_id={} account_id={} next_attempt_strip_session_affinity=true",
                        trace_id,
                        account.id
                    );
                }
                record_failover_attempt(
                    &mut attempt_trace,
                    &mut last_attempt_url,
                    &mut last_attempt_error,
                );
                continue;
            }
            CandidateUpstreamDecision::Terminal {
                status_code,
                message,
            } => {
                if should_failover_terminal_gateway_error(
                    context,
                    &account.id,
                    has_more_candidates,
                    &message,
                    &mut attempt_trace,
                    &mut last_attempt_url,
                    &mut last_attempt_error,
                ) {
                    if context.protocol_type() == crate::apikey_profile::PROTOCOL_ANTHROPIC_NATIVE
                        && is_challenge_failover_error(Some(message.as_str()))
                    {
                        force_strip_session_affinity_after_challenge = true;
                        log::warn!(
                            "event=gateway_anthropic_challenge_strip_affinity trace_id={} account_id={} next_attempt_strip_session_affinity=true",
                            trace_id,
                            account.id
                        );
                    }
                    continue;
                }
                let request = request.take().ok_or_else(|| {
                    "request already consumed before terminal response".to_string()
                })?;
                return respond_terminal_attempt(
                    request,
                    context,
                    &account.id,
                    attempt_trace.last_attempt_url.as_deref(),
                    status_code,
                    message,
                    trace_id,
                    started_at,
                    attempt_model_for_log,
                    Some(attempted_account_ids.as_slice()),
                );
            }
            CandidateUpstreamDecision::RespondUpstream(mut resp) => {
                if resp.status().as_u16() == 400
                    && !strip_session_affinity
                    && (incoming_turn_state.is_some() || setup.has_body_encrypted_content)
                {
                    let retry_body = state.retry_body(
                        path,
                        body,
                        setup,
                        attempt_model_override,
                        attempt_prompt_cache_key,
                    );
                    let retry_decision = run_candidate_attempt(CandidateAttemptParams {
                        storage,
                        method,
                        request_ctx,
                        incoming_headers: &attempt_headers,
                        body: &retry_body,
                        upstream_is_stream,
                        path,
                        request_deadline,
                        account: &account,
                        token: &mut token,
                        strip_session_affinity: true,
                        debug,
                        allow_openai_fallback: attempt_allow_openai_fallback,
                        disable_challenge_stateless_retry,
                        has_more_candidates,
                        context,
                        setup,
                        trace: &mut attempt_trace,
                    });

                    match retry_decision {
                        CandidateUpstreamDecision::RespondUpstream(retry_resp) => {
                            resp = retry_resp;
                        }
                        CandidateUpstreamDecision::Failover => {
                            record_failover_attempt(
                                &mut attempt_trace,
                                &mut last_attempt_url,
                                &mut last_attempt_error,
                            );
                            continue;
                        }
                        CandidateUpstreamDecision::Terminal {
                            status_code,
                            message,
                        } => {
                            let request = request.take().ok_or_else(|| {
                                "request already consumed before retry terminal response"
                                    .to_string()
                            })?;
                            return respond_terminal_attempt(
                                request,
                                context,
                                &account.id,
                                attempt_trace.last_attempt_url.as_deref(),
                                status_code,
                                message,
                                trace_id,
                                started_at,
                                attempt_model_for_log,
                                Some(attempted_account_ids.as_slice()),
                            );
                        }
                    }
                }
                let health_retry_after =
                    super::stream_preflight::retry_after_for_health(resp.headers());
                let health_observer = if dynamic_enabled {
                    let (observed, observer) = resp.observe_health();
                    resp = observed;
                    Some(observer)
                } else {
                    None
                };
                let preflight = preflight_stream_response_with_dynamic_failover(
                    resp,
                    path,
                    upstream_is_stream,
                    has_more_candidates,
                    overload_budget
                        .permits(attempted_account_ids.len(), portable_for_overload_retry),
                    dynamic_enabled,
                );
                if !matches!(&preflight, StreamPreflightOutcome::OverloadFailover { .. })
                    && health_observer
                        .as_ref()
                        .is_some_and(|o| o.snapshot().overloaded)
                {
                    if let Some(permit) = dynamic_permit.as_mut() {
                        permit.record_overload(health_retry_after);
                    }
                }
                match preflight {
                    StreamPreflightOutcome::Ready(response) => {
                        resp = response;
                    }
                    StreamPreflightOutcome::OverloadFailover {
                        message,
                        retry_after,
                        response,
                    } => {
                        if dynamic_enabled {
                            // Preserve the prefetched error if no live backup can be reserved.
                            let response = response.cancel_prefetched_stream();
                            if let Some(permit) = dynamic_permit.as_mut() {
                                permit.record_overload(retry_after);
                            }
                            drop(dynamic_permit.take());
                            if let Some(guard) = inflight_guard.as_mut() {
                                guard.release();
                            }
                            let mut selected = None;
                            if generation_scope.can_send() {
                                if let Some(delay) =
                                    retry_delay(trace_id, retry_after, request_deadline)
                                {
                                    std::thread::sleep(delay);
                                    if !deadline::is_expired(request_deadline)
                                        && generation_scope.can_send()
                                    {
                                        if let Some(policy) = dynamic_policy.as_ref() {
                                            match policy.select_backup(storage, model_for_log, effective_model, &attempted_account_ids, context.effective_account_inflight_limit()) {
                                                Ok(backup) => selected = backup,
                                                Err(err) => log::warn!("event=gateway_dynamic_backup_unavailable trace_id={} reason={}", trace_id, err),
                                            }
                                        }
                                    }
                                }
                            }
                            if let Some((candidate, permit)) =
                                selected.filter(|_| generation_scope.reserve_backup_send())
                            {
                                drop(response);
                                drop(inflight_guard.take());
                                drop(dynamic_permit.take());
                                remaining.clear();
                                remaining.push_back(candidate);
                                reserved_backup = Some(permit);
                                overload_budget.consume();
                                attempt_trace.last_attempt_error = Some(message);
                                record_failover_attempt(
                                    &mut attempt_trace,
                                    &mut last_attempt_url,
                                    &mut last_attempt_error,
                                );
                                log::warn!("event=gateway_overload_failover trace_id={} account_id={} retry=1 max_retries=1 selection=dynamic code=server_is_overloaded", trace_id, account.id);
                                continue;
                            }
                            resp = response;
                        } else {
                            if let Some(delay) =
                                retry_delay(trace_id, retry_after, request_deadline)
                            {
                                std::thread::sleep(delay);
                                if !deadline::is_expired(request_deadline) {
                                    drop(response);
                                    drop(inflight_guard.take());
                                    overload_budget.consume();
                                    super::super::super::mark_account_cooldown_for_status(
                                        &account.id,
                                        503,
                                    );
                                    super::super::super::record_route_quality(&account.id, 503);
                                    attempt_trace.last_attempt_error = Some(message);
                                    record_failover_attempt(
                                        &mut attempt_trace,
                                        &mut last_attempt_url,
                                        &mut last_attempt_error,
                                    );
                                    log::warn!("event=gateway_overload_failover trace_id={} account_id={} retry=1 max_retries=1 backoff_ms={} code=server_is_overloaded", trace_id, account.id, delay.as_millis());
                                    continue;
                                }
                            }
                            resp = response;
                        }
                    }
                    StreamPreflightOutcome::Failover(message) => {
                        if should_failover_terminal_gateway_error(
                            context,
                            &account.id,
                            has_more_candidates,
                            message.as_str(),
                            &mut attempt_trace,
                            &mut last_attempt_url,
                            &mut last_attempt_error,
                        ) {
                            continue;
                        }
                        let request = request.take().ok_or_else(|| {
                            "request already consumed before stream preflight error response"
                                .to_string()
                        })?;
                        return respond_terminal_attempt(
                            request,
                            context,
                            &account.id,
                            attempt_trace.last_attempt_url.as_deref(),
                            429,
                            message,
                            trace_id,
                            started_at,
                            attempt_model_for_log,
                            Some(attempted_account_ids.as_slice()),
                        );
                    }
                    StreamPreflightOutcome::StatusFailover {
                        status_code,
                        message,
                    } => {
                        super::super::super::mark_account_cooldown_for_status(
                            &account.id,
                            status_code,
                        );
                        super::super::super::record_route_quality(&account.id, status_code);
                        attempt_trace.last_attempt_error = Some(message);
                        record_failover_attempt(
                            &mut attempt_trace,
                            &mut last_attempt_url,
                            &mut last_attempt_error,
                        );
                        continue;
                    }
                    StreamPreflightOutcome::RetryUsageNotice(message) => {
                        // Some Codex-compatible upstreams encode quota exhaustion as assistant
                        // output followed by an incomplete terminal sequence. Retry it before
                        // delivery, but do not persistently disable the account based on model
                        // output alone.
                        super::super::super::mark_account_cooldown(
                            &account.id,
                            super::super::super::CooldownReason::Default,
                        );
                        let _ =
                            crate::usage_refresh::enqueue_usage_refresh_for_account(&account.id);
                        attempt_trace.last_attempt_error = Some(message);
                        record_failover_attempt(
                            &mut attempt_trace,
                            &mut last_attempt_url,
                            &mut last_attempt_error,
                        );
                        continue;
                    }
                    StreamPreflightOutcome::TransportFailover(message) => {
                        super::super::super::mark_account_cooldown(
                            &account.id,
                            super::super::super::CooldownReason::Network,
                        );
                        super::super::super::record_route_quality(&account.id, 502);
                        attempt_trace.last_attempt_error = Some(message);
                        record_failover_attempt(
                            &mut attempt_trace,
                            &mut last_attempt_url,
                            &mut last_attempt_error,
                        );
                        continue;
                    }
                }
                let request = request.take().ok_or_else(|| {
                    "request already consumed before upstream response".to_string()
                })?;
                let guard = inflight_guard.take().ok_or_else(|| {
                    "inflight guard already consumed before upstream response".to_string()
                })?;
                match finalize_upstream_response(
                    request,
                    resp,
                    guard,
                    context,
                    &account.id,
                    attempt_trace.last_attempt_url.as_deref(),
                    attempt_trace.last_attempt_error.as_deref(),
                    response_adapter,
                    gemini_stream_output_mode,
                    tool_name_restore_map,
                    client_is_stream,
                    path,
                    trace_id,
                    started_at,
                    attempt_model_for_log,
                    Some(attempted_account_ids.as_slice()),
                    has_more_candidates,
                    dynamic_permit,
                    health_observer,
                    health_retry_after,
                    safeguards_evaluator,
                    context_enrichment.clone(),
                )? {
                    FinalizeUpstreamResponseOutcome::Handled { final_status } => {
                        match super::super::super::conversation_binding::record_conversation_binding_terminal_response(
                            storage, setup.conversation_routing.as_ref(), &account, attempt_model_for_log, final_status,
                        ) {
                            Ok(outcome) => log::info!("event=gateway_conversation_binding_result trace_id={} account_id={} result={}", trace_id, account.id, outcome.as_str()),
                            Err(err) => log::warn!("event=gateway_conversation_binding_update_failed trace_id={} account_id={} err={}", trace_id, account.id, err),
                        }
                        return Ok(CandidateExecutionResult::Handled);
                    }
                }
            }
        }
    }

    Ok(CandidateExecutionResult::Exhausted {
        request: Box::new(
            request
                .ok_or_else(|| "request already consumed after candidate exhaustion".to_string())?,
        ),
        attempted_account_ids,
        skipped_cooldown,
        skipped_inflight,
        last_attempt_url,
        last_attempt_error,
        temporary_unavailable,
    })
}

#[cfg(test)]
#[path = "candidate_executor_tests.rs"]
mod tests;
