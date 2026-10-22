//! Durable handoff content and recipient receipts.
//!
//! The content record is immutable. A separate receipt records that the
//! recipient-side consumer durably took responsibility for it; domain meaning
//! and settlement remain with the producer/consumer.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::control_plane::public_contracts::{Handoff, HandoffIdentity, HandoffReceipt};
use crate::control_plane::registry::{publication::write_atomic, read_record_file};
use crate::model::SessionUuid;

pub const HANDOFF_VERSION: u32 = 1;
pub const MAX_HANDOFF_CONTENT_BYTES: usize = 384 * 1024;
pub const MAX_HANDOFF_SUMMARY_CHARS: usize = 600;
pub const MAX_CORRELATION_ID_BYTES: usize = 256;
pub const MAX_PENDING_PER_RECIPIENT: usize = 128;
const MAX_HANDOFF_RECORD_BYTES: usize = 900 * 1024;
const MAX_STORED_HANDOFFS: usize = 512;
const MAX_STORED_HANDOFF_BYTES: u64 = 64 * 1024 * 1024;
const ACKED_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(String),
    Conflict,
    NotFound,
    WrongRecipient,
    Capacity,
    Internal(String),
}

pub struct CommunicationStore {
    root: PathBuf,
    lock: Mutex<()>,
}

