//! Atomic manifest for independently scheduled topic images. No live restore path.
use super::*;
use crate::config::TopicStorage;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Entry {
    pub(super) slot: u8,
    pub(super) bytes: u64,
    hash: [u8; 32],
    pub storage: TopicStorage,
    pub revision: u64,
}
pub(super) type Entries = BTreeMap<String, Entry>;
pub(super) fn filename(topic: &str, slot: u8) -> String {
    let hash: String = crypto::sha256(topic.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("topic-{hash}-{slot}.bin")
}
pub(super) fn committed_files(entries: &Entries) -> std::collections::HashSet<String> {
    entries
        .iter()
        .map(|(name, entry)| filename(name, entry.slot))
        .collect()
}
fn mode(storage: &TopicStorage, common: ContentEncryption) -> ContentEncryption {
    if storage.encrypted() {
        common
    } else {
        ContentEncryption::None
    }
}
#[cfg(unix)]
pub(super) fn load(
    store: &disk::Store,
    bytes: &[u8],
    config: &Config,
    secret: Option<&ContentSecret>,
    maximum: u64,
    now: u64,
) -> std::result::Result<(Checkpoint, Entries), BoxError> {
    if bytes.starts_with(b"MYNCP001") {
        return Err(
            "Legacy content requires offline storage preview-legacy or storage migrate-legacy"
                .into(),
        );
    }
    if bytes.len() < 56
        || &bytes[..8] != b"MYNTOP01"
        || crypto::sha256(&bytes[56..]) != bytes[24..56]
    {
        return Err("Invalid topic content manifest".into());
    }
    let meta_len = usize::try_from(u64::from_le_bytes(bytes[8..16].try_into()?))?;
    let index_len = usize::try_from(u64::from_le_bytes(bytes[16..24].try_into()?))?;
    if index_len > 10_000 * 1024 {
        return Err("Topic manifest index exceeds size limit".into());
    }
    if meta_len.checked_add(index_len) != Some(bytes.len() - 56) {
        return Err("Invalid topic manifest lengths".into());
    }
    let mut checkpoint = Checkpoint::read(&bytes[56..56 + meta_len], config, maximum)?;
    if checkpoint.image.message_count() != 0 || !checkpoint.image.topics.is_empty() {
        return Err("Invalid shared owner metadata".into());
    }
    // Reject duplicate JSON keys: entries are encoded as pairs, then checked before insertion.
    struct LimitedEntries;
    impl<'de> serde::de::Visitor<'de> for LimitedEntries {
        type Value = Vec<(String, Entry)>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 10000 topic entries")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut sequence: A,
        ) -> std::result::Result<Self::Value, A::Error> {
            let mut entries = Vec::new();
            while let Some(entry) = sequence.next_element()? {
                if entries.len() == 10_000 {
                    return Err(serde::de::Error::custom("Too many topic entries"));
                }
                entries.push(entry);
            }
            Ok(entries)
        }
    }
    let mut parser = serde_json::Deserializer::from_slice(&bytes[56 + meta_len..]);
    let entries = serde::de::Deserializer::deserialize_seq(&mut parser, LimitedEntries)?;
    parser.end()?;
    if entries.len() > 10_000 {
        return Err("Too many topic checkpoints".into());
    }
    let mut index = Entries::new();
    let bindings: std::collections::HashMap<_, _> = checkpoint
        .image
        .clients
        .iter()
        .enumerate()
        .map(|(slot, name)| (name.clone(), slot as u16))
        .collect();
    let mut ids = std::collections::HashSet::new();
    let mut positions = std::collections::HashSet::new();
    let common = config
        .content_storage
        .as_ref()
        .ok_or("Missing content config")?
        .encryption;
    let mut total = bytes.len() as u64
        + if common == ContentEncryption::None {
            0
        } else {
            80
        };
    for (name, entry) in entries {
        if name.is_empty()
            || name.len() > 256
            || entry.slot > 1
            || !entry.storage.persistent()
            || entry
                .storage
                .interval()
                .is_some_and(|seconds| !(1..=2_592_000).contains(&seconds))
            || entry.storage.encrypted()
                && config
                    .content_storage
                    .as_ref()
                    .is_none_or(|s| s.encryption == ContentEncryption::None)
            || entry.revision > checkpoint.revision
            || index.contains_key(&name)
        {
            return Err("Invalid topic checkpoint entry".into());
        }
        total = total
            .checked_add(entry.bytes)
            .ok_or("Topic storage size overflow")?;
        if total > maximum {
            return Err("Topic content exceeds aggregate size limit".into());
        }
        let data = store
            .read_named(&filename(&name, entry.slot), maximum)?
            .ok_or("Missing committed topic checkpoint")?;
        if data.len() as u64 != entry.bytes || crypto::sha256(&data) != entry.hash {
            return Err("Corrupt committed topic checkpoint".into());
        }
        let plain = encryption::decode(
            data,
            mode(
                &entry.storage,
                config
                    .content_storage
                    .as_ref()
                    .ok_or("Missing content config")?
                    .encryption,
            ),
            secret,
            maximum,
        )?;
        let mut topic = Checkpoint::read(&plain, config, maximum)?;
        if topic.image.topics != [name.clone()]
            || topic.revision != entry.revision
            || topic.next_message > checkpoint.next_message
            || topic.cutoff > checkpoint.cutoff
        {
            return Err("Topic checkpoint does not match committed owner metadata".into());
        }
        topic.discard_expired(now.max(checkpoint.cutoff));
        {
            for record in topic.image.records.iter() {
                let producer = bindings
                    .get(&topic.image.clients[record.producer_slot()])
                    .ok_or("Unknown shared producer")?;
                if !ids.insert(record.id()) || !positions.insert((*producer, record.own_pos())) {
                    return Err(
                        "Conflicting active message IDs or producer positions across topics".into(),
                    );
                }
            }

            checkpoint.merge(topic, maximum, &bindings)?;
        }
        index.insert(name, entry);
    }
    Ok((checkpoint, index))
}
#[cfg(unix)]
pub(super) fn save(
    store: &disk::Store,
    checkpoint: &Checkpoint,
    selected: &[(String, TopicStorage)],
    entries: &mut Entries,
    common: ContentEncryption,
    secret: Option<&ContentSecret>,
    maximum: u64,
) -> std::result::Result<(), BoxError> {
    if store.uncertain() {
        return Err("Content manifest durability is uncertain; restart before saving again".into());
    }
    struct Staged<'a> {
        store: &'a disk::Store,
        names: Vec<String>,
        committed: bool,
    }
    impl Drop for Staged<'_> {
        fn drop(&mut self) {
            if !self.committed && !self.store.uncertain() {
                for name in &self.names {
                    self.store.remove_named(name);
                }
            }
        }
    }
    let mut staged = Staged {
        store,
        names: Vec::new(),
        committed: false,
    };
    let topic_slots: HashMap<_, _> = checkpoint
        .image
        .topics
        .iter()
        .enumerate()
        .map(|(slot, name)| (name.as_str(), slot))
        .collect();
    let mut next = entries.clone();
    // Configured RAM topics are absent from the captured persistent registry; remove old entries.
    next.retain(|name, _| topic_slots.contains_key(name.as_str()));
    // Budget the next committed generation, not a mixture of old and new
    // selected files. Old slots remain on disk until the atomic manifest commit.
    let replacing: std::collections::HashSet<_> =
        selected.iter().map(|(name, _)| name.as_str()).collect();
    let mut total = next
        .iter()
        .filter(|(name, _)| !replacing.contains(name.as_str()))
        .try_fold(0u64, |total, (_, entry)| {
            total
                .checked_add(entry.bytes)
                .ok_or("Topic storage size overflow")
        })?;
    // One pass over the captured records/pairs, rather than one whole scan per topic.
    let mut record_slots = vec![Vec::new(); checkpoint.image.topics.len()];
    for (slot, record) in checkpoint.image.records.iter_slots() {
        record_slots[record.topic() as usize].push(slot);
    }

    for (name, storage) in selected {
        let topic_slot = *topic_slots
            .get(name.as_str())
            .ok_or("Unknown captured topic")?;
        let borrowed = checkpoint.image.topics.len() == 1 && {
            let mut used = vec![false; checkpoint.image.clients.len()];
            for record in checkpoint.image.records.iter() {
                used[record.producer_slot()] = true;
            }
            used.iter().all(|used| *used)
        };
        let owned = if borrowed {
            None
        } else {
            Some(checkpoint.select_topic(
                u16::try_from(topic_slot)?,
                &record_slots[topic_slot],
                maximum,
            )?)
        };
        let topic = owned.as_ref().unwrap_or(checkpoint);
        let bytes = encryption::encode(topic.write(maximum)?, mode(storage, common), secret)?;
        total = total
            .checked_add(bytes.len() as u64)
            .ok_or("Topic storage size overflow")?;
        if total > maximum {
            return Err("Topic content exceeds aggregate size limit".into());
        }
        let slot = entries.get(name).map_or(0, |entry| 1 - entry.slot);
        let file = filename(name, slot);
        staged.names.push(file.clone());
        store.stage_topic(&file, &bytes)?;
        next.insert(
            name.clone(),
            Entry {
                slot,
                bytes: bytes.len() as u64,
                hash: crypto::sha256(&bytes),
                storage: storage.clone(),
                revision: checkpoint.revision,
            },
        );
    }
    let metadata = checkpoint.select(&[], maximum)?.write(maximum)?;
    let index = serde_json::to_vec(&next.iter().collect::<Vec<_>>())?;
    let mut manifest = Zeroizing::new(vec![0; 56]);
    manifest[..8].copy_from_slice(b"MYNTOP01");
    manifest[8..16].copy_from_slice(&(metadata.len() as u64).to_le_bytes());
    manifest[16..24].copy_from_slice(&(index.len() as u64).to_le_bytes());
    manifest.extend_from_slice(&metadata);
    manifest.extend_from_slice(&index);
    let hash = crypto::sha256(&manifest[56..]);
    manifest[24..56].copy_from_slice(&hash);
    let bytes = encryption::encode(manifest, common, secret)?;
    if total
        .checked_add(bytes.len() as u64)
        .is_none_or(|size| size > maximum)
    {
        return Err("Topic content exceeds aggregate size limit".into());
    }
    if !staged.names.is_empty() {
        store.sync_staged_topics()?;
    }
    store.save(&bytes)?;
    staged.committed = true;
    for (name, old) in entries.iter() {
        if next.get(name).is_none_or(|entry| entry.slot != old.slot) {
            store.remove_named(&filename(name, old.slot));
        }
    }
    for name in entries.keys().filter(|name| !next.contains_key(*name)) {
        store.remove_named(&filename(name, 0));
        store.remove_named(&filename(name, 1));
    }
    *entries = next;
    Ok(())
}

