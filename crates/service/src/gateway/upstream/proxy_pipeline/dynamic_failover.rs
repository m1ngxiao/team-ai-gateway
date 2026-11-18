//! Authorization refresh and live backup reservation. No database work under the
//! scheduler lock, and no stale/unfiltered fallback on refresh errors.
use crate::gateway::dynamic_pool::{self, DynamicAccountPermit};
use codexmanager_core::storage::{Account, Storage, Token};

fn order_quota_aware_candidates(
    candidates: &mut [(Account, Token)],
    snapshot: &crate::gateway::quota_aware::QuotaAwareSnapshot,
) -> bool {
    crate::gateway::quota_aware::order_candidates(candidates, snapshot)
}

pub(super) struct DynamicRequestPolicy {
    key_id: String,
    platform_key_hash: String,
    group: Option<String>,
    plan: Option<String>,
    pub(super) pool: String,
}

impl DynamicRequestPolicy {
    pub(super) fn load(storage: &Storage, key_id: &str) -> Result<Self, String> {
        let key = storage
            .find_api_key_by_id(key_id)
            .map_err(|_| "read routing authorization failed")?
            .ok_or("routing key no longer exists")?;
        if key.status != "active" {
            return Err("routing key is no longer active".into());
        }
        let group = storage
            .find_api_key_account_group_filter(key_id)
            .map_err(|_| "read routing group failed")?;
        let pool = serde_json::to_string(&(group.as_deref(), key.account_plan_filter.as_deref()))
            .map_err(|_| "encode routing pool failed")?;
        Ok(Self {
            key_id: key_id.to_string(),
            platform_key_hash: key.key_hash,
            group,
            plan: key.account_plan_filter,
            pool,
        })
    }

    fn refresh(
        &self,
        storage: &Storage,
        request_model: Option<&str>,
        effective_model: &str,
    ) -> Result<Vec<(Account, Token)>, String> {
        let current = Self::load(storage, &self.key_id)?;
        if current.platform_key_hash != self.platform_key_hash
            || current.group != self.group
            || current.plan != self.plan
        {
            return Err("routing authorization changed during request".into());
        }
        // This exceptional path must see account revocations and current usage.
        crate::gateway::invalidate_candidate_cache();
        let mut candidates = super::super::support::candidates::prepare_gateway_candidates(
            storage,
            request_model,
            self.group.as_deref(),
            self.plan.as_deref(),
            crate::gateway::LowQuotaCandidateMode::NormalOnly,
        )?;
        if request_model != Some(effective_model) {
            let actual = super::super::support::candidates::prepare_gateway_candidates(
                storage,
                Some(effective_model),
                self.group.as_deref(),
                self.plan.as_deref(),
                crate::gateway::LowQuotaCandidateMode::NormalOnly,
            )?
            .into_iter()
            .map(|(account, _)| account.id)
            .collect::<std::collections::HashSet<_>>();
            candidates.retain(|(account, _)| actual.contains(&account.id));
        }
        candidates.retain(|(account, _)| !crate::gateway::is_account_in_cooldown(&account.id));
        Ok(candidates)
    }

    fn rank_candidates(&self, storage: &Storage, candidates: &mut [(Account, Token)]) -> bool {
        let Some(snapshot) = crate::gateway::load_quota_aware_snapshot_if_enabled(
            storage,
            self.platform_key_hash.as_str(),
            candidates,
        ) else {
            return false;
        };
        let _ = order_quota_aware_candidates(candidates, &snapshot);
        true
    }

    pub(super) fn select_backup(
        &self,
        storage: &Storage,
        request_model: Option<&str>,
        effective_model: &str,
        attempted: &[String],
        max_inflight: usize,
    ) -> Result<Option<((Account, Token), DynamicAccountPermit)>, String> {
        for _ in 0..3 {
            let mut candidates = self.refresh(storage, request_model, effective_model)?;
            let ranked = self.rank_candidates(storage, candidates.as_mut_slice());
            log::info!(
                "event=gateway_dynamic_backup_ranked ranking={} candidates={} attempted={}",
                if ranked { "quota_aware" } else { "in_flight" },
                candidates.len(),
                attempted.len()
            );
            let ids = candidates
                .iter()
                .map(|(a, _)| a.id.clone())
                .collect::<Vec<_>>();
            let selected = if ranked {
                dynamic_pool::select_and_reserve_ranked(
                    &self.pool,
                    effective_model,
                    &ids,
                    attempted,
                    max_inflight,
                    true,
                )
            } else {
                dynamic_pool::select_and_reserve(
                    &self.pool,
                    effective_model,
                    &ids,
                    attempted,
                    max_inflight,
                    true,
                )
            };
            let Some(permit) = selected else {
                return Ok(None);
            };
            // Recheck after reservation; re-read the token too. In particular,
            // deletion/disable during backoff must never fall back to old data.
            if let Some(candidate) = self
                .refresh(storage, request_model, effective_model)?
                .into_iter()
                .find(|(account, _)| account.id == permit.account_id())
            {
                return Ok(Some((candidate, permit)));
            }
            drop(permit);
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway::quota_aware::{QuotaAwareAccountMetrics, QuotaAwareSnapshot};

    fn account(id: &str, sort: i64) -> Account {
        Account {
            id: id.to_string(),
            label: id.to_string(),
            issuer: "https://auth.openai.com".to_string(),
            chatgpt_account_id: None,
            workspace_id: None,
            group_name: None,
            sort,
            status: "active".to_string(),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn token(account_id: &str) -> Token {
        Token {
            account_id: account_id.to_string(),
            id_token: String::new(),
            access_token: "access".to_string(),
            refresh_token: "refresh".to_string(),
            api_key_access_token: None,
            last_refresh: 1,
        }
    }

    #[test]
    fn backup_candidates_follow_quota_aware_score_order() {
        let mut candidates = vec![
            (account("backup-low", 0), token("backup-low")),
            (account("backup-best", 1), token("backup-best")),
            (account("backup-mid", 2), token("backup-mid")),
        ];
        let snapshot = QuotaAwareSnapshot::new([
            (
                "backup-low".to_string(),
                QuotaAwareAccountMetrics {
                    remaining_percent: Some(40.0),
                    recent_tokens: 50_000,
                    active_bindings: 3,
                },
            ),
            (
                "backup-best".to_string(),
                QuotaAwareAccountMetrics {
                    remaining_percent: Some(90.0),
                    recent_tokens: 1_000,
                    active_bindings: 1,
                },
            ),
            (
                "backup-mid".to_string(),
                QuotaAwareAccountMetrics {
                    remaining_percent: Some(70.0),
                    recent_tokens: 10_000,
                    active_bindings: 2,
                },
            ),
        ]);

        assert!(order_quota_aware_candidates(
            candidates.as_mut_slice(),
            &snapshot
        ));
        assert_eq!(
            candidates
                .iter()
                .map(|(account, _)| account.id.as_str())
                .collect::<Vec<_>>(),
            vec!["backup-best", "backup-mid", "backup-low"]
        );
    }
}