impl CommunicationStore {
    pub fn open(state_root: &Path) -> Result<Self, String> {
        let root = state_root.join("communications");
        fs::create_dir_all(&root)
            .map_err(|error| format!("communication store {}: {error}", root.display()))?;
        let metadata = fs::symlink_metadata(&root)
            .map_err(|error| format!("communication store {}: {error}", root.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "communication store {} is not a directory",
                root.display()
            ));
        }
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("communication store {}: {error}", root.display()))?;
        Ok(Self {
            root,
            lock: Mutex::new(()),
        })
    }

    pub fn publish(&self, handoff: &Handoff) -> Result<(Handoff, bool), Error> {
        validate_handoff(handoff)?;
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_acked(handoff.published_at_unix_ms as i64)?;
        let path = self.handoff_path(&handoff.id);
        if path.exists() {
            let existing = self.read_handoff(&handoff.id)?;
            if same_publication(&existing, handoff) {
                return Ok((existing, false));
            }
            return Err(Error::Conflict);
        }
        let records = self.read_all()?;
        let stored_bytes = self.stored_bytes(&records)?;
        if records.len() >= MAX_STORED_HANDOFFS
            || stored_bytes >= MAX_STORED_HANDOFF_BYTES
            || records
                .iter()
                .filter(|record| {
                    record.recipient_subject_id == handoff.recipient_subject_id
                        && !self.receipt_path(&record.id).exists()
                })
                .count()
                >= MAX_PENDING_PER_RECIPIENT
        {
            return Err(Error::Capacity);
        }
        let bytes = serde_json::to_vec(handoff)
            .map_err(|error| Error::Internal(format!("encode handoff: {error}")))?;
        if bytes.len() > MAX_HANDOFF_RECORD_BYTES {
            return Err(Error::Invalid("handoff record is too large".into()));
        }
        if stored_bytes.saturating_add(bytes.len() as u64) > MAX_STORED_HANDOFF_BYTES {
            return Err(Error::Capacity);
        }
        write_atomic(&self.root, &path, &bytes, MAX_HANDOFF_RECORD_BYTES)
            .map_err(Error::Internal)?;
        Ok((handoff.clone(), true))
    }

    pub fn pending(&self, recipient: &HandoffIdentity) -> Result<Vec<Handoff>, Error> {
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_acked(crate::control_plane::store::now_ms())?;
        let mut records = self.read_all()?;
        records.retain(|handoff| {
            recipient_matches(&handoff.recipient_subject_id, recipient)
                && !self.receipt_path(&handoff.id).exists()
        });
        records.sort_by(|left, right| {
            left.published_at_unix_ms
                .cmp(&right.published_at_unix_ms)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(records)
    }

    pub fn get(
        &self,
        id: &str,
        recipient: &HandoffIdentity,
    ) -> Result<(Handoff, Option<HandoffReceipt>), Error> {
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_acked(crate::control_plane::store::now_ms())?;
        let handoff = self.read_handoff(id)?;
        if !recipient_matches(&handoff.recipient_subject_id, recipient) {
            return Err(Error::WrongRecipient);
        }
        let receipt = self.read_receipt(id)?;
        Ok((handoff, receipt))
    }

    pub fn acknowledge(
        &self,
        id: &str,
        recipient: &HandoffIdentity,
        received_at_unix_ms: u64,
    ) -> Result<(HandoffReceipt, bool), Error> {
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_acked(crate::control_plane::store::now_ms())?;
        let handoff = self.read_handoff(id)?;
        if !recipient_matches(&handoff.recipient_subject_id, recipient) {
            return Err(Error::WrongRecipient);
        }
        if let Some(receipt) = self.read_receipt(id)? {
            return Ok((receipt, false));
        }
        if received_at_unix_ms > crate::model::MAX_SAFE_JSON_INTEGER {
            return Err(Error::Invalid(
                "receipt timestamp exceeds the safe JSON integer range".into(),
            ));
        }
        let receipt = HandoffReceipt {
            version: HANDOFF_VERSION,
            handoff_id: id.to_owned(),
            recipient: recipient.clone(),
            received_at_unix_ms,
        };
        let bytes = serde_json::to_vec(&receipt)
            .map_err(|error| Error::Internal(format!("encode handoff receipt: {error}")))?;
        write_atomic(&self.root, &self.receipt_path(id), &bytes, 16 * 1024)
            .map_err(Error::Internal)?;
        Ok((receipt, true))
    }

    fn read_all(&self) -> Result<Vec<Handoff>, Error> {
        let entries = fs::read_dir(&self.root)
            .map_err(|error| Error::Internal(format!("list handoffs: {error}")))?;
        let mut records = Vec::new();
        let mut stored_bytes = 0u64;
        for entry in entries {
            let entry =
                entry.map_err(|error| Error::Internal(format!("list handoffs: {error}")))?;
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json")
                || path
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .is_none_or(|stem| !is_id(stem))
            {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            stored_bytes = stored_bytes.saturating_add(
                fs::symlink_metadata(&path)
                    .map_err(|error| Error::Internal(format!("stat handoff: {error}")))?
                    .len(),
            );
            if let Ok(metadata) = fs::symlink_metadata(self.receipt_path(id)) {
                stored_bytes = stored_bytes.saturating_add(metadata.len());
            }
            if stored_bytes > MAX_STORED_HANDOFF_BYTES {
                return Err(Error::Internal(
                    "communication store exceeds its configured byte bound".into(),
                ));
            }
            records.push(self.read_handoff(id)?);
            if records.len() > MAX_STORED_HANDOFFS {
                return Err(Error::Internal(
                    "communication store exceeds its configured record bound".into(),
                ));
            }
        }
        Ok(records)
    }

    fn stored_bytes(&self, records: &[Handoff]) -> Result<u64, Error> {
        let mut total = 0u64;
        for record in records {
            total = total.saturating_add(
                fs::symlink_metadata(self.handoff_path(&record.id))
                    .map_err(|error| Error::Internal(format!("stat handoff: {error}")))?
                    .len(),
            );
            if let Ok(metadata) = fs::symlink_metadata(self.receipt_path(&record.id)) {
                total = total.saturating_add(metadata.len());
            }
        }
        Ok(total)
    }

    fn read_handoff(&self, id: &str) -> Result<Handoff, Error> {
        if !is_id(id) {
            return Err(Error::Invalid("handoff id must be a canonical UUID".into()));
        }
        let path = self.handoff_path(id);
        let bytes = read_record_file(&path, MAX_HANDOFF_RECORD_BYTES).map_err(|error| {
            if error.contains("No such file or directory") {
                Error::NotFound
            } else {
                Error::Internal(error)
            }
        })?;
        let handoff: Handoff = serde_json::from_slice(&bytes).map_err(|error| {
            Error::Internal(format!(
                "invalid handoff record {}: {error}",
                path.display()
            ))
        })?;
        validate_handoff(&handoff)
            .map_err(|error| Error::Internal(format!("invalid persisted handoff: {error:?}")))?;
        if handoff.id != id {
            return Err(Error::Internal(
                "handoff id does not match its filename".into(),
            ));
        }
        Ok(handoff)
    }

    fn read_receipt(&self, id: &str) -> Result<Option<HandoffReceipt>, Error> {
        let path = self.receipt_path(id);
        let bytes = match read_record_file(&path, 16 * 1024) {
            Ok(bytes) => bytes,
            Err(error) if error.contains("No such file or directory") => return Ok(None),
            Err(error) => return Err(Error::Internal(error)),
        };
        let receipt: HandoffReceipt = serde_json::from_slice(&bytes)
            .map_err(|error| Error::Internal(format!("invalid handoff receipt: {error}")))?;
        if receipt.version != HANDOFF_VERSION {
            return Err(Error::Internal(
                "unsupported handoff receipt version".into(),
            ));
        }
        if receipt.handoff_id != id {
            return Err(Error::Internal(
                "receipt id does not match its filename".into(),
            ));
        }
        validate_identity(&receipt.recipient)
            .map_err(|error| Error::Internal(format!("invalid receipt identity: {error:?}")))?;
        if receipt.received_at_unix_ms > crate::model::MAX_SAFE_JSON_INTEGER {
            return Err(Error::Internal(
                "receipt timestamp exceeds the safe JSON integer range".into(),
            ));
        }
        Ok(Some(receipt))
    }

    fn prune_acked(&self, now_ms: i64) -> Result<(), Error> {
        let cutoff = now_ms.saturating_sub(ACKED_RETENTION_MS).max(0) as u64;
        for handoff in self.read_all()? {
            if let Some(receipt) = self.read_receipt(&handoff.id)?
                && receipt.received_at_unix_ms < cutoff
            {
                fs::remove_file(self.receipt_path(&handoff.id))
                    .map_err(|error| Error::Internal(format!("expire receipt: {error}")))?;
                fs::remove_file(self.handoff_path(&handoff.id))
                    .map_err(|error| Error::Internal(format!("expire handoff: {error}")))?;
            }
        }
        Ok(())
    }

    fn handoff_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }
    fn receipt_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.receipt.json"))
    }
}