#[cfg(all(test, unix))]
mod functional {
    use super::*;
    use crate::{
        config::{ContentStorage, Policy},
        content::checkpoint::TopicState,
        ids::Id,
        message_image::MessageImage,
        protocol::payload::Payload,
        storage::{Arena, Blobs, ContentTypes, Record, RecordFields, StoredPayload},
        test_support::{self, Rng},
    };
    #[test]
    fn functional_seeded_manifest_mutations_validate_bindings_and_never_change_files() {
        struct Directory(std::path::PathBuf);
        impl Drop for Directory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let directory = Directory(std::env::temp_dir().join(format!(
            "mynofy-manifest-fuzz-{:016x}",
            u64::from_le_bytes(crypto::random().unwrap())
        )));
        let store = disk::Store::open(&directory.0).unwrap();
        let mut config: Config =
            serde_json::from_str(include_str!("../../../../config/scenarios/local.json")).unwrap();
        config.content_storage = Some(ContentStorage {
            store_id: None,
            directory: directory.0.clone(),
            encryption: ContentEncryption::None,
            max_snapshot_bytes: 256 * 1024,
            autosave_retry: Default::default(),
        });
        config.topics[0].storage = TopicStorage::Manual { encrypted: false };
        let now = 1_800_000_000_000;
        let mut records = Arena::new();
        let mut types = ContentTypes::default();
        let mut blobs = Blobs::default();
        let payload = StoredPayload::new(
            Payload::bytes(b"manifest-test-data".to_vec(), None),
            &mut types,
            &mut blobs,
        )
        .unwrap();
        records
            .insert(Record::new(
                RecordFields {
                    id: Id::from_index(0),
                    producer: 0,
                    topic: 0,
                    created: now,
                    until: now + 1000,
                    seq: 1,
                    own_pos: 1,
                },
                payload,
            ))
            .ok()
            .unwrap();
        let image = MessageImage::capture(
            &config.service.id,
            &[config.clients[0].client_id.clone()],
            &[config.topics[0].name.clone()],
            &records,
            &types,
            &blobs,
            Default::default(),
        )
        .unwrap();
        let checkpoint = Checkpoint {
            image,
            revision: 3,
            next_message: 1,
            reserved_ids: 2048,
            cutoff: now,
            produced: vec![1],
            topics: vec![TopicState {
                head: 1,
                policy: Policy {
                    default_ttl_seconds: 1,
                    max_ttl_seconds: 2,
                },
            }],
        };
        let slots: Vec<_> = checkpoint
            .image
            .records
            .iter_slots()
            .map(|(slot, _)| slot)
            .collect();
        let optimized = checkpoint.select_topic(0, &slots, 256 * 1024).unwrap();
        let reference = checkpoint
            .select_indexed(&[0], 256 * 1024, Some(&slots))
            .unwrap();
        assert_eq!(
            optimized.write(256 * 1024).unwrap().as_slice(),
            reference.write(256 * 1024).unwrap().as_slice(),
        );
        let empty = checkpoint.select_topic(0, &[], 256 * 1024).unwrap();
        assert!(empty.image.clients.is_empty());
        assert!(empty.produced.is_empty());

        let mut entries = Entries::new();
        save(
            &store,
            &checkpoint,
            &[(
                config.topics[0].name.clone(),
                config.topics[0].storage.clone(),
            )],
            &mut entries,
            ContentEncryption::None,
            None,
            256 * 1024,
        )
        .unwrap();
        let original = store.read(256 * 1024).unwrap().unwrap();
        let files: Vec<_> = entries
            .iter()
            .map(|(name, entry)| {
                let name = filename(name, entry.slot);
                let bytes = store.read_named(&name, 256 * 1024).unwrap().unwrap();
                (name, bytes)
            })
            .collect();
        let cases = test_support::fuzz_cases();
        let mut rng = Rng(test_support::seed());
        let mut accepted = 0;
        for case in 0..cases {
            let mut bytes = if case % 16 == 0 {
                original.to_vec()
            } else {
                rng.mutate(&original, 256 * 1024)
            };
            if case % 2 == 0 && bytes.len() >= 56 {
                let hash = crypto::sha256(&bytes[56..]);
                bytes[24..56].copy_from_slice(&hash);
            }
            let result =
                std::panic::catch_unwind(|| load(&store, &bytes, &config, None, 256 * 1024, now));
            if let Ok((loaded, index)) = result.unwrap_or_else(|_| {
                panic!(
                    "Manifest panic: seed=0x{:016x} case={case}",
                    test_support::seed()
                )
            }) {
                accepted += 1;
                assert_eq!(loaded.image.owner(), config.service.id);
                assert_eq!(loaded.image.message_count(), index.len());
                assert!(index.len() <= 1);
            }
        }
        assert_eq!(
            store.read(256 * 1024).unwrap().unwrap().as_slice(),
            original.as_slice()
        );
        for (name, expected) in files {
            assert_eq!(
                store
                    .read_named(&name, 256 * 1024)
                    .unwrap()
                    .unwrap()
                    .as_slice(),
                expected.as_slice()
            );
        }
        assert!(accepted > 0);
        eprintln!(
            "Manifest mutations: {cases} cases, {accepted} accepted; committed files unchanged"
        );
    }
}
