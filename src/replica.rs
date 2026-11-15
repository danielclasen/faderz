//! The mixer state on one replica: samplez-replica's log and snapshots in
//! the data directory, a set of one voter, and what the set's leader does
//! besides ordering commands.

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Write as _};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use openraft::error::{InstallSnapshotError, RPCError, RaftError, Unreachable};
use openraft::network::{RPCOption, RaftNetwork, RaftNetworkFactory};
use openraft::raft::{
    AppendEntriesRequest, AppendEntriesResponse, InstallSnapshotRequest, InstallSnapshotResponse,
    VoteRequest, VoteResponse,
};
use openraft::{BasicNode, ServerState};
use samplez_replica::dedup::Limits;
use samplez_replica::entry::{Body, NodeId, SessionKey, stamping_index};
use samplez_replica::peer::{PeerClient, PeerReply, PeerRequest};
use samplez_replica::{Estimate, Replica, Settings, TypeConfig, raft_config};
use serde::{Deserialize, Serialize};

use crate::shape::Shape;
use crate::state::Mixer;

/// Samples per second: model `Shape.sampleRate` allows 48000 alone.
pub const RATE: u64 = 48_000;
/// The block size the stamping index rounds to: samplez's default B.
pub const BLOCK: u64 = 48;
/// A session's lease: the contract's 2 s after its last renewal.
pub const LEASE: u64 = 2 * RATE;
/// How long a write waits to be ordered and applied before it is
/// answered not applied, within the client's 3 s failover bound.
const WRITE_TIMEOUT: Duration = Duration::from_secs(2);
/// The address the single voter registers under; nothing dials it.
const ADDR: &str = "local";

/// The ids drawn when the data directory was empty: the state's epoch
/// and this replica's node id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Identity {
    pub epoch: u64,
    pub node: NodeId,
}

const IDENTITY: &str = "faderz.json";
const STORE: &str = "replica";

/// The identity kept in `dir`, or a new one when `dir` holds no state:
/// a new epoch is drawn exactly when the state starts empty, and kept
/// across restarts.
///
/// # Errors
///
/// When `dir` holds a replicated state but no identity, which would
/// otherwise continue an old history under a new epoch, or when reading,
/// writing or the random source fails.
pub fn identity(dir: &Path) -> io::Result<Identity> {
    let path = dir.join(IDENTITY);
    match fs::read(&path) {
        Ok(bytes) => return serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
        Err(_) => {}
    }
    let store = dir.join(STORE);
    if fs::read_dir(&store).is_ok_and(|mut d| d.next().is_some()) {
        return Err(io::Error::other(format!(
            "{} holds a replicated state but no {IDENTITY}",
            store.display()
        )));
    }
    let identity = Identity {
        epoch: random()?,
        node: random()?,
    };
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!("{IDENTITY}.tmp"));
    let mut file = fs::File::create(&tmp)?;
    file.write_all(&serde_json::to_vec(&identity).map_err(io::Error::other)?)?;
    file.sync_all()?;
    fs::rename(&tmp, &path)?;
    fs::File::open(dir)?.sync_all()?;
    Ok(identity)
}

fn random() -> io::Result<u64> {
    let mut bytes = [0u8; 8];
    // SAFETY: `bytes` is writable for its length.
    let n = unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if n != bytes.len() as isize {
        return Err(io::Error::last_os_error());
    }
    Ok(u64::from_le_bytes(bytes))
}

/// CLOCK_TAI in samples: the estimate of the processed index the
/// replicated log stamps entries with, until an engine provides one.
pub fn tai_samples() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `ts` is writable; CLOCK_TAI exists on Linux.
    unsafe { libc::clock_gettime(libc::CLOCK_TAI, &mut ts) };
    ts.tv_sec as u64 * RATE + ts.tv_nsec as u64 * RATE / 1_000_000_000
}

