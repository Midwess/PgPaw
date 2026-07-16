use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

use pglite::CommittedTransaction;
#[cfg(feature = "server")]
use pglite::Replica;
use tokio::sync::broadcast;

use crate::error::CacheError;
use crate::version::VersionIndex;

#[derive(Clone)]
#[allow(dead_code)]
pub struct CdcBridge {
    stop: Arc<AtomicBool>,
    tx: broadcast::Sender<Arc<CommittedTransaction>>,
    handle: Arc<JoinHandle<()>>,
    input: Option<std::sync::mpsc::Sender<Arc<CommittedTransaction>>>,
}

impl CdcBridge {
    #[cfg(feature = "server")]
    pub fn start(replica: &Replica, versions: VersionIndex) -> Result<CdcBridge, CacheError> {
        let rx = replica.subscribe();
        let (tx, _) = broadcast::channel(1024);
        let stop = Arc::new(AtomicBool::new(false));

        let thread_tx = tx.clone();
        let thread_stop = stop.clone();
        let handle = std::thread::Builder::new()
            .name("pgpaw-cdc".into())
            .spawn(move || {
                log::info!("event=cdc_thread_start thread=pgpaw-cdc");
                while let Ok(txn) = rx.recv() {
                    if thread_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    versions.advance(txn.as_ref());
                    log::info!(
                        "event=cdc_transaction txid={} lsn={} changes={}",
                        txn.xid,
                        txn.end_lsn.0,
                        txn.changes.len(),
                    );
                    let _ = thread_tx.send(txn);
                }
                log::warn!("event=cdc_thread_stop thread=pgpaw-cdc");
            })
            .map_err(CacheError::Io)?;

        Ok(CdcBridge {
            stop,
            tx,
            handle: Arc::new(handle),
            input: None,
        })
    }

    pub(crate) fn primary(versions: VersionIndex) -> Result<CdcBridge, CacheError> {
        let (input, rx) = std::sync::mpsc::channel::<Arc<CommittedTransaction>>();
        let (tx, _) = broadcast::channel(1024);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_tx = tx.clone();
        let thread_stop = stop.clone();
        let handle = std::thread::Builder::new()
            .name("pgpaw-cdc".into())
            .spawn(move || {
                while let Ok(txn) = rx.recv() {
                    if thread_stop.load(Ordering::SeqCst) {
                        break;
                    }
                    versions.advance(txn.as_ref());
                    let _ = thread_tx.send(txn);
                }
            })
            .map_err(CacheError::Io)?;
        Ok(CdcBridge {
            stop,
            tx,
            handle: Arc::new(handle),
            input: Some(input),
        })
    }

    pub(crate) fn publish(&self, transaction: CommittedTransaction) {
        if let Some(input) = &self.input {
            let _ = input.send(Arc::new(transaction));
        }
    }

    #[allow(dead_code)]
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<CommittedTransaction>> {
        self.tx.subscribe()
    }

    pub fn stop(&self) {
        log::info!("event=cdc_stop_requested");
        self.stop.store(true, Ordering::SeqCst);
    }

    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        !self.handle.is_finished()
    }
}
