//! Authorization refresh and live backup reservation. No database work under the
//! scheduler lock, and no stale/unfiltered fallback on refresh errors.
use codexmanager_core::storage::{Account, Storage, Token};
use crate::gateway::dynamic_pool::{self, DynamicAccountPermit};

pub(super) struct DynamicRequestPolicy {
    key_id: String,
    group: Option<String>,
    plan: Option<String>,
    pub(super) pool: String,
}

impl DynamicRequestPolicy {
    pub(super) fn load(storage: &Storage, key_id: &str) -> Result<Self, String> {
        let key = storage.find_api_key_by_id(key_id).map_err(|_| "read routing authorization failed")?
            .ok_or("routing key no longer exists")?;
        if key.status != "active" { return Err("routing key is no longer active".into()); }
        let group = storage.find_api_key_account_group_filter(key_id).map_err(|_| "read routing group failed")?;
        let pool = serde_json::to_string(&(group.as_deref(), key.account_plan_filter.as_deref()))
            .map_err(|_| "encode routing pool failed")?;
        Ok(Self { key_id: key_id.to_string(), group, plan: key.account_plan_filter, pool })
    }

    fn refresh(&self, storage: &Storage, request_model: Option<&str>, effective_model: &str) -> Result<Vec<(Account, Token)>, String> {
        let current = Self::load(storage, &self.key_id)?;
        if current.group != self.group || current.plan != self.plan {
            return Err("routing authorization changed during request".into());
        }
        // This exceptional path must see account revocations and current usage.
        crate::gateway::invalidate_candidate_cache();
        let mut candidates = super::super::support::candidates::prepare_gateway_candidates(
            storage, request_model, self.group.as_deref(), self.plan.as_deref(),
            crate::gateway::LowQuotaCandidateMode::NormalOnly,
        )?;
        if request_model != Some(effective_model) {
            let actual = super::super::support::candidates::prepare_gateway_candidates(
                storage, Some(effective_model), self.group.as_deref(), self.plan.as_deref(),
                crate::gateway::LowQuotaCandidateMode::NormalOnly,
            )?.into_iter().map(|(account, _)| account.id).collect::<std::collections::HashSet<_>>();
            candidates.retain(|(account, _)| actual.contains(&account.id));
        }
        candidates.retain(|(account, _)| !crate::gateway::is_account_in_cooldown(&account.id));
        Ok(candidates)
    }

    pub(super) fn select_backup(
        &self, storage: &Storage, request_model: Option<&str>, effective_model: &str,
        attempted: &[String], max_inflight: usize,
    ) -> Result<Option<((Account, Token), DynamicAccountPermit)>, String> {
        for _ in 0..3 {
            let candidates = self.refresh(storage, request_model, effective_model)?;
            let ids = candidates.iter().map(|(a, _)| a.id.clone()).collect::<Vec<_>>();
            let Some(permit) = dynamic_pool::select_and_reserve(
                &self.pool, effective_model, &ids, attempted, max_inflight, true,
            ) else { return Ok(None); };
            // Recheck after reservation; re-read the token too. In particular,
            // deletion/disable during backoff must never fall back to old data.
            if let Some(candidate) = self.refresh(storage, request_model, effective_model)?.into_iter()
                .find(|(account, _)| account.id == permit.account_id()) {
                return Ok(Some((candidate, permit)));
            }
            drop(permit);
        }
        Ok(None)
    }
}
