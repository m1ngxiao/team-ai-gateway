//! One ledger per logical gateway request, shared across every transport send.
//! The TLS slot only conveys ownership on the synchronous gateway worker; async
//! transport threads capture the Arc explicitly before they are spawned.
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::gateway::dynamic_pool;

const MAX_GENERATION_SENDS: usize = 3;

thread_local! {
    static CURRENT: RefCell<Option<Arc<GenerationBudget>>> = const { RefCell::new(None) };
}

#[derive(Default)]
struct SendState {
    sent: usize,
    backup_started_at: Option<usize>,
    reserved_backup: Option<dynamic_pool::RetryReservation>,
    closed: bool,
}

pub(crate) struct GenerationBudget {
    pool: String,
    model: String,
    trace_id: String,
    deadline: Option<Instant>,
    state: Mutex<SendState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BudgetError {
    Cancelled,
    Deadline,
    RequestLimit,
    BackupLimit,
    SharedLimit,
}

const MSG_CANCELLED: &str = "generation request is no longer active";
const MSG_DEADLINE: &str = "upstream total timeout exceeded";
const MSG_REQUEST_LIMIT: &str = "generation retry budget exhausted: request send limit";
const MSG_BACKUP_LIMIT: &str = "generation retry budget exhausted: backup send limit";
const MSG_SHARED_LIMIT: &str = "generation retry budget exhausted: shared pool limit";

impl std::fmt::Display for BudgetError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Cancelled => MSG_CANCELLED,
            Self::Deadline => MSG_DEADLINE,
            Self::RequestLimit => MSG_REQUEST_LIMIT,
            Self::BackupLimit => MSG_BACKUP_LIMIT,
            Self::SharedLimit => MSG_SHARED_LIMIT,
        })
    }
}
impl std::error::Error for BudgetError {}

pub(crate) fn is_deadline_error_message(message: &str) -> bool {
    message.trim() == MSG_DEADLINE
}

pub(crate) fn is_budget_error_message(message: &str) -> bool {
    matches!(
        message.trim(),
        MSG_CANCELLED | MSG_DEADLINE | MSG_REQUEST_LIMIT | MSG_BACKUP_LIMIT | MSG_SHARED_LIMIT
    )
}

impl GenerationBudget {
    fn local_check(&self, state: &SendState) -> Result<(), BudgetError> {
        if state.closed {
            return Err(BudgetError::Cancelled);
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            return Err(BudgetError::Deadline);
        }
        if state.sent >= MAX_GENERATION_SENDS {
            return Err(BudgetError::RequestLimit);
        }
        if state
            .backup_started_at
            .is_some_and(|start| state.sent > start)
        {
            return Err(BudgetError::BackupLimit);
        }
        Ok(())
    }

    /// Advisory only. A competing logical request can consume the shared token
    /// before this request actually sends; before_send is the authority.
    pub(crate) fn can_send(&self) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.local_check(&state).is_ok()
            && (state.sent == 0
                || state.reserved_backup.is_some()
                || dynamic_pool::retry_available(&self.pool, &self.model))
    }

    /// Called at the actual POST / response.create boundary. Conservatively
    /// counts a connection or write failure once a send has been authorized.
    pub(crate) fn before_send(&self) -> Result<(), BudgetError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let result = self.local_check(&state).and_then(|()| {
            if state.sent == 0 {
                dynamic_pool::record_first_generation(&self.pool, &self.model);
            } else {
                let mut token = state
                    .reserved_backup
                    .take()
                    .or_else(|| dynamic_pool::reserve_retry(&self.pool, &self.model))
                    .ok_or(BudgetError::SharedLimit)?;
                token.commit();
            }
            state.sent += 1;
            Ok(())
        });
        match &result {
            Ok(()) => log::info!(
                "event=gateway_generation_send trace_id={} model={} physical_send={} backup={}",
                self.trace_id, self.model, state.sent, state.backup_started_at.is_some()
            ),
            Err(reason) => log::info!(
                "event=gateway_generation_send_denied trace_id={} model={} physical_sends={} reason={}",
                self.trace_id, self.model, state.sent, reason
            ),
        }
        result
    }

    pub(crate) fn sent_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sent
    }

    fn reserve_backup_send(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.sent == 0 || self.local_check(&state).is_err() {
            return false;
        }
        if state.reserved_backup.is_some() {
            return true;
        }
        let Some(token) = dynamic_pool::reserve_retry(&self.pool, &self.model) else {
            return false;
        };
        let sent = state.sent;
        state.backup_started_at.get_or_insert(sent);
        state.reserved_backup = Some(token);
        true
    }

    fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.closed = true;
        // No POST/frame was authorized for a retained reservation; return it
        // immediately even when a transport thread still holds this Arc.
        state.reserved_backup.take();
    }
}

pub(crate) struct BudgetScope {
    active: Option<Arc<GenerationBudget>>,
    previous: Option<Arc<GenerationBudget>>,
    // Dropping a TLS scope on a different thread would restore the wrong slot.
    _same_thread: PhantomData<Rc<()>>,
}

pub(crate) fn enter(
    enabled: bool,
    pool: &str,
    model: &str,
    trace_id: &str,
    deadline: Option<Instant>,
) -> BudgetScope {
    let active = enabled.then(|| {
        Arc::new(GenerationBudget {
            pool: pool.to_string(),
            model: model.to_string(),
            trace_id: trace_id.to_string(),
            deadline,
            state: Mutex::new(SendState::default()),
        })
    });
    let previous = CURRENT.with(|slot| slot.replace(active.clone()));
    BudgetScope {
        active,
        previous,
        _same_thread: PhantomData,
    }
}

