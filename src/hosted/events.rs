//! Reading the existing board chain, with filters applied before pagination.
use super::*;
use crate::EventFilter;

impl HostedBoard {
    pub async fn events(&self) -> BridgeResult<Vec<Event>> {
        self.events_page(0, usize::MAX).await
    }

    pub async fn events_page(&self, after: i64, limit: usize) -> BridgeResult<Vec<Event>> {
        self.query_events(
            &EventFilter {
                after_seq: after,
                ..Default::default()
            },
            i64::try_from(limit.max(1)).unwrap_or(i64::MAX),
        )
        .await
    }

    /// A bounded page of matching events. Cursors are exclusive and limits are 1..=1000.
    pub async fn events_matching(&self, filter: &EventFilter) -> BridgeResult<Vec<Event>> {
        if filter.after_seq < 0 || filter.before_seq.is_some_and(|seq| seq < 1) {
            return Err(invalid(
                "event cursors must be nonnegative (before_seq must be positive)",
            ));
        }
        self.query_events(filter, filter.limit.unwrap_or(200).clamp(1, 1000) as i64)
            .await
    }

    async fn query_events(&self, filter: &EventFilter, limit: i64) -> BridgeResult<Vec<Event>> {
        let action = filter
            .action_prefix
            .as_deref()
            .map(|prefix| prefix.strip_suffix('*').unwrap_or(prefix));
        transaction!(&self.store, |connection| {
            self.scope(connection).await?;
            let rows: Vec<String> = sqlx::query_scalar(
                "SELECT row_to_json(e)::text FROM board_events e
                 WHERE tenant_id=$1 AND board_id=$2 AND seq>$3
                   AND ($4::bigint IS NULL OR seq<$4)
                   AND ($5::text IS NULL OR actor=$5)
                   AND ($6::text IS NULL OR left(action,char_length($6))=$6)
                   AND ($7::text IS NULL OR ticket=$7)
                 ORDER BY CASE WHEN $8::boolean THEN seq END DESC, seq ASC LIMIT $9",
            )
            .bind(&self.tenant)
            .bind(&self.project)
            .bind(filter.after_seq)
            .bind(filter.before_seq)
            .bind(&filter.actor)
            .bind(action)
            .bind(&filter.ticket)
            .bind(filter.descending)
            .bind(limit)
            .fetch_all(&mut *connection)
            .await?;
            decode_events(rows)
        })
    }
}

pub(in crate::hosted) fn decode_events(rows: Vec<String>) -> DbResult<Vec<Event>> {
    rows.iter()
        .map(|text| {
            let mut value: serde_json::Value = decode(text)?;
            let data = value["data"]
                .as_str()
                .ok_or_else(|| fail("board_damaged", "invalid event data"))?;
            value["data"] = decode(data)?;
            serde_json::from_value(value).map_err(|error| fail("board_damaged", error.to_string()))
        })
        .collect::<BridgeResult<Vec<Event>>>()
        .map_err(Failed::from)
}
