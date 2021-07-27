//! BABE testsuite

// FIXME #2532: need to allow deprecated until refactor is done
// https://github.com/paritytech/substrate/issues/2532
#![allow(deprecated)]

use std::{
    borrow::Cow, cell::RefCell, collections::HashMap, sync::Arc, task::Poll, time::Duration,
};

use codec::Encode;
use futures::{
    channel::{
        mpsc::{channel, Receiver, Sender},
        oneshot,
    },
    executor::block_on,
    prelude::*,
};
use log::debug;
use parking_lot::Mutex;

use sc_block_builder::{BlockBuilder, BlockBuilderProvider};
use sc_client_api::{backend::TransactionFor, BlockchainEvents};
use sc_consensus_babe::{
    authorship::claim_slot, block_import, find_pre_digest, start_babe, BabeEpochConfiguration,
    BabeGenesisConfiguration, BabeIntermediate, BabeLink, BabeParams, BabeVerifier,
    CompatibleDigestItem, Config, Epoch, NextEpochDescriptor, PreDigest, SecondaryPlainPreDigest,
    INTERMEDIATE_KEY,
};
use sc_consensus_epochs::{
    descendent_query, Epoch as EpochT, EpochChangesFor, SharedEpochChanges, ViableEpochDescriptor,
};
use sc_consensus_slots::{BackoffAuthoringOnFinalizedHeadLagging, SlotProportion};
use sc_keystore::LocalKeystore;
use sc_network::config::ProtocolConfig;
use sc_network_test::{Block as TestBlock, *};

use sp_application_crypto::key_types::BABE;
use sp_consensus::{
    import_queue::{BoxBlockImport, BoxJustificationImport, CacheKeyId, Verifier},
    AlwaysCanAuthor, BlockCheckParams, BlockImport, BlockImportParams, BlockOrigin,
    DisableProofRecording, Environment, ForkChoiceStrategy, ImportResult, NoNetwork as DummyOracle,
    Proposal, Proposer,
};
use sp_consensus_babe::{
    inherents::InherentDataProvider, AllowedSlots, AuthorityPair, ConsensusLog, Slot,
    BABE_ENGINE_ID,
};
use sp_core::crypto::Pair;
use sp_inherents::{CreateInherentDataProviders, InherentData};
use sp_keystore::SyncCryptoStore;
use sp_runtime::{
    generic::{BlockId, DigestItem},
    traits::{Block as BlockT, DigestFor, Header as HeaderT},
    Justifications,
};
use sp_timestamp::InherentDataProvider as TimestampInherentDataProvider;

use substrate_test_runtime_client::SyncCryptoStorePtr;

type Item = DigestItem<Hash>;

type Error = sp_blockchain::Error;

type TestClient = substrate_test_runtime_client::client::Client<
    substrate_test_runtime_client::Backend,
    substrate_test_runtime_client::Executor,
    TestBlock,
    substrate_test_runtime_client::runtime::RuntimeApi,
>;

#[derive(Copy, Clone, PartialEq)]
enum Stage {
    PreSeal,
    PostSeal,
}

type Mutator = Arc<dyn Fn(&mut TestHeader, Stage) + Send + Sync>;

type BabeBlockImport = PanickingBlockImport<
    sc_consensus_babe::BabeBlockImport<TestBlock, TestClient, Arc<TestClient>>,
>;

#[derive(Clone)]
struct DummyFactory {
    client: Arc<TestClient>,
    epoch_changes: SharedEpochChanges<TestBlock, Epoch>,
    config: Config,
    mutator: Mutator,
}

struct DummyProposer {
    factory: DummyFactory,
    parent_hash: Hash,
    parent_number: u64,
    parent_slot: Slot,
}

impl Environment<TestBlock> for DummyFactory {
    type CreateProposer = future::Ready<Result<DummyProposer, Error>>;
    type Proposer = DummyProposer;
    type Error = Error;

