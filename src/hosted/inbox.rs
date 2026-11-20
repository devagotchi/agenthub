//! A recipient's position on a hosted board (`agenthub::inbox`): one row of `board_settings`,
//! read with the addressed notes and written in the same transaction. Beside it, the board's
//! webhooks (`agenthub::wake`), another row, read and changed in one transaction.

use crate::chain::actions;
use devagotchi_contracts::BridgeResult;
use sqlx::AnyConnection;

use super::{DbResult, HostedBoard, decode};
use crate::{
    Actor, Note,
    inbox::{self, Cursors, Delivery, InboxNote, KEY, SCAN},
    invalid,
    wake::{self, Wake, Wakes},
};
use devagotchi_store::transaction;

impl HostedBoard {
    async fn cursors(&self, connection: &mut AnyConnection) -> DbResult<Cursors> {
        let raw: Option<String> = sqlx::query_scalar(
            "SELECT value FROM board_settings WHERE tenant_id=$1 AND board_id=$2 AND key=$3",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(KEY)
        .fetch_optional(&mut *connection)
        .await?;
        // A damaged row is refused, never read as empty and then overwritten (G4).
        Ok(raw.map(|raw| decode(&raw)).transpose()?.unwrap_or_default())
    }

    async fn save_cursors(
        &self,
        connection: &mut AnyConnection,
        cursors: &Cursors,
    ) -> DbResult<()> {
        sqlx::query(
            "INSERT INTO board_settings (tenant_id,board_id,key,value) VALUES ($1,$2,$3,$4)
             ON CONFLICT (tenant_id,board_id,key) DO UPDATE SET value=excluded.value",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(KEY)
        .bind(serde_json::to_string(cursors).expect("inbox json"))
        .execute(&mut *connection)
        .await?;
        Ok(())
    }

    async fn addressed(&self, connection: &mut AnyConnection, after: i64) -> DbResult<Vec<Note>> {
        let texts: Vec<String> = sqlx::query_scalar(
            "SELECT row_to_json(n)::text FROM board_notes n
             WHERE tenant_id=$1 AND board_id=$2 AND seq>$3 AND to_principal IS NOT NULL
             ORDER BY seq LIMIT $4",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(after)
        .bind(SCAN)
        .fetch_all(&mut *connection)
        .await?;
        Ok(texts
            .iter()
            .map(|text| decode(text))
            .collect::<BridgeResult<Vec<Note>>>()?)
    }

    pub async fn inbox_deliver(
        &self,
        who: &[String],
        me: &str,
        commit: bool,
    ) -> BridgeResult<Delivery> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let mut cursors = self.cursors(connection).await?;
            let notes = self
                .addressed(connection, inbox::floor(&cursors, who))
                .await?;
            let before = cursors.clone();
            let delivery = inbox::plan(&notes, who, me, &mut cursors, now);
            if commit && cursors != before {
                self.save_cursors(connection, &cursors).await?;
            }
            Ok(delivery)
        })
    }

    pub async fn inbox_mark_delivered(&self, notes: &[InboxNote]) -> BridgeResult<()> {
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let mut cursors = self.cursors(connection).await?;
            inbox::mark_delivered(&mut cursors, notes, now);
            self.save_cursors(connection, &cursors).await?;
            Ok(())
        })
    }

    pub async fn inbox(
        &self,
        who: &[String],
        me: &str,
        ack_through: Option<i64>,
    ) -> BridgeResult<Vec<InboxNote>> {
        transaction!(&self.store, |connection| {
            self.scope(connection).await?;
            let mut cursors = self.cursors(connection).await?;
            if let Some(through) = ack_through {
                inbox::acknowledge(&mut cursors, who, through);
                self.save_cursors(connection, &cursors).await?;
            }
            let notes = self
                .addressed(connection, inbox::floor(&cursors, who))
                .await?;
            Ok(inbox::unread(&notes, who, me, &cursors))
        })
    }

    pub async fn wakes(&self) -> BridgeResult<Wakes> {
        transaction!(&self.store, |connection| {
            self.scope(connection).await?;
            self.wakes_in(connection).await
        })
    }

    async fn wakes_in(&self, connection: &mut AnyConnection) -> DbResult<Wakes> {
        let raw: Option<String> = sqlx::query_scalar(
            "SELECT value FROM board_settings WHERE tenant_id=$1 AND board_id=$2 AND key=$3",
        )
        .bind(&self.tenant)
        .bind(&self.project)
        .bind(wake::KEY)
        .fetch_optional(&mut *connection)
        .await?;
        Ok(raw.map(|raw| decode(&raw)).transpose()?.unwrap_or_default())
    }

    /// Point `recipient` at `url`, or (no url) take its webhook away, in one transaction: the
    /// read, the `MAX_WAKES` check, the write and `wake.set`. A new secret comes back once; it
    /// is made before the transaction and never goes on the chain. Taking away a webhook the
    /// recipient does not have writes nothing.
    pub async fn set_wake(
        &self,
        actor: &Actor<'_>,
        recipient: &str,
        url: Option<&str>,
    ) -> BridgeResult<Option<String>> {
        let secret = url.map(|_| hex::encode(devagotchi_store::random::<32>()));
        transaction!(&self.store, |connection| {
            let now = self.scope(connection).await?;
            let mut wakes = self.wakes_in(connection).await?;
            match (url, &secret) {
                (Some(url), Some(secret)) => {
                    if !wakes.contains_key(recipient) && wakes.len() >= wake::MAX_WAKES {
                        return Err(invalid(format!(
                            "a board holds at most {} webhooks",
                            wake::MAX_WAKES
                        ))
                        .into());
                    }
                    wakes.insert(
                        recipient.to_string(),
                        Wake {
                            url: url.to_string(),
                            secret: secret.clone(),
                        },
                    );
                }
                _ => {
                    if wakes.remove(recipient).is_none() {
                        return Ok(None);
                    }
                }
            }
            sqlx::query(
                "INSERT INTO board_settings (tenant_id,board_id,key,value) VALUES ($1,$2,$3,$4)
                 ON CONFLICT (tenant_id,board_id,key) DO UPDATE SET value=excluded.value",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(wake::KEY)
            .bind(serde_json::to_string(&wakes).expect("wakes json"))
            .execute(&mut *connection)
            .await?;
            self.event(
                connection,
                actor.id,
                now,
                actions::WAKE_SET,
                None,
                None,
                serde_json::json!({ "recipient": recipient, "url": url }),
            )
            .await?;
            Ok(secret.clone())
        })
    }
}
