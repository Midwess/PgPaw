use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pglite::{CommittedTransaction, PGlite, RowChange};
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;
use tokio_stream::Stream;

use crate::auth::Principal;
use crate::cdc::CdcBridge;
use crate::diff::{diff, keyed_map, Delta};
use crate::rows;

struct Subscription {
    tables: Vec<String>,
    pk: Option<String>,
    sql: String,
    sender: mpsc::UnboundedSender<String>,
    last: HashMap<String, Value>,
    principal: Option<Principal>,
}

type LiveJob = (
    u64,
    String,
    Option<String>,
    HashMap<String, Value>,
    Option<Principal>,
);

#[derive(Clone)]
pub struct LiveHub {
    subs: Arc<Mutex<HashMap<u64, Subscription>>>,
    next_id: Arc<AtomicU64>,
    db: PGlite,
    pk: Arc<HashMap<String, String>>,
}

pub struct LiveSubscription {
    id: u64,
    subs: Arc<Mutex<HashMap<u64, Subscription>>>,
    receiver: mpsc::UnboundedReceiver<String>,
}

impl Stream for LiveSubscription {
    type Item = String;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.receiver.poll_recv(cx)
    }
}

impl Drop for LiveSubscription {
    fn drop(&mut self) {
        self.subs.lock().unwrap().remove(&self.id);
    }
}

impl LiveHub {
    pub fn start(bridge: &CdcBridge, db: PGlite, pk: Arc<HashMap<String, String>>) -> LiveHub {
        let hub = LiveHub {
            subs: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(0)),
            db,
            pk,
        };
        let mut receiver = bridge.subscribe();
        let worker = hub.clone();
        tokio::spawn(async move {
            loop {
                match receiver.recv().await {
                    Ok(txn) => worker.on_commit(&txn).await,
                    Err(RecvError::Lagged(_)) => worker.reset_all(),
                    Err(RecvError::Closed) => break,
                }
            }
        });
        hub
    }

    pub fn subscription_count(&self) -> usize {
        self.subs.lock().unwrap().len()
    }

    pub fn subscribe(
        &self,
        sql: String,
        tables: Vec<String>,
        hash: String,
        version: u64,
        snapshot_body: &str,
        principal: Option<Principal>,
    ) -> LiveSubscription {
        let (sender, receiver) = mpsc::unbounded_channel();
        let pk = if tables.len() == 1 {
            self.pk.get(&tables[0].to_ascii_lowercase()).cloned()
        } else {
            None
        };

        let last = keyed_map(snapshot_body, pk.as_deref());
        let first = if principal.is_some() {
            let rows: Value = serde_json::from_str(snapshot_body).unwrap_or_else(|_| json!([]));
            json!({"type": "snapshot", "rows": rows, "version": version})
        } else {
            json!({"type": "snapshot", "url": format!("/q/{hash}/{version}"), "version": version})
        };
        let _ = sender.send(format!("data: {first}\n\n"));

        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        log::info!(
            "event=live_subscription_registered id={} scope={} tables={} pk={:?} version={} snapshot_bytes={}",
            id,
            if principal.is_some() { "private" } else { "public" },
            tables.join(","),
            pk,
            version,
            snapshot_body.len(),
        );
        self.subs.lock().unwrap().insert(
            id,
            Subscription {
                tables,
                pk,
                sql,
                sender,
                last,
                principal,
            },
        );
        LiveSubscription {
            id,
            subs: self.subs.clone(),
            receiver,
        }
    }

    fn reset_all(&self) {
        let mut subs = self.subs.lock().unwrap();
        log::warn!(
            "event=live_reset subscriptions={} reason=broadcast_lag",
            subs.len(),
        );
        for sub in subs.values() {
            let _ = sub.sender.send(reset());
        }
        subs.clear();
    }

    async fn on_commit(&self, txn: &CommittedTransaction) {
        let txid = txn.xid;
        let changed: HashSet<String> = txn
            .changes
            .iter()
            .map(|change| change_table(change).to_ascii_lowercase())
            .collect();
        let changed_tables = changed.iter().cloned().collect::<Vec<_>>().join(",");

        let jobs: Vec<LiveJob> = {
            let subs = self.subs.lock().unwrap();
            subs.iter()
                .filter(|(_, sub)| {
                    sub.tables
                        .iter()
                        .any(|table| changed.contains(&table.to_ascii_lowercase()))
                })
                .map(|(id, sub)| {
                    (
                        *id,
                        sub.sql.clone(),
                        sub.pk.clone(),
                        sub.last.clone(),
                        sub.principal.clone(),
                    )
                })
                .collect()
        };
        log::info!(
            "event=live_commit txid={} lsn={} changed_tables={} subscriptions_matched={}",
            txid,
            txn.end_lsn.0,
            changed_tables,
            jobs.len(),
        );

        for (id, sql, pk, last, principal) in jobs {
            let fresh = match &principal {
                Some(p) => rows::query_json_as(&self.db, &p.role, &p.claims_json, &sql).await,
                None => rows::query_json(&self.db, &sql).await,
            }
            .unwrap_or_else(|_| "[]".to_string());
            let next = keyed_map(&fresh, pk.as_deref());
            let deltas = diff(&last, &next);
            log::info!(
                "event=live_diff subscription_id={} txid={} previous_rows={} next_rows={} deltas={}",
                id,
                txid,
                last.len(),
                next.len(),
                deltas.len(),
            );

            let mut subs = self.subs.lock().unwrap();
            if let Some(sub) = subs.get_mut(&id) {
                let mut alive = true;
                for delta in &deltas {
                    if sub.sender.send(encode(delta, txid)).is_err() {
                        log::warn!(
                            "event=live_subscriber_send_failed subscription_id={} txid={}",
                            id,
                            txid,
                        );
                        alive = false;
                        break;
                    }
                }
                if alive {
                    let _ = sub.sender.send(up_to_date(txid));
                    sub.last = next;
                }
            }
        }

        let mut subs = self.subs.lock().unwrap();
        let before = subs.len();
        subs.retain(|_, sub| !sub.sender.is_closed());
        let removed = before.saturating_sub(subs.len());
        if removed > 0 {
            log::info!(
                "event=live_subscribers_cleaned removed={} active={}",
                removed,
                subs.len(),
            );
        }
    }
}

