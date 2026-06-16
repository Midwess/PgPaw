use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use pglite::{CommittedTransaction, PGlite, RowChange};
use serde_json::{json, Value};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::mpsc;

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

    pub fn subscribe(
        &self,
        sql: String,
        tables: Vec<String>,
        hash: String,
        version: u64,
        snapshot_body: &str,
        principal: Option<Principal>,
    ) -> mpsc::UnboundedReceiver<String> {
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
        receiver
    }

    fn reset_all(&self) {
        let mut subs = self.subs.lock().unwrap();
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

        for (id, sql, pk, last, principal) in jobs {
            let fresh = match &principal {
                Some(p) => rows::query_json_as(&self.db, &p.role, &p.claims_json, &sql).await,
                None => rows::query_json(&self.db, &sql).await,
            }
            .unwrap_or_else(|_| "[]".to_string());
            let next = keyed_map(&fresh, pk.as_deref());
            let deltas = diff(&last, &next);

            let mut subs = self.subs.lock().unwrap();
            if let Some(sub) = subs.get_mut(&id) {
                let mut alive = true;
                for delta in &deltas {
                    if sub.sender.send(encode(delta, txid)).is_err() {
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

        self.subs
            .lock()
            .unwrap()
            .retain(|_, sub| !sub.sender.is_closed());
    }
}

pub(crate) fn encode(delta: &Delta, txid: u32) -> String {
    let payload = match delta {
        Delta::Insert { key, row } => json!({"op": "insert", "key": key, "row": row, "txid": txid}),
        Delta::Update { key, row } => json!({"op": "update", "key": key, "row": row, "txid": txid}),
        Delta::Delete { key } => json!({"op": "delete", "key": key, "txid": txid}),
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