pub(crate) fn current() -> Option<Arc<GenerationBudget>> {
    CURRENT.with(|slot| slot.borrow().clone())
}

impl BudgetScope {
    pub(crate) fn can_send(&self) -> bool {
        self.active
            .as_ref()
            .map_or(true, |budget| budget.can_send())
    }
    pub(crate) fn sent_count(&self) -> usize {
        self.active.as_ref().map_or(0, |budget| budget.sent_count())
    }
    pub(crate) fn reserve_backup_send(&self) -> bool {
        self.active
            .as_ref()
            .map_or(true, |budget| budget.reserve_backup_send())
    }
}

impl Drop for BudgetScope {
    fn drop(&mut self) {
        if let Some(budget) = self.active.as_ref() {
            budget.close();
        }
        CURRENT.with(|slot| {
            slot.replace(self.previous.take());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;

    #[test]
    fn concurrent_transport_sends_share_the_logical_request_limit() {
        let _scope = enter(true, "generation_budget_concurrent", "model", "trace", None);
        let budget = current().unwrap();
        let barrier = Arc::new(Barrier::new(12));
        let tasks: Vec<_> = (0..12)
            .map(|_| {
                let budget = budget.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    budget.before_send().is_ok()
                })
            })
            .collect();
        let sent = tasks
            .into_iter()
            .filter_map(|task| task.join().ok())
            .filter(|sent| *sent)
            .count();
        assert_eq!(sent, 3);
        assert_eq!(budget.sent_count(), 3);
        assert!(!budget.can_send());
    }

    #[test]
    fn backup_has_one_send_even_when_request_limit_has_room() {
        let scope = enter(true, "generation_budget_backup", "model", "trace", None);
        let budget = current().unwrap();
        budget.before_send().unwrap();
        assert!(scope.reserve_backup_send());
        budget.before_send().unwrap();
        assert_eq!(budget.before_send(), Err(BudgetError::BackupLimit));
        assert_eq!(scope.sent_count(), 2);
        assert!(!scope.can_send());
    }

    #[test]
    fn reserved_backup_token_survives_other_requests_draining_the_bucket() {
        let pool = "generation_budget_reserved_backup";
        let scope = enter(true, pool, "model", "trace", None);
        let budget = current().unwrap();
        budget.before_send().unwrap();
        assert!(scope.reserve_backup_send());
        for _ in 0..2 {
            dynamic_pool::reserve_retry(pool, "model").unwrap().commit();
        }
        assert!(!dynamic_pool::retry_available(pool, "model"));
        assert!(scope.can_send());
        budget.before_send().unwrap();
        assert_eq!(scope.sent_count(), 2);
        assert_eq!(budget.before_send(), Err(BudgetError::BackupLimit));
    }

    #[test]
    fn scope_drop_refunds_unsent_reservation_and_rejects_late_worker_send() {
        let pool = "generation_budget_cancelled_reservation";
        let scope = enter(true, pool, "model", "trace", None);
        let budget = current().unwrap();
        budget.before_send().unwrap();
        assert!(scope.reserve_backup_send());
        drop(scope);
        assert_eq!(budget.before_send(), Err(BudgetError::Cancelled));
        for _ in 0..3 {
            dynamic_pool::reserve_retry(pool, "model").unwrap().commit();
        }
        assert!(dynamic_pool::reserve_retry(pool, "model").is_none());
        assert_eq!(budget.sent_count(), 1);
    }

    #[test]
    fn exhausted_shared_budget_does_not_block_a_first_send() {
        let pool = "generation_budget_shared";
        for _ in 0..3 {
            dynamic_pool::reserve_retry(pool, "model").unwrap().commit();
        }
        let _scope = enter(true, pool, "model", "trace", None);
        let budget = current().unwrap();
        assert!(budget.can_send());
        budget.before_send().unwrap();
        assert_eq!(budget.before_send(), Err(BudgetError::SharedLimit));
        assert_eq!(budget.sent_count(), 1);
    }

    #[test]
    fn deadline_prevents_initial_send_and_disabled_nested_scope_restores_owner() {
        let _scope = enter(
            true,
            "generation_budget_deadline",
            "model",
            "trace",
            Some(Instant::now()),
        );
        let budget = current().unwrap();
        assert_eq!(budget.before_send(), Err(BudgetError::Deadline));
        assert_eq!(budget.sent_count(), 0);
        {
            let disabled = enter(false, "unused", "unused", "unused", None);
            assert!(current().is_none());
            assert!(disabled.can_send());
        }
        assert!(Arc::ptr_eq(&budget, &current().unwrap()));
    }

    #[test]
    fn budget_and_deadline_error_message_helpers_classify_all_variants() {
        for err in [
            BudgetError::Cancelled,
            BudgetError::Deadline,
            BudgetError::RequestLimit,
            BudgetError::BackupLimit,
            BudgetError::SharedLimit,
        ] {
            let msg = err.to_string();
            assert!(is_budget_error_message(&msg), "expected budget error for {msg}");
            assert_eq!(
                is_deadline_error_message(&msg),
                err == BudgetError::Deadline
            );
        }
        assert!(!is_budget_error_message("connection reset by peer"));
        assert!(!is_deadline_error_message("connection timed out"));
    }
}
