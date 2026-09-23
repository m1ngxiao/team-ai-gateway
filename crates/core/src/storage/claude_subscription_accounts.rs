use rusqlite::{params, OptionalExtension, Result, Row};

use super::{ClaudeSubscriptionAccount, ClaudeSubscriptionLoginSession, Storage};

const ACCOUNT_SELECT: &str = "SELECT id, label, email, account_uuid, organization_uuid,
    subscription_type, status, sort, access_token, refresh_token, scopes,
    expires_at, last_error, created_at, updated_at
    FROM claude_subscription_accounts";

fn read_account(row: &Row<'_>) -> Result<ClaudeSubscriptionAccount> {
    Ok(ClaudeSubscriptionAccount {
        id: row.get(0)?,
        label: row.get(1)?,
        email: row.get(2)?,
        account_uuid: row.get(3)?,
        organization_uuid: row.get(4)?,
        subscription_type: row.get(5)?,
        status: row.get(6)?,
        sort: row.get(7)?,
        access_token: row.get(8)?,
        refresh_token: row.get(9)?,
        scopes: row.get(10)?,
        expires_at: row.get(11)?,
        last_error: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn read_login_session(row: &Row<'_>) -> Result<ClaudeSubscriptionLoginSession> {
    Ok(ClaudeSubscriptionLoginSession {
        id: row.get(0)?,
        state: row.get(1)?,
        code_verifier: row.get(2)?,
        status: row.get(3)?,
        error: row.get(4)?,
        account_id: row.get(5)?,
        created_at: row.get(6)?,
        updated_at: row.get(7)?,
    })
}

impl Storage {
    pub fn insert_claude_subscription_login_session(
        &self,
        session: &ClaudeSubscriptionLoginSession,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO claude_subscription_login_sessions
             (id, state, code_verifier, status, error, account_id, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                session.id,
                session.state,
                session.code_verifier,
                session.status,
                session.error,
                session.account_id,
                session.created_at,
                session.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn find_claude_subscription_login_session(
        &self,
        id: &str,
    ) -> Result<Option<ClaudeSubscriptionLoginSession>> {
        self.conn
            .query_row(
                "SELECT id, state, code_verifier, status, error, account_id, created_at, updated_at
                 FROM claude_subscription_login_sessions WHERE id = ?1",
                [id],
                read_login_session,
            )
            .optional()
    }

    pub fn claim_claude_subscription_login_session(
        &self,
        id: &str,
        state: &str,
        now: i64,
    ) -> Result<Option<ClaudeSubscriptionLoginSession>> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE claude_subscription_login_sessions
             SET status = 'expired', code_verifier = '', updated_at = ?1
             WHERE status IN ('pending', 'completing') AND created_at <= ?1 - 900",
            [now],
        )?;
        let changed = tx.execute(
            "UPDATE claude_subscription_login_sessions
             SET status = 'completing', updated_at = ?3
             WHERE id = ?1 AND state = ?2 AND status = 'pending'
               AND created_at > ?3 - 900 AND created_at <= ?3",
            params![id, state, now],
        )?;
        let session = if changed == 1 {
            tx.query_row(
                "SELECT id, state, code_verifier, status, error, account_id, created_at, updated_at
                 FROM claude_subscription_login_sessions WHERE id = ?1",
                [id],
                read_login_session,
            )
            .optional()?
        } else {
            None
        };
        tx.commit()?;
        Ok(session)
    }

    pub fn finish_claude_subscription_login_session(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
        account_id: Option<&str>,
        now: i64,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE claude_subscription_login_sessions
             SET status = ?2, error = ?3, account_id = ?4,
                 code_verifier = '', updated_at = ?5
             WHERE id = ?1 AND status = 'completing'",
            params![id, status, error, account_id, now],
        )? == 1)
    }

    pub fn upsert_claude_subscription_account(
        &self,
        account: &ClaudeSubscriptionAccount,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO claude_subscription_accounts
             (id, label, email, account_uuid, organization_uuid, subscription_type,
              status, sort, access_token, refresh_token, scopes, expires_at,
              last_error, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
             ON CONFLICT(id) DO UPDATE SET
               label = excluded.label,
               email = excluded.email,
               account_uuid = excluded.account_uuid,
               organization_uuid = excluded.organization_uuid,
               subscription_type = excluded.subscription_type,
               status = CASE
                 WHEN claude_subscription_accounts.status = 'needs_login' THEN 'disabled'
                 ELSE claude_subscription_accounts.status
               END,
               access_token = excluded.access_token,
               refresh_token = excluded.refresh_token,
               scopes = excluded.scopes,
               expires_at = excluded.expires_at,
               last_error = NULL,
               updated_at = excluded.updated_at",
            params![
                account.id,
                account.label,
                account.email,
                account.account_uuid,
                account.organization_uuid,
                account.subscription_type,
                account.status,
                account.sort,
                account.access_token,
                account.refresh_token,
                account.scopes,
                account.expires_at,
                account.last_error,
                account.created_at,
                account.updated_at,
            ],
        )?;
        Ok(())
    }

    pub fn find_claude_subscription_account(
        &self,
        id: &str,
    ) -> Result<Option<ClaudeSubscriptionAccount>> {
        self.conn
            .query_row(
                &format!("{ACCOUNT_SELECT} WHERE id = ?1"),
                [id],
                read_account,
            )
            .optional()
    }

    pub fn list_claude_subscription_accounts(&self) -> Result<Vec<ClaudeSubscriptionAccount>> {
        let mut statement = self.conn.prepare(&format!(
            "{ACCOUNT_SELECT} ORDER BY sort ASC, created_at ASC, id ASC"
        ))?;
        statement.query_map([], read_account)?.collect()
    }

    pub fn list_active_claude_subscription_accounts(
        &self,
    ) -> Result<Vec<ClaudeSubscriptionAccount>> {
        let mut statement = self.conn.prepare(&format!(
            "{ACCOUNT_SELECT} WHERE status = 'active' ORDER BY sort ASC, created_at ASC, id ASC"
        ))?;
        statement.query_map([], read_account)?.collect()
    }

    pub fn update_claude_subscription_account_status(
        &self,
        id: &str,
        status: &str,
        now: i64,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE claude_subscription_accounts SET status = ?2, updated_at = ?3
             WHERE id = ?1 AND ?2 IN ('active', 'disabled', 'needs_login')
               AND NOT (status = 'needs_login' AND ?2 = 'active')",
            params![id, status, now],
        )? == 1)
    }

    pub fn delete_claude_subscription_account(&self, id: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM claude_subscription_accounts WHERE id = ?1",
            [id],
        )? == 1)
    }

    pub fn rotate_claude_subscription_token(
        &self,
        id: &str,
        previous_access_token: &str,
        previous_refresh_token: &str,
        access_token: &str,
        refresh_token: &str,
        scopes: &str,
        expires_at: i64,
        now: i64,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE claude_subscription_accounts
             SET access_token = ?4, refresh_token = ?5, scopes = ?6,
                 expires_at = ?7,
                 status = CASE WHEN status = 'needs_login' THEN 'active' ELSE status END,
                 last_error = NULL, updated_at = ?8
             WHERE id = ?1 AND access_token = ?2 AND refresh_token = ?3",
            params![
                id,
                previous_access_token,
                previous_refresh_token,
                access_token,
                refresh_token,
                scopes,
                expires_at,
                now,
            ],
        )? == 1)
    }

    pub fn mark_claude_subscription_account_needs_login(
        &self,
        id: &str,
        previous_access_token: &str,
        previous_refresh_token: &str,
        error: &str,
        now: i64,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE claude_subscription_accounts
             SET status = 'needs_login', last_error = ?4, updated_at = ?5
             WHERE id = ?1 AND access_token = ?2 AND refresh_token = ?3
               AND status = 'active'",
            params![id, previous_access_token, previous_refresh_token, error, now],
        )? == 1)
    }

    pub fn mark_claude_subscription_account_needs_login_if_access_token(
        &self,
        id: &str,
        expected_access_token: &str,
        error: &str,
        now: i64,
    ) -> Result<bool> {
        Ok(self.conn.execute(
            "UPDATE claude_subscription_accounts
             SET status = 'needs_login', last_error = ?3, updated_at = ?4
             WHERE id = ?1 AND access_token = ?2 AND status = 'active'",
            params![id, expected_access_token, error, now],
        )? == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage() -> Storage {
        let storage = Storage::open_in_memory().expect("open in-memory database");
        storage.init().expect("initialize schema");
        storage
    }

    fn login_session(id: &str, created_at: i64) -> ClaudeSubscriptionLoginSession {
        ClaudeSubscriptionLoginSession {
            id: id.to_string(),
            state: format!("{id}-state"),
            code_verifier: format!("{id}-verifier"),
            status: "pending".to_string(),
            error: None,
            account_id: None,
            created_at,
            updated_at: created_at,
        }
    }

    fn account(id: &str) -> ClaudeSubscriptionAccount {
        ClaudeSubscriptionAccount {
            id: id.to_string(),
            label: id.to_string(),
            email: Some(format!("{id}@example.test")),
            account_uuid: Some(id.to_string()),
            organization_uuid: Some(format!("{id}-org")),
            subscription_type: Some("pro".to_string()),
            status: "active".to_string(),
            sort: 0,
            access_token: format!("{id}-access-1"),
            refresh_token: format!("{id}-refresh-1"),
            scopes: "user:inference".to_string(),
            expires_at: 2000,
            last_error: None,
            created_at: 1000,
            updated_at: 1000,
        }
    }

    #[test]
    fn login_session_claim_is_one_time_and_clears_verifier_on_completion() {
        let storage = storage();
        let session = login_session("login", 1000);
        storage
            .insert_claude_subscription_login_session(&session)
            .expect("insert session");

        assert!(storage
            .claim_claude_subscription_login_session("login", "wrong-state", 1001)
            .expect("wrong state")
            .is_none());
        let claimed = storage
            .claim_claude_subscription_login_session("login", "login-state", 1001)
            .expect("claim session")
            .expect("pending session should be claimed");
        assert_eq!(claimed.status, "completing");
        assert_eq!(claimed.code_verifier, "login-verifier");
        assert!(storage
            .claim_claude_subscription_login_session("login", "login-state", 1002)
            .expect("replay attempt")
            .is_none());

        assert!(storage
            .finish_claude_subscription_login_session("login", "completed", None, Some("account"), 1002)
            .expect("finish session"));
        assert!(!storage
            .finish_claude_subscription_login_session("login", "failed", Some("late error"), None, 1003)
            .expect("late completion"));
        let finished = storage
            .find_claude_subscription_login_session("login")
            .expect("find session")
            .expect("session exists");
        assert_eq!(finished.status, "completed");
        assert_eq!(finished.account_id.as_deref(), Some("account"));
        assert!(finished.code_verifier.is_empty());
    }

    #[test]
    fn expired_login_session_cannot_be_claimed_and_loses_verifier() {
        let storage = storage();
        storage
            .insert_claude_subscription_login_session(&login_session("expired", 1000))
            .expect("insert session");
        storage
            .insert_claude_subscription_login_session(&login_session("fresh", 1001))
            .expect("insert session");

        assert!(storage
            .claim_claude_subscription_login_session("expired", "expired-state", 1900)
            .expect("expired claim")
            .is_none());
        let expired = storage
            .find_claude_subscription_login_session("expired")
            .expect("find expired session")
            .expect("session exists");
        assert_eq!(expired.status, "expired");
        assert!(expired.code_verifier.is_empty());
        assert!(storage
            .claim_claude_subscription_login_session("fresh", "fresh-state", 1900)
            .expect("fresh claim")
            .is_some());
    }

    #[test]
    fn token_rotation_and_login_failure_are_guarded_by_current_credentials() {
        let storage = storage();
        storage
            .upsert_claude_subscription_account(&account("claude-a"))
            .expect("insert first account");
        storage
            .upsert_claude_subscription_account(&account("claude-b"))
            .expect("insert second account");

        assert!(storage
            .rotate_claude_subscription_token(
                "claude-a", "claude-a-access-1", "claude-a-refresh-1", "claude-a-access-2",
                "claude-a-refresh-2", "user:inference", 3000, 2000,
            )
            .expect("rotate first account"));
        assert!(!storage
            .rotate_claude_subscription_token(
                "claude-a", "claude-a-access-1", "claude-a-refresh-1", "stale-access",
                "stale-refresh", "user:inference", 3001, 2001,
            )
            .expect("stale rotation"));
        assert!(!storage
            .mark_claude_subscription_account_needs_login(
                "claude-a", "claude-a-access-1", "claude-a-refresh-1", "stale refresh failed", 2002,
            )
            .expect("stale refresh failure"));

        let first = storage
            .find_claude_subscription_account("claude-a")
            .expect("find first account")
            .expect("first account exists");
        assert_eq!(first.status, "active");
        assert_eq!(first.access_token, "claude-a-access-2");
        assert_eq!(first.refresh_token, "claude-a-refresh-2");
        assert!(storage
            .mark_claude_subscription_account_needs_login(
                "claude-a", "claude-a-access-2", "claude-a-refresh-2", "refresh rejected", 2003,
            )
            .expect("current refresh failure"));
        assert!(storage
            .rotate_claude_subscription_token(
                "claude-b", "claude-b-access-1", "claude-b-refresh-1", "claude-b-access-2",
                "claude-b-refresh-1", "user:inference", 3000, 2000,
            )
            .expect("rotate while retaining refresh token"));
        assert!(!storage
            .mark_claude_subscription_account_needs_login(
                "claude-b", "claude-b-access-1", "claude-b-refresh-1", "stale refresh failed", 2001,
            )
            .expect("stale failure with retained refresh token"));
        let active = storage
            .list_active_claude_subscription_accounts()
            .expect("list active accounts");
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].id, "claude-b");
        assert_eq!(active[0].access_token, "claude-b-access-2");
        assert_eq!(active[0].refresh_token, "claude-b-refresh-1");
    }

    #[test]
    fn stale_upstream_unauthorized_does_not_disable_refreshed_or_admin_disabled_account() {
        let storage = storage();
        storage
            .upsert_claude_subscription_account(&account("claude-a"))
            .expect("insert account");
        assert!(storage
            .rotate_claude_subscription_token(
                "claude-a", "claude-a-access-1", "claude-a-refresh-1",
                "claude-a-access-2", "claude-a-refresh-2", "user:inference", 3000, 2000,
            )
            .expect("rotate account"));
        assert!(!storage
            .mark_claude_subscription_account_needs_login_if_access_token(
                "claude-a", "claude-a-access-1", "stale 401", 2001,
            )
            .expect("stale 401"));
        assert_eq!(storage.list_active_claude_subscription_accounts().unwrap().len(), 1);

        assert!(storage
            .update_claude_subscription_account_status("claude-a", "disabled", 2002)
            .expect("admin disables account"));
        assert!(!storage
            .mark_claude_subscription_account_needs_login_if_access_token(
                "claude-a", "claude-a-access-2", "late 401", 2003,
            )
            .expect("late 401"));
        assert_eq!(
            storage
                .find_claude_subscription_account("claude-a")
                .unwrap()
                .unwrap()
                .status,
            "disabled"
        );
    }

    #[test]
    fn successful_relogin_restores_needs_login_account_to_disabled_before_activation() {
        let storage = storage();
        let mut expired = account("claude-a");
        expired.status = "needs_login".to_string();
        expired.last_error = Some("login expired".to_string());
        storage
            .upsert_claude_subscription_account(&expired)
            .expect("insert expired account");

        assert!(!storage
            .update_claude_subscription_account_status("claude-a", "active", 2000)
            .expect("activation before new login"));
        let mut relogged = account("claude-a");
        relogged.status = "disabled".to_string();
        relogged.access_token = "next-access".to_string();
        relogged.refresh_token = "next-refresh".to_string();
        relogged.created_at = 2001;
        relogged.updated_at = 2001;
        storage
            .upsert_claude_subscription_account(&relogged)
            .expect("save successful new login");

        let restored = storage
            .find_claude_subscription_account("claude-a")
            .expect("find account")
            .expect("account exists");
        assert_eq!(restored.status, "disabled");
        assert_eq!(restored.access_token, "next-access");
        assert_eq!(restored.refresh_token, "next-refresh");
        assert_eq!(restored.last_error, None);
        assert_eq!(restored.created_at, 1000);
        assert!(storage
            .update_claude_subscription_account_status("claude-a", "active", 2002)
            .expect("activate after new login"));
        assert_eq!(storage.list_active_claude_subscription_accounts().unwrap().len(), 1);
    }

    #[test]
    fn relogin_of_active_account_preserves_its_active_status() {
        let storage = storage();
        storage
            .upsert_claude_subscription_account(&account("claude-a"))
            .expect("insert active account");
        let mut relogged = account("claude-a");
        relogged.status = "disabled".to_string();
        relogged.access_token = "next-access".to_string();
        storage
            .upsert_claude_subscription_account(&relogged)
            .expect("save successful new login");
        let updated = storage
            .find_claude_subscription_account("claude-a")
            .unwrap()
            .unwrap();
        assert_eq!(updated.status, "active");
        assert_eq!(updated.access_token, "next-access");
    }

    #[test]
    fn successful_refresh_recovers_racing_unauthorized_but_preserves_admin_disable() {
        let storage = storage();
        storage
            .upsert_claude_subscription_account(&account("claude-a"))
            .expect("insert account");
        assert!(storage
            .mark_claude_subscription_account_needs_login_if_access_token(
                "claude-a", "claude-a-access-1", "old request returned 401", 2000,
            )
            .expect("mark unauthorized"));
        assert!(storage
            .rotate_claude_subscription_token(
                "claude-a", "claude-a-access-1", "claude-a-refresh-1",
                "claude-a-access-2", "claude-a-refresh-2", "user:inference", 3000, 2001,
            )
            .expect("save successful refresh"));
        let recovered = storage
            .find_claude_subscription_account("claude-a")
            .unwrap()
            .unwrap();
        assert_eq!(recovered.status, "active");
        assert_eq!(recovered.access_token, "claude-a-access-2");
        assert_eq!(recovered.last_error, None);

        assert!(storage
            .update_claude_subscription_account_status("claude-a", "disabled", 2002)
            .expect("admin disables account"));
        assert!(storage
            .rotate_claude_subscription_token(
                "claude-a", "claude-a-access-2", "claude-a-refresh-2",
                "claude-a-access-3", "claude-a-refresh-3", "user:inference", 4000, 2003,
            )
            .expect("late refresh after admin disable"));
        let disabled = storage
            .find_claude_subscription_account("claude-a")
            .unwrap()
            .unwrap();
        assert_eq!(disabled.status, "disabled");
        assert_eq!(disabled.access_token, "claude-a-access-3");
    }
}