    fn init(&mut self, parent_header: &<TestBlock as BlockT>::Header) -> Self::CreateProposer {
        let parent_slot = find_pre_digest::<TestBlock>(parent_header)
            .expect("parent header has a pre-digest")
            .slot();

        future::ready(Ok(DummyProposer {
            factory: self.clone(),
            parent_hash: parent_header.hash(),
            parent_number: *parent_header.number(),
            parent_slot,
        }))
    }
}

impl DummyProposer {
    fn propose_with(
        &mut self,
        pre_digests: DigestFor<TestBlock>,
    ) -> future::Ready<
        Result<
            Proposal<
                TestBlock,
                sc_client_api::TransactionFor<substrate_test_runtime_client::Backend, TestBlock>,
                (),
            >,
            Error,
        >,
    > {
        let block_builder = self
            .factory
            .client
            .new_block_at(&BlockId::Hash(self.parent_hash), pre_digests, false)
            .unwrap();

        let mut block = match block_builder.build().map_err(|e| e.into()) {
            Ok(b) => b.block,
            Err(e) => return future::ready(Err(e)),
        };

        let this_slot = find_pre_digest::<TestBlock>(block.header())
            .expect("baked block has valid pre-digest")
            .slot();

        // figure out if we should add a consensus digest, since the test runtime
        // doesn't.
        let epoch_changes = self.factory.epoch_changes.shared_data();
        let epoch = epoch_changes
            .epoch_data_for_child_of(
                descendent_query(&*self.factory.client),
                &self.parent_hash,
                self.parent_number,
                this_slot,
                |slot| Epoch::genesis(&self.factory.config, slot),
            )
            .expect("client has data to find epoch")
            .expect("can compute epoch for baked block");

        let first_in_epoch = self.parent_slot < epoch.start_slot;
        if first_in_epoch {
            // push a `Consensus` digest signalling next change.
            // we just reuse the same randomness and authorities as the prior
            // epoch. this will break when we add light client support, since
            // that will re-check the randomness logic off-chain.
            let digest_data = ConsensusLog::NextEpochData(NextEpochDescriptor {
                authorities: epoch.authorities.clone(),
                randomness: epoch.randomness.clone(),
            })
            .encode();
            let digest = DigestItem::Consensus(BABE_ENGINE_ID, digest_data);
            block.header.digest_mut().push(digest)
        }

        // mutate the block header according to the mutator.
        (self.factory.mutator)(&mut block.header, Stage::PreSeal);

        future::ready(Ok(Proposal {
            block,
            proof: (),
            storage_changes: Default::default(),
        }))
    }
}

impl Proposer<TestBlock> for DummyProposer {
    type Error = Error;
    type Transaction =
        sc_client_api::TransactionFor<substrate_test_runtime_client::Backend, TestBlock>;
    type Proposal = future::Ready<Result<Proposal<TestBlock, Self::Transaction, ()>, Error>>;
    type ProofRecording = DisableProofRecording;
    type Proof = ();

    fn propose(
        mut self,
        _: InherentData,
        pre_digests: DigestFor<TestBlock>,
        _: Duration,
        _: Option<usize>,
    ) -> Self::Proposal {
        self.propose_with(pre_digests)
    }
}

thread_local! {
    static MUTATOR: RefCell<Mutator> = RefCell::new(Arc::new(|_, _|()));
}

#[derive(Clone)]
pub struct PanickingBlockImport<B>(B);

#[async_trait::async_trait]
impl<B: BlockImport<TestBlock>> BlockImport<TestBlock> for PanickingBlockImport<B>
where
    B::Transaction: Send,
    B: Send,
{
    type Error = B::Error;
    type Transaction = B::Transaction;

    async fn import_block(
        &mut self,
        block: BlockImportParams<TestBlock, Self::Transaction>,
        new_cache: HashMap<CacheKeyId, Vec<u8>>,
    ) -> Result<ImportResult, Self::Error> {
        Ok(self
            .0
            .import_block(block, new_cache)
            .await
            .expect("importing block failed"))
    }

    async fn check_block(
        &mut self,
        block: BlockCheckParams<TestBlock>,
    ) -> Result<ImportResult, Self::Error> {
        Ok(self
            .0
            .check_block(block)
            .await
            .expect("checking block failed"))
    }
}