fn validate_handoff(handoff: &Handoff) -> Result<(), Error> {
    if handoff.version != HANDOFF_VERSION {
        return Err(Error::Invalid("unsupported handoff record version".into()));
    }
    if !is_id(&handoff.id) {
        return Err(Error::Invalid("handoff id must be a canonical UUID".into()));
    }
    validate_identity(&handoff.sender)?;
    if !is_id(&handoff.recipient_subject_id) {
        return Err(Error::Invalid(
            "recipient subject must be a canonical UUID".into(),
        ));
    }
    if handoff.correlation_id.as_deref().is_some_and(|id| {
        id.is_empty() || id.len() > MAX_CORRELATION_ID_BYTES || id.chars().any(char::is_control)
    }) {
        return Err(Error::Invalid(format!(
            "correlation id must be nonempty, at most {MAX_CORRELATION_ID_BYTES} bytes, and contain no control characters"
        )));
    }
    if handoff.summary.chars().count() > MAX_HANDOFF_SUMMARY_CHARS {
        return Err(Error::Invalid(format!(
            "handoff summary exceeds {MAX_HANDOFF_SUMMARY_CHARS} characters"
        )));
    }
    if handoff.content.len() > MAX_HANDOFF_CONTENT_BYTES {
        return Err(Error::Invalid(format!(
            "handoff content exceeds {MAX_HANDOFF_CONTENT_BYTES} bytes"
        )));
    }
    if handoff.published_at_unix_ms > crate::model::MAX_SAFE_JSON_INTEGER {
        return Err(Error::Invalid(
            "handoff timestamp exceeds the safe JSON integer range".into(),
        ));
    }
    Ok(())
}

fn validate_identity(identity: &HandoffIdentity) -> Result<(), Error> {
    if !is_id(&identity.registration_id) || !is_id(&identity.session_id) {
        return Err(Error::Invalid("identity is incomplete".into()));
    }
    if is_id(&identity.subject_id) && is_id(&identity.claim_id) && !identity.claim_id.is_empty() {
        Ok(())
    } else {
        Err(Error::Invalid("managed identity is incomplete".into()))
    }
}

fn same_publication(existing: &Handoff, requested: &Handoff) -> bool {
    existing.id == requested.id
        && same_sender(&existing.sender, &requested.sender)
        && existing.recipient_subject_id == requested.recipient_subject_id
        && existing.correlation_id == requested.correlation_id
        && existing.summary == requested.summary
        && existing.content == requested.content
}

fn same_sender(left: &HandoffIdentity, right: &HandoffIdentity) -> bool {
    left.subject_id == right.subject_id
}

pub fn is_parent_recipient(parent_subject_id: Option<&str>, recipient_subject_id: &str) -> bool {
    parent_subject_id == Some(recipient_subject_id)
}

pub fn recipient_matches(recipient_subject_id: &str, identity: &HandoffIdentity) -> bool {
    recipient_subject_id == identity.subject_id
}