pub(crate) fn encode(delta: &Delta, txid: u32) -> String {
    let payload = match delta {
        Delta::Insert { key, row } => json!({"op": "insert", "key": key, "row": row, "txid": txid}),
        Delta::Update { key, row } => json!({"op": "update", "key": key, "row": row, "txid": txid}),
        Delta::Delete { key, row } => json!({"op": "delete", "key": key, "row": row, "txid": txid}),
    };
    format!("data: {payload}\n\n")
}

pub(crate) fn up_to_date(txid: u32) -> String {
    format!("data: {}\n\n", json!({"op": "up-to-date", "txid": txid}))
}

pub(crate) fn reset() -> String {
    "data: {\"op\":\"reset\"}\n\n".to_string()
}

fn change_table(change: &RowChange) -> &str {
    match change {
        RowChange::Insert { table, .. }
        | RowChange::Update { table, .. }
        | RowChange::Delete { table, .. }
        | RowChange::Truncate { table, .. } => table,
    }
}

#[cfg(test)]
mod tests {
    use super::{LiveSubscription, Subscription};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;

    #[test]
    fn dropping_subscription_removes_it_immediately() {
        let subs: Arc<Mutex<HashMap<u64, Subscription>>> = Arc::new(Mutex::new(HashMap::new()));
        let (sender, receiver) = mpsc::unbounded_channel();
        subs.lock().unwrap().insert(
            7,
            Subscription {
                tables: Vec::new(),
                pk: None,
                sql: String::new(),
                sender,
                last: HashMap::new(),
                principal: None,
            },
        );
        let subscription = LiveSubscription {
            id: 7,
            subs: subs.clone(),
            receiver,
        };
        drop(subscription);
        assert!(subs.lock().unwrap().is_empty());
    }
}