type BabePeer = Peer<Option<PeerData>, BabeBlockImport>;

pub struct BabeTestNet {
    peers: Vec<BabePeer>,
}

type TestHeader = <TestBlock as BlockT>::Header;
type TestExtrinsic = <TestBlock as BlockT>::Extrinsic;

type TestSelectChain =
    substrate_test_runtime_client::LongestChain<substrate_test_runtime_client::Backend, TestBlock>;

pub struct TestVerifier {
    inner: BabeVerifier<
        TestBlock,
        PeersFullClient,
        TestSelectChain,
        AlwaysCanAuthor,
        Box<
            dyn CreateInherentDataProviders<
                TestBlock,
                (),
                InherentDataProviders = (TimestampInherentDataProvider, InherentDataProvider),
            >,
        >,
    >,
    mutator: Mutator,
}

#[async_trait::async_trait]
impl Verifier<TestBlock> for TestVerifier {
    /// Verify the given data and return the BlockImportParams and an optional
    /// new set of validators to import. If not, err with an Error-Message
    /// presented to the User in the logs.
    async fn verify(
        &mut self,
        origin: BlockOrigin,
        mut header: TestHeader,
        justifications: Option<Justifications>,
        body: Option<Vec<TestExtrinsic>>,
    ) -> Result<
        (
            BlockImportParams<TestBlock, ()>,
            Option<Vec<(CacheKeyId, Vec<u8>)>>,
        ),
        String,
    > {
        // apply post-sealing mutations (i.e. stripping seal, if desired).
        (self.mutator)(&mut header, Stage::PostSeal);
        self.inner
            .verify(origin, header, justifications, body)
            .await
    }
}

pub struct PeerData {
    link: BabeLink<TestBlock>,
    block_import: Mutex<
        Option<
            BoxBlockImport<
                TestBlock,
                TransactionFor<substrate_test_runtime_client::Backend, TestBlock>,
            >,
        >,
    >,
}

impl TestNetFactory for BabeTestNet {
    type Verifier = TestVerifier;
    type PeerData = Option<PeerData>;
    type BlockImport = BabeBlockImport;

    /// Create new test network with peers and given config.
    fn from_config(_config: &ProtocolConfig) -> Self {
        debug!(target: "babe", "Creating test network from config");
        BabeTestNet { peers: Vec::new() }
    }

    fn make_block_import(
        &self,
        client: PeersClient,
    ) -> (
        BlockImportAdapter<Self::BlockImport>,
        Option<BoxJustificationImport<Block>>,
        Option<PeerData>,
    ) {
        let client = client.as_full().expect("only full clients are tested");

        let config = Config::get_or_compute(&*client).expect("config available");
        let (block_import, link) = block_import(config, client.clone(), client.clone())
            .expect("can initialize block-import");

        let block_import = PanickingBlockImport(block_import);

        let data_block_import =
            Mutex::new(Some(Box::new(block_import.clone()) as BoxBlockImport<_, _>));
        (
            BlockImportAdapter::new(block_import),
            None,
            Some(PeerData {
                link,
                block_import: data_block_import,
            }),
        )
    }