/// Opens the state in `dir`, a set of one voter, and waits until it
/// leads, which a lone voter does once its election timeout passed.
///
/// # Errors
///
/// When the identity or the store cannot be read, or the voter does not
/// lead within 10 s.
pub async fn start(dir: &Path, shape: Shape, estimate: Estimate) -> Result<Replica<Mixer>, String> {
    let identity = identity(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let settings = Settings {
        id: identity.node,
        epoch: identity.epoch,
        limits: Limits::defaults(RATE),
        block: BLOCK,
        write_timeout: WRITE_TIMEOUT,
        addr: ADDR.into(),
    };
    let replica = Replica::start(
        settings,
        raft_config("faderz"),
        &dir.join(STORE),
        Mixer::new(shape),
        Lone,
        Arc::new(Lone),
        estimate,
    )
    .await?;
    let initialized = replica
        .raft()
        .is_initialized()
        .await
        .map_err(|e| e.to_string())?;
    if !initialized {
        replica
            .initialize([(identity.node, ADDR.to_owned())])
            .await?;
    }
    replica
        .raft()
        .wait(Some(Duration::from_secs(10)))
        .state(ServerState::Leader, "the single voter leads")
        .await
        .map_err(|e| e.to_string())?;
    let dropped = replica
        .store()
        .with_state(|sm| sm.product.dropped().to_vec());
    if !dropped.is_empty() {
        tracing::warn!(
            ?dropped,
            "the configured shape has no place for these saved leaves"
        );
    }
    Ok(replica)
}

/// Ends the lease of every session not renewed for [`LEASE`], as the
/// leader decides it; the expiry is a no-op when a renewal was applied
/// after the one it was decided on. Never returns.
///
/// Stop-gap: samplez-replica does not decide lease expiries yet. Remove
/// this, and its task in [`crate::server::Server::start`], once it does.
pub async fn expire_leases(replica: &Replica<Mixer>, estimate: Estimate) {
    let mut decided: BTreeSet<(SessionKey, u64)> = BTreeSet::new();
    let mut ticks = tokio::time::interval(Duration::from_millis(250));
    loop {
        ticks.tick().await;
        if replica.raft().metrics().borrow().state != ServerState::Leader {
            continue;
        }
        let now = stamping_index(estimate(), BLOCK);
        let leases = replica.store().with_state(|sm| sm.leases.clone());
        decided.retain(|(s, renewed)| leases.get(s) == Some(renewed));
        for (session, renewed) in leases {
            if renewed + LEASE <= now && decided.insert((session.clone(), renewed)) {
                replica.append(Body::LeaseExpiry { session, renewed });
            }
        }
    }
}

/// The network of a set of one: there is no peer to reach.
struct Lone;

fn unreachable<E: std::error::Error>() -> RPCError<NodeId, BasicNode, E> {
    RPCError::Unreachable(Unreachable::new(&io::Error::other(
        "a set of one has no peers",
    )))
}

impl RaftNetworkFactory<TypeConfig> for Lone {
    type Network = Lone;

    async fn new_client(&mut self, _target: NodeId, _node: &BasicNode) -> Lone {
        Lone
    }
}

impl RaftNetwork<TypeConfig> for Lone {
    async fn append_entries(
        &mut self,
        _rpc: AppendEntriesRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<AppendEntriesResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        Err(unreachable())
    }

    async fn install_snapshot(
        &mut self,
        _rpc: InstallSnapshotRequest<TypeConfig>,
        _option: RPCOption,
    ) -> Result<
        InstallSnapshotResponse<NodeId>,
        RPCError<NodeId, BasicNode, RaftError<NodeId, InstallSnapshotError>>,
    > {
        Err(unreachable())
    }

    async fn vote(
        &mut self,
        _rpc: VoteRequest<NodeId>,
        _option: RPCOption,
    ) -> Result<VoteResponse<NodeId>, RPCError<NodeId, BasicNode, RaftError<NodeId>>> {
        Err(unreachable())
    }
}

impl PeerClient for Lone {
    fn call<'a>(
        &'a self,
        _to: Option<NodeId>,
        _addr: &'a str,
        _request: PeerRequest,
        _within: Duration,
    ) -> BoxFuture<'a, Result<PeerReply, String>> {
        Box::pin(async { Err("a set of one has no peers".into()) })
    }
}