fn is_id(value: &str) -> bool {
    SessionUuid::parse(value).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::store::random_uuid;

    fn identity(subject_id: Option<String>) -> HandoffIdentity {
        HandoffIdentity {
            registration_id: random_uuid(),
            session_id: random_uuid(),
            subject_id: subject_id.unwrap_or_else(random_uuid),
            claim_id: random_uuid(),
        }
    }
    fn sample(sender: &HandoffIdentity, recipient: &HandoffIdentity) -> Handoff {
        Handoff {
            version: HANDOFF_VERSION,
            id: random_uuid(),
            sender: sender.clone(),
            recipient_subject_id: recipient.subject_id.clone(),
            correlation_id: Some(random_uuid()),
            summary: "finished".into(),
            content: "Full result".into(),
            published_at_unix_ms: 1_800_000_000_000,
        }
    }
    struct TestDir(PathBuf);

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    impl TestDir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    fn store() -> (TestDir, CommunicationStore) {
        let path = std::env::temp_dir().join(format!("radar-communication-{}", random_uuid()));
        fs::create_dir_all(&path).unwrap();
        let dir = TestDir(path);
        let store = CommunicationStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn managed_handoff_targets_only_the_durable_parent_subject() {
        let parent = random_uuid();
        assert!(is_parent_recipient(Some(&parent), &parent));
        assert!(!is_parent_recipient(Some(&parent), &random_uuid()));
        assert!(!is_parent_recipient(None, &parent));
    }

    #[test]
    fn pending_is_recoverable_after_reopen_and_ack_does_not_delete_content() {
        let (dir, store) = store();
        let sender = identity(Some(random_uuid()));
        let recipient = identity(Some(random_uuid()));
        let message = sample(&sender, &recipient);
        store.publish(&message).unwrap();
        assert_eq!(store.pending(&recipient).unwrap(), vec![message.clone()]);
        drop(store);
        let reopened = CommunicationStore::open(dir.path()).unwrap();
        assert_eq!(reopened.pending(&recipient).unwrap(), vec![message.clone()]);
        let (receipt, created) = reopened
            .acknowledge(&message.id, &recipient, 1_800_000_000_100)
            .unwrap();
        assert!(created);
        assert!(reopened.pending(&recipient).unwrap().is_empty());
        let (retrieved, stored_receipt) = reopened.get(&message.id, &recipient).unwrap();
        assert_eq!(retrieved, message);
        assert_eq!(stored_receipt, Some(receipt));
        assert!(
            !reopened
                .acknowledge(&retrieved.id, &recipient, 1_800_000_000_200)
                .unwrap()
                .1
        );
    }

    #[test]
    fn expired_acknowledged_handoffs_are_pruned_but_pending_work_is_not() {
        let (_dir, store) = store();
        let sender = identity(Some(random_uuid()));
        let recipient = identity(Some(random_uuid()));
        let acknowledged = sample(&sender, &recipient);
        store.publish(&acknowledged).unwrap();
        store.acknowledge(&acknowledged.id, &recipient, 1).unwrap();

        let pending = sample(&sender, &recipient);
        store.publish(&pending).unwrap();
        assert_eq!(store.pending(&recipient).unwrap(), vec![pending.clone()]);
        assert_eq!(
            store.get(&acknowledged.id, &recipient),
            Err(Error::NotFound)
        );
    }

    #[test]
    fn oversized_payload_is_rejected_without_creating_a_pending_record() {
        let (_dir, store) = store();
        let sender = identity(Some(random_uuid()));
        let recipient = identity(Some(random_uuid()));
        let mut message = sample(&sender, &recipient);
        message.content = "x".repeat(MAX_HANDOFF_CONTENT_BYTES + 1);
        assert!(matches!(store.publish(&message), Err(Error::Invalid(_))));
        assert!(store.pending(&recipient).unwrap().is_empty());
    }

    #[test]
    fn only_the_addressed_subject_can_acknowledge_and_successor_keeps_the_inbox() {
        let (_dir, store) = store();
        let sender = identity(Some(random_uuid()));
        let recipient = identity(Some(random_uuid()));
        let message = sample(&sender, &recipient);
        store.publish(&message).unwrap();
        let successor = HandoffIdentity {
            registration_id: random_uuid(),
            session_id: random_uuid(),
            claim_id: random_uuid(),
            subject_id: recipient.subject_id.clone(),
        };
        assert!(store.pending(&successor).unwrap().len() == 1);
        assert_eq!(
            store.acknowledge(
                &message.id,
                &identity(Some(random_uuid())),
                1_800_000_000_100
            ),
            Err(Error::WrongRecipient)
        );
        assert!(
            store
                .acknowledge(&message.id, &successor, 1_800_000_000_100)
                .unwrap()
                .1
        );
        assert!(store.pending(&recipient).unwrap().is_empty());
    }
}