    fn make_verifier(
        &self,
        client: PeersClient,
        _cfg: &ProtocolConfig,
        maybe_link: &Option<PeerData>,
    ) -> Self::Verifier {
        use substrate_test_runtime_client::DefaultTestClientBuilderExt;

        let client = client
            .as_full()
            .expect("only full clients are used in test");
        log::trace!(target: "babe", "Creating a verifier");

        // ensure block import and verifier are linked correctly.
        let data = maybe_link
            .as_ref()
            .expect("babe link always provided to verifier instantiation");

        let (_, longest_chain) = TestClientBuilder::new().build_with_longest_chain();

        TestVerifier {
            inner: BabeVerifier {
                client: client.clone(),
                select_chain: longest_chain,
                create_inherent_data_providers: Box::new(|_, _| async {
                    let timestamp = TimestampInherentDataProvider::from_system_time();
                    let slot = InherentDataProvider::from_timestamp_and_duration(
                        *timestamp,
                        Duration::from_secs(6),
                    );

                    Ok((timestamp, slot))
                }),
                config: data.link.config().clone(),
                epoch_changes: data.link.epoch_changes().clone(),
                can_author_with: AlwaysCanAuthor,
                telemetry: None,
            },
            mutator: MUTATOR.with(|m| m.borrow().clone()),
        }
    }

    fn peer(&mut self, i: usize) -> &mut BabePeer {
        log::trace!(target: "babe", "Retrieving a peer");
        &mut self.peers[i]
    }

    fn peers(&self) -> &Vec<BabePeer> {
        log::trace!(target: "babe", "Retrieving peers");
        &self.peers
    }

    fn mut_peers<F: FnOnce(&mut Vec<BabePeer>)>(&mut self, closure: F) {
        closure(&mut self.peers);
    }
}

#[test]
#[should_panic]
fn rejects_empty_block() {
    sp_tracing::try_init_simple();
    let mut net = BabeTestNet::new(3);
    let block_builder = |builder: BlockBuilder<_, _, _>| builder.build().unwrap().block;
    net.mut_peers(|peer| {
        peer[0].generate_blocks(1, BlockOrigin::NetworkInitialSync, block_builder);
    })
}

fn run_one_test(mutator: impl Fn(&mut TestHeader, Stage) + Send + Sync + 'static) {
    sp_tracing::try_init_simple();
    let mutator = Arc::new(mutator) as Mutator;

    MUTATOR.with(|m| *m.borrow_mut() = mutator.clone());
    let net = BabeTestNet::new(3);

    let peers = &[(0, "//Alice"), (1, "//Bob"), (2, "//Charlie")];

    let net = Arc::new(Mutex::new(net));
    let mut import_notifications = Vec::new();
    let mut babe_futures = Vec::new();
    let mut keystore_paths = Vec::new();

    for (peer_id, seed) in peers {
        let mut net = net.lock();
        let peer = net.peer(*peer_id);
        let client = peer
            .client()
            .as_full()
            .expect("Only full clients are used in tests")
            .clone();
        let select_chain = peer.select_chain().expect("Full client has select_chain");

        let keystore_path = tempfile::tempdir().expect("Creates keystore path");
        let keystore: SyncCryptoStorePtr =
            Arc::new(LocalKeystore::open(keystore_path.path(), None).expect("Creates keystore"));
        SyncCryptoStore::sr25519_generate_new(&*keystore, BABE, Some(seed))
            .expect("Generates authority key");
        keystore_paths.push(keystore_path);

        let mut got_own = false;
        let mut got_other = false;

        let data = peer
            .data
            .as_ref()
            .expect("babe link set up during initialization");

        let environ = DummyFactory {
            client: client.clone(),
            config: data.link.config().clone(),
            epoch_changes: data.link.epoch_changes().clone(),
            mutator: mutator.clone(),
        };

        import_notifications.push(
            // run each future until we get one of our own blocks with number higher than 5
            // that was produced locally.
            client
                .import_notification_stream()
                .take_while(move |n| {
                    future::ready(
                        n.header.number() < &5 || {
                            if n.origin == BlockOrigin::Own {
                                got_own = true;
                            } else {
                                got_other = true;
                            }

                            // continue until we have at least one block of our own
                            // and one of another peer.
                            !(got_own && got_other)
                        },
                    )
                })
                .for_each(|_| future::ready(())),
        );

        babe_futures.push(
            start_babe(BabeParams {
                block_import: data
                    .block_import
                    .lock()
                    .take()
                    .expect("import set up during init"),
                select_chain,
                client,
                env: environ,
                sync_oracle: DummyOracle,
                create_inherent_data_providers: Box::new(|_, _| async {
                    let timestamp = TimestampInherentDataProvider::from_system_time();
                    let slot = InherentDataProvider::from_timestamp_and_duration(
                        *timestamp,
                        Duration::from_secs(6),
                    );

                    Ok((timestamp, slot))
                }),
                force_authoring: false,
                backoff_authoring_blocks: Some(BackoffAuthoringOnFinalizedHeadLagging::default()),
                babe_link: data.link.clone(),
                keystore,
                can_author_with: sp_consensus::AlwaysCanAuthor,
                justification_sync_link: (),
                block_proposal_slot_portion: SlotProportion::new(0.5),
                max_block_proposal_slot_portion: None,
                telemetry: None,
            })
            .expect("Starts babe"),
        );
    }
    block_on(future::select(
        futures::future::poll_fn(move |cx| {
            let mut net = net.lock();
            net.poll(cx);
            for p in net.peers() {
                for (h, e) in p.failed_verifications() {
                    panic!("Verification failed for {:?}: {}", h, e);
                }
            }

            Poll::<()>::Pending
        }),
        future::select(
            future::join_all(import_notifications),
            future::join_all(babe_futures),
        ),
    ));
}

