use crate::{
    constants::TX_VERSION,
    errors::{BlockProcessResult, RuleError},
    model::{
        services::reachability::MTReachabilityService,
        stores::{
            block_transactions::DbBlockTransactionsStore,
            block_window_cache::BlockWindowCacheStore,
            ghostdag::DbGhostdagStore,
            headers::DbHeadersStore,
            reachability::DbReachabilityStore,
            statuses::{DbStatusesStore, StatusesStore, StatusesStoreBatchExtensions, StatusesStoreReader},
            tips::DbTipsStore,
            DB,
        },
    },
    pipeline::deps_manager::{BlockTask, BlockTaskDependencyManager},
    processes::{
        coinbase::CoinbaseManager, mass::MassCalculator, past_median_time::PastMedianTimeManager,
        transaction_validator::TransactionValidator,
    },
};
use consensus_core::{
    block::Block,
    blockstatus::BlockStatus::{self, StatusHeaderOnly, StatusInvalid},
    subnets::SUBNETWORK_ID_COINBASE,
    tx::Transaction,
};
use crossbeam_channel::{Receiver, Sender};
use hashes::Hash;
use parking_lot::RwLock;
use rayon::ThreadPool;
use rocksdb::WriteBatch;
use std::sync::Arc;

pub struct BlockBodyProcessor {
    // Channels
    receiver: Receiver<BlockTask>,
    sender: Sender<BlockTask>,

    // Thread pool
    pub(super) thread_pool: Arc<ThreadPool>,

    // DB
    db: Arc<DB>,

    // Config
    pub(super) max_block_mass: u64,
    pub(super) genesis_hash: Hash,

    // Stores
    pub(super) statuses_store: Arc<RwLock<DbStatusesStore>>,
    pub(super) ghostdag_store: Arc<DbGhostdagStore>,
    pub(super) headers_store: Arc<DbHeadersStore>,
    pub(super) block_transactions_store: Arc<DbBlockTransactionsStore>,
    pub(super) body_tips_store: Arc<RwLock<DbTipsStore>>,

    // Managers and services
    pub(super) reachability_service: MTReachabilityService<DbReachabilityStore>,
    pub(super) coinbase_manager: CoinbaseManager,
    pub(crate) mass_calculator: MassCalculator,
    pub(super) transaction_validator: TransactionValidator,
    pub(super) past_median_time_manager: PastMedianTimeManager<DbHeadersStore, DbGhostdagStore, BlockWindowCacheStore>,

    // Dependency manager
    task_manager: BlockTaskDependencyManager,
}