#[test]
fn authoring_blocks() {
    run_one_test(|_, _| ())
}

#[test]
#[should_panic]
fn rejects_missing_inherent_digest() {
    run_one_test(|header: &mut TestHeader, stage| {
        let v = std::mem::take(&mut header.digest_mut().logs);
        header.digest_mut().logs = v
            .into_iter()
            .filter(|v| stage == Stage::PostSeal || v.as_babe_pre_digest().is_none())
            .collect()
    })
}

#[test]
#[should_panic]
fn rejects_missing_seals() {
    run_one_test(|header: &mut TestHeader, stage| {
        let v = std::mem::take(&mut header.digest_mut().logs);
        header.digest_mut().logs = v
            .into_iter()
            .filter(|v| stage == Stage::PreSeal || v.as_babe_seal().is_none())
            .collect()
    })
}

#[test]
#[should_panic]
fn rejects_missing_consensus_digests() {
    run_one_test(|header: &mut TestHeader, stage| {
        let v = std::mem::take(&mut header.digest_mut().logs);
        header.digest_mut().logs = v
            .into_iter()
            .filter(|v| stage == Stage::PostSeal || v.as_next_epoch_descriptor().is_none())
            .collect()
    });
}

#[test]
fn wrong_consensus_engine_id_rejected() {
    sp_tracing::try_init_simple();
    let sig = AuthorityPair::generate().0.sign(b"");
    let bad_seal: Item = DigestItem::Seal([0; 4], sig.to_vec());
    assert!(bad_seal.as_babe_pre_digest().is_none());
    assert!(bad_seal.as_babe_seal().is_none())
}

#[test]
fn malformed_pre_digest_rejected() {
    sp_tracing::try_init_simple();
    let bad_seal: Item = DigestItem::Seal(BABE_ENGINE_ID, [0; 64].to_vec());
    assert!(bad_seal.as_babe_pre_digest().is_none());
}

#[test]
fn sig_is_not_pre_digest() {
    sp_tracing::try_init_simple();
    let sig = AuthorityPair::generate().0.sign(b"");
    let bad_seal: Item = DigestItem::Seal(BABE_ENGINE_ID, sig.to_vec());
    assert!(bad_seal.as_babe_pre_digest().is_none());
    assert!(bad_seal.as_babe_seal().is_some())
}

#[test]
fn can_author_block() {
    sp_tracing::try_init_simple();
    let keystore_path = tempfile::tempdir().expect("Creates keystore path");
    let keystore: SyncCryptoStorePtr =
        Arc::new(LocalKeystore::open(keystore_path.path(), None).expect("Creates keystore"));
    let public = SyncCryptoStore::sr25519_generate_new(&*keystore, BABE, Some("//Alice"))
        .expect("Generates authority pair");

    let mut i = 0;
    let epoch = Epoch {
        start_slot: 0.into(),
        authorities: vec![(public.into(), 1)],
        randomness: [0; 32],
        epoch_index: 1,
        duration: 100,
        config: BabeEpochConfiguration {
            c: (3, 10),
            allowed_slots: AllowedSlots::PrimaryAndSecondaryPlainSlots,
        },
    };

    let mut config = BabeGenesisConfiguration {
        slot_duration: 1000,
        epoch_length: 100,
        c: (3, 10),
        genesis_authorities: Vec::new(),
        randomness: [0; 32],
        allowed_slots: AllowedSlots::PrimaryAndSecondaryPlainSlots,
    };

    // with secondary slots enabled it should never be empty
    match claim_slot(i.into(), &epoch, &keystore) {
        None => i += 1,
        Some(s) => debug!(target: "babe", "Authored block {:?}", s.0),
    }

    // otherwise with only vrf-based primary slots we might need to try a couple
    // of times.
    config.allowed_slots = AllowedSlots::PrimarySlots;
    loop {
        match claim_slot(i.into(), &epoch, &keystore) {
            None => i += 1,
            Some(s) => {
                debug!(target: "babe", "Authored block {:?}", s.0);
                break;
            }
        }
    }
}

// Propose and import a new BABE block on top of the given parent.
fn propose_and_import_block<Transaction: Send + 'static>(
    parent: &TestHeader,
    slot: Option<Slot>,
    proposer_factory: &mut DummyFactory,
    block_import: &mut BoxBlockImport<TestBlock, Transaction>,
) -> sp_core::H256 {
    let mut proposer = futures::executor::block_on(proposer_factory.init(parent)).unwrap();

    let slot = slot.unwrap_or_else(|| {
        let parent_pre_digest = find_pre_digest::<TestBlock>(parent).unwrap();
        parent_pre_digest.slot() + 1
    });

    let pre_digest = sp_runtime::generic::Digest {
        logs: vec![Item::babe_pre_digest(PreDigest::SecondaryPlain(
            SecondaryPlainPreDigest {
                authority_index: 0,
                slot,
            },
        ))],
    };

    let parent_hash = parent.hash();

    let mut block = futures::executor::block_on(proposer.propose_with(pre_digest))
        .unwrap()
        .block;

    let epoch_descriptor = proposer_factory
        .epoch_changes
        .shared_data()
        .epoch_descriptor_for_child_of(
            descendent_query(&*proposer_factory.client),
            &parent_hash,
            *parent.number(),
            slot,
        )
        .unwrap()
        .unwrap();

    let seal = {
        // sign the pre-sealed hash of the block and then
        // add it to a digest item.
        let pair = AuthorityPair::from_seed(&[1; 32]);
        let pre_hash = block.header.hash();
        let signature = pair.sign(pre_hash.as_ref());
        Item::babe_seal(signature)
    };

    let post_hash = {
        block.header.digest_mut().push(seal.clone());
        let h = block.header.hash();
        block.header.digest_mut().pop();
        h
    };

    let mut import = BlockImportParams::new(BlockOrigin::Own, block.header);
    import.post_digests.push(seal);
    import.body = Some(block.extrinsics);
    import.intermediates.insert(
        Cow::from(INTERMEDIATE_KEY),
        Box::new(BabeIntermediate::<TestBlock> { epoch_descriptor }) as Box<_>,
    );
    import.fork_choice = Some(ForkChoiceStrategy::LongestChain);
    let import_result = block_on(block_import.import_block(import, Default::default())).unwrap();

    match import_result {
        ImportResult::Imported(_) => {}
        _ => panic!("expected block to be imported"),
    }

    post_hash
}