impl BlockBodyProcessor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        receiver: Receiver<BlockTask>,
        sender: Sender<BlockTask>,
        thread_pool: Arc<ThreadPool>,
        db: Arc<DB>,
        statuses_store: Arc<RwLock<DbStatusesStore>>,
        ghostdag_store: Arc<DbGhostdagStore>,
        headers_store: Arc<DbHeadersStore>,
        block_transactions_store: Arc<DbBlockTransactionsStore>,
        body_tips_store: Arc<RwLock<DbTipsStore>>,
        reachability_service: MTReachabilityService<DbReachabilityStore>,
        coinbase_manager: CoinbaseManager,
        mass_calculator: MassCalculator,
        transaction_validator: TransactionValidator,
        past_median_time_manager: PastMedianTimeManager<DbHeadersStore, DbGhostdagStore, BlockWindowCacheStore>,
        max_block_mass: u64,
        genesis_hash: Hash,
    ) -> Self {
        Self {
            receiver,
            sender,
            thread_pool,
            db,
            statuses_store,
            reachability_service,
            ghostdag_store,
            headers_store,
            block_transactions_store,
            body_tips_store,
            coinbase_manager,
            mass_calculator,
            transaction_validator,
            past_median_time_manager,
            max_block_mass,
            genesis_hash,
            task_manager: BlockTaskDependencyManager::new(),
        }
    }

    pub fn worker(self: &Arc<BlockBodyProcessor>) {
        while let Ok(task) = self.receiver.recv() {
            match task {
                BlockTask::Exit => break,
                BlockTask::Process(block, result_transmitters) => {
                    let hash = block.header.hash;
                    if self.task_manager.register(block, result_transmitters) {
                        let processor = self.clone();
                        self.thread_pool.spawn(move || {
                            processor.queue_block(hash);
                        });
                    }
                }
            };
        }

        // Wait until all workers are idle before exiting
        self.task_manager.wait_for_idle();

        // Pass the exit signal on to the following processor
        self.sender.send(BlockTask::Exit).unwrap();
    }

    fn queue_block(self: &Arc<BlockBodyProcessor>, hash: Hash) {
        if let Some(block) = self.task_manager.try_begin(hash) {
            let res = self.process_block_body(&block);

            let dependent_tasks = self.task_manager.end(hash, |block, result_transmitters| {
                if res.is_err() {
                    for transmitter in result_transmitters {
                        // We don't care if receivers were dropped
                        let _ = transmitter.send(res.clone());
                    }
                } else {
                    self.sender.send(BlockTask::Process(block, result_transmitters)).unwrap();
                }
            });

            for dep in dependent_tasks {
                let processor = self.clone();
                self.thread_pool.spawn(move || processor.queue_block(dep));
            }
        }
    }

    fn process_block_body(self: &Arc<BlockBodyProcessor>, block: &Block) -> BlockProcessResult<BlockStatus> {
        let status = self.statuses_store.read().get(block.hash()).unwrap();
        match status {
            StatusInvalid => return Err(RuleError::KnownInvalid),
            StatusHeaderOnly => {} // Proceed to body processing
            _ if status.has_block_body() => return Ok(status),
            _ => panic!("unexpected block status {:?}", status),
        }

        if let Err(e) = self.validate_body(block) {
            // We mark invalid blocks with status StatusInvalid except in the
            // case of the following errors:
            // MissingParents - If we got MissingParents the block shouldn't be
            // considered as invalid because it could be added later on when its
            // parents are present.
            // BadMerkleRoot - if we get BadMerkleRoot we shouldn't mark the
            // block as invalid because later on we can get the block with
            // transactions that fits the merkle root.
            // PrunedBlock - PrunedBlock is an error that rejects a block body and
            // not the block as a whole, so we shouldn't mark it as invalid.
            // TODO: implement the last part.
            if !matches!(e, RuleError::BadMerkleRoot(_, _) | RuleError::MissingParents(_)) {
                self.statuses_store.write().set(block.hash(), BlockStatus::StatusInvalid).unwrap();
            }
            return Err(e);
        }

        self.commit_body(block.hash(), block.header.direct_parents(), block.transactions.clone());
        Ok(BlockStatus::StatusUTXOPendingVerification)
    }

    fn validate_body(self: &Arc<BlockBodyProcessor>, block: &Block) -> BlockProcessResult<()> {
        self.validate_body_in_isolation(block)?;
        self.validate_body_in_context(block)
    }

    fn commit_body(self: &Arc<BlockBodyProcessor>, hash: Hash, parents: &[Hash], transactions: Arc<Vec<Transaction>>) {
        let mut batch = WriteBatch::default();

        // This is an append only store so it requires no lock.
        self.block_transactions_store.insert_batch(&mut batch, hash, transactions).unwrap();

        let mut body_tips_write_guard = self.body_tips_store.write();
        body_tips_write_guard.add_tip_batch(&mut batch, hash, parents).unwrap();
        let statuses_write_guard =
            self.statuses_store.set_batch(&mut batch, hash, BlockStatus::StatusUTXOPendingVerification).unwrap();

        self.db.write(batch).unwrap();

        // Calling the drops explicitly after the batch is written in order to avoid possible errors.
        drop(statuses_write_guard);
        drop(body_tips_write_guard);
    }

    pub fn process_genesis_if_needed(self: &Arc<BlockBodyProcessor>) {
        let status = self.statuses_store.read().get(self.genesis_hash).unwrap();
        match status {
            StatusHeaderOnly => {
                let mut batch = WriteBatch::default();
                let mut body_tips_write_guard = self.body_tips_store.write();
                body_tips_write_guard.init_batch(&mut batch, &[]).unwrap();
                self.db.write(batch).unwrap();
                drop(body_tips_write_guard);

                let genesis_coinbase = Transaction::new(
                    TX_VERSION,
                    vec![],
                    vec![],
                    0,
                    SUBNETWORK_ID_COINBASE,
                    0,
                    vec![
                        // Kaspad devnet coinbase payload
                        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, // Blue score
                        0x00, 0xE1, 0xF5, 0x05, 0x00, 0x00, 0x00, 0x00, // Subsidy
                        0x00, 0x00, // Script version
                        0x01, // Varint
                        0x00, // OP-FALSE
                        0x6b, 0x61, 0x73, 0x70, 0x61, 0x2d, 0x64, 0x65, 0x76, 0x6e, 0x65, 0x74, // kaspa-devnet
                    ],
                );
                self.commit_body(self.genesis_hash, &[], Arc::new(vec![genesis_coinbase]))
            }
            _ if status.has_block_body() => (),
            _ => panic!("unexpected genesis status {:?}", status),
        }
    }
}
