use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use crate::control_plane::public_contracts::{
    AgentIdentity, Attention, Message, MessageReceipt, PublishBody,
};
use crate::control_plane::registry::{publication::write_atomic, read_record_file};
use crate::model::SessionUuid;

pub const MESSAGE_VERSION: u32 = 1;
pub const MAX_CONTENT_BYTES: usize = 384 * 1024;
pub const MAX_SUMMARY_CHARS: usize = 600;
pub const MAX_CORRELATION_ID_BYTES: usize = 256;
pub const MAX_RECIPIENTS: usize = 32;
pub const MAX_PENDING_PER_SUBJECT: usize = 128;
pub const MAX_PAGE: usize = 32;
const MAX_RECORD_BYTES: usize = 900 * 1024;
const MAX_STORED_MESSAGES: usize = 512;
const MAX_STORED_BYTES: u64 = 64 * 1024 * 1024;
const MAX_RECEIPT_BYTES: usize = 16 * 1024;
const RECEIVED_RETENTION_MS: i64 = 30 * 24 * 60 * 60 * 1000;

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Invalid(String),
    Conflict,
    NotFound,
    WrongRecipient,
    Capacity,
    Internal(String),
}

/// One durable, producer-addressed message lane: a message is written once with
/// its whole recipient set, and each recipient's receipt is its own record.
///
/// The files are the truth. `index` is what readers consult: rebuilt from them at
/// open and changed only after the file it mirrors is durable, so a read never
/// touches the disk and never waits behind a write.
pub struct MessageStore {
    root: PathBuf,
    /// Distinguishes this daemon instance: a revision earned in an earlier
    /// process never compares equal to one from this process.
    instance: String,
    /// Serialises mutations. Reads never take it.
    writes: Mutex<()>,
    index: RwLock<BTreeMap<String, Entry>>,
}

struct Entry {
    recipients: Vec<String>,
    created_at: u64,
    /// The message file and its receipts, for the storage bound.
    bytes: u64,
    /// Receipt time per recipient that has captured the message.
    received: BTreeMap<String, u64>,
}

impl Entry {
    fn of(message: &Message, bytes: u64) -> Self {
        Self {
            recipients: message.recipients.clone(),
            created_at: message.created_at_unix_ms,
            bytes,
            received: BTreeMap::new(),
        }
    }

    fn pending_for(&self, subject: &str) -> bool {
        self.recipients.iter().any(|recipient| recipient == subject)
            && !self.received.contains_key(subject)
    }
}

/// A pending message's place in acceptance order; the id makes it total.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    created_at: u64,
    id: String,
}

impl Position {
    fn cursor(&self) -> String {
        format!("{}:{}", self.created_at, self.id)
    }
}

impl MessageStore {
    pub fn open(state_root: &Path) -> Result<Self, String> {
        let root = state_root.join("communications");
        fs::create_dir_all(&root)
            .map_err(|error| format!("message store {}: {error}", root.display()))?;
        let metadata = fs::symlink_metadata(&root)
            .map_err(|error| format!("message store {}: {error}", root.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "message store {} is not a directory",
                root.display()
            ));
        }
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("message store {}: {error}", root.display()))?;
        let instance = format!(
            "{:016x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos() as u64)
                .unwrap_or_default()
        );
        let store = Self {
            root,
            instance,
            writes: Mutex::new(()),
            index: RwLock::new(BTreeMap::new()),
        };
        store.reject_legacy_records()?;
        store
            .load()
            .map_err(|error| format!("message store {}: {error:?}", store.root.display()))?;
        Ok(store)
    }

    /// Accepts one immutable message, addressed to its whole recipient set.
    pub fn publish(&self, message: &Message) -> Result<(Message, bool), Error> {
        validate_message(message)?;
        let _writes = self.writes();
        self.prune_received(message.created_at_unix_ms as i64)?;
        if self.index().contains_key(&message.id) {
            let existing = self.read_message(&message.id)?;
            return if same_publication(&existing, message) {
                Ok((existing, false))
            } else {
                Err(Error::Conflict)
            };
        }
        let bytes = serde_json::to_vec(message)
            .map_err(|error| Error::Internal(format!("encode message: {error}")))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(Error::Invalid("message record is too large".into()));
        }
        {
            let index = self.index();
            let stored: u64 = index.values().map(|entry| entry.bytes).sum();
            if index.len() >= MAX_STORED_MESSAGES
                || stored.saturating_add(bytes.len() as u64) > MAX_STORED_BYTES
                || message.recipients.iter().any(|recipient| {
                    index
                        .values()
                        .filter(|entry| entry.pending_for(recipient))
                        .count()
                        >= MAX_PENDING_PER_SUBJECT
                })
            {
                return Err(Error::Capacity);
            }
        }
        write_atomic(
            &self.root,
            &self.message_path(&message.id),
            &bytes,
            MAX_RECORD_BYTES,
        )
        .map_err(Error::Internal)?;
        self.index_mut()
            .insert(message.id.clone(), Entry::of(message, bytes.len() as u64));
        Ok((message.clone(), true))
    }

    /// One bounded page of pending ids. The cursor is the last returned
    /// acceptance position, so a page stays valid even after another recipient
    /// receives its copy.
    pub fn page(
        &self,
        subject: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<String>, Option<String>), Error> {
        let limit = limit.clamp(1, MAX_PAGE);
        let after = after.map(parse_cursor).transpose()?;
        let mut pending = self.pending(subject);
        if let Some(after) = after {
            pending.retain(|position| *position > after);
        }
        let next = (pending.len() > limit).then(|| pending[limit - 1].cursor());
        pending.truncate(limit);
        Ok((
            pending.into_iter().map(|position| position.id).collect(),
            next,
        ))
    }

    pub fn get(&self, id: &str, subject: &str) -> Result<(Message, Option<MessageReceipt>), Error> {
        let captured = self.captured(id, subject)?;
        let message = self.read_message(id)?;
        let receipt = if captured {
            self.read_receipt(id, subject)?
        } else {
            None
        };
        Ok((message, receipt))
    }

    /// Records durable recipient-side capture. Repeating it returns the first
    /// receipt: a receipt attests capture only, never understanding or settlement.
    pub fn receive(
        &self,
        id: &str,
        recipient: &AgentIdentity,
        received_at_unix_ms: u64,
    ) -> Result<(MessageReceipt, bool), Error> {
        let _writes = self.writes();
        self.prune_received(crate::control_plane::store::now_ms())?;
        let subject = &recipient.subject_id;
        if self.captured(id, subject)? {
            let first = self
                .read_receipt(id, subject)?
                .ok_or_else(|| Error::Internal("an indexed message receipt is missing".into()))?;
            return Ok((first, false));
        }
        if received_at_unix_ms > crate::model::MAX_SAFE_JSON_INTEGER {
            return Err(Error::Invalid(
                "receipt timestamp exceeds the safe JSON integer range".into(),
            ));
        }
        let receipt = MessageReceipt {
            version: MESSAGE_VERSION,
            message_id: id.to_owned(),
            recipient: recipient.clone(),
            received_at_unix_ms,
        };
        let bytes = serde_json::to_vec(&receipt)
            .map_err(|error| Error::Internal(format!("encode message receipt: {error}")))?;
        write_atomic(
            &self.root,
            &self.receipt_path(id, subject),
            &bytes,
            MAX_RECEIPT_BYTES,
        )
        .map_err(Error::Internal)?;
        if let Some(entry) = self.index_mut().get_mut(id) {
            entry.received.insert(subject.clone(), received_at_unix_ms);
            entry.bytes = entry.bytes.saturating_add(bytes.len() as u64);
        }
        Ok((receipt, true))
    }

    /// What one recipient has waiting. The revision is opaque: it changes when
    /// this subject's pending set changes and never on another subject's work.
    pub fn attention(&self, subject: &str) -> Attention {
        let pending = self.pending(subject);
        let mut state = String::new();
        for position in &pending {
            let _ = write!(state, "{}:{};", position.id, position.created_at);
        }
        Attention {
            revision: format!(
                "{}:{}",
                self.instance,
                crate::content::digest(format!("{subject}\0{state}").as_bytes())
            ),
            pending: pending.len() as u64,
        }
    }

    fn pending(&self, subject: &str) -> Vec<Position> {
        let mut pending: Vec<Position> = self
            .index()
            .iter()
            .filter(|(_, entry)| entry.pending_for(subject))
            .map(|(id, entry)| Position {
                created_at: entry.created_at,
                id: id.clone(),
            })
            .collect();
        pending.sort();
        pending
    }

    /// Whether this recipient already holds the message, once the id is known to
    /// be one this recipient is addressed by.
    fn captured(&self, id: &str, subject: &str) -> Result<bool, Error> {
        if !is_id(id) {
            return Err(Error::Invalid("message id must be a canonical UUID".into()));
        }
        let index = self.index();
        let entry = index.get(id).ok_or(Error::NotFound)?;
        if !entry
            .recipients
            .iter()
            .any(|recipient| recipient == subject)
        {
            return Err(Error::WrongRecipient);
        }
        Ok(entry.received.contains_key(subject))
    }

    /// Rebuilds the index from the files. A receipt whose message is gone is the
    /// residue of an interrupted expiry, which removes the message first.
    fn load(&self) -> Result<(), Error> {
        let listing = |error: std::io::Error| Error::Internal(format!("list messages: {error}"));
        let mut entries = BTreeMap::new();
        let mut receipts = Vec::new();
        for entry in fs::read_dir(&self.root).map_err(listing)? {
            let name = entry.map_err(listing)?.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(id) = parse_message_file(name) {
                let message = self.read_message(id)?;
                entries.insert(
                    id.to_owned(),
                    Entry::of(&message, file_len(&self.message_path(id))),
                );
                if entries.len() > MAX_STORED_MESSAGES {
                    return Err(Error::Internal(
                        "message store exceeds its configured record bound".into(),
                    ));
                }
            } else if let Some((id, subject)) = parse_receipt_file(name) {
                receipts.push((id.to_owned(), subject.to_owned()));
            }
        }
        for (id, subject) in receipts {
            match entries
                .get_mut(&id)
                .filter(|entry| entry.recipients.contains(&subject))
            {
                Some(entry) => {
                    if let Some(receipt) = self.read_receipt(&id, &subject)? {
                        entry
                            .received
                            .insert(subject.clone(), receipt.received_at_unix_ms);
                        entry.bytes = entry
                            .bytes
                            .saturating_add(file_len(&self.receipt_path(&id, &subject)));
                    }
                }
                None => fs::remove_file(self.receipt_path(&id, &subject)).map_err(|error| {
                    Error::Internal(format!("remove orphaned message receipt: {error}"))
                })?,
            }
        }
        *self.index_mut() = entries;
        self.prune_received(crate::control_plane::store::now_ms())
    }

    fn read_message(&self, id: &str) -> Result<Message, Error> {
        if !is_id(id) {
            return Err(Error::Invalid("message id must be a canonical UUID".into()));
        }
        let path = self.message_path(id);
        let bytes = read_record_file(&path, MAX_RECORD_BYTES).map_err(|error| {
            if error.contains("No such file or directory") {
                Error::NotFound
            } else {
                Error::Internal(error)
            }
        })?;
        let message: Message = serde_json::from_slice(&bytes).map_err(|error| {
            Error::Internal(format!(
                "invalid message record {}: {error}",
                path.display()
            ))
        })?;
        validate_message(&message)
            .map_err(|error| Error::Internal(format!("invalid persisted message: {error:?}")))?;
        if message.id != id {
            return Err(Error::Internal(
                "message id does not match its filename".into(),
            ));
        }
        Ok(message)
    }

    fn read_receipt(&self, id: &str, subject: &str) -> Result<Option<MessageReceipt>, Error> {
        let path = self.receipt_path(id, subject);
        let bytes = match read_record_file(&path, MAX_RECEIPT_BYTES) {
            Ok(bytes) => bytes,
            Err(error) if error.contains("No such file or directory") => return Ok(None),
            Err(error) => return Err(Error::Internal(error)),
        };
        let receipt: MessageReceipt = serde_json::from_slice(&bytes)
            .map_err(|error| Error::Internal(format!("invalid message receipt: {error}")))?;
        if receipt.version != MESSAGE_VERSION {
            return Err(Error::Internal(
                "unsupported message receipt version".into(),
            ));
        }
        if receipt.message_id != id || receipt.recipient.subject_id != subject {
            return Err(Error::Internal(
                "receipt does not match its filename".into(),
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

    /// Retires a body only once every recipient has had it for the retention
    /// window. A message pending for anyone is never expired to make room.
    /// Callers hold `writes`. The message goes first, so a crash leaves receipts
    /// that `load` sweeps, never a message that looks pending again.
    fn prune_received(&self, now_ms: i64) -> Result<(), Error> {
        let cutoff = now_ms.saturating_sub(RECEIVED_RETENTION_MS).max(0) as u64;
        let expired: Vec<(String, Vec<String>)> = self
            .index()
            .iter()
            .filter(|(_, entry)| {
                entry.received.len() == entry.recipients.len()
                    && entry.received.values().all(|&at| at < cutoff)
            })
            .map(|(id, entry)| (id.clone(), entry.recipients.clone()))
            .collect();
        for (id, recipients) in expired {
            fs::remove_file(self.message_path(&id))
                .map_err(|error| Error::Internal(format!("expire message: {error}")))?;
            self.index_mut().remove(&id);
            for recipient in &recipients {
                fs::remove_file(self.receipt_path(&id, recipient))
                    .map_err(|error| Error::Internal(format!("expire message receipt: {error}")))?;
            }
        }
        Ok(())
    }

    fn writes(&self) -> MutexGuard<'_, ()> {
        self.writes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn index(&self) -> RwLockReadGuard<'_, BTreeMap<String, Entry>> {
        self.index.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn index_mut(&self) -> RwLockWriteGuard<'_, BTreeMap<String, Entry>> {
        self.index.write().unwrap_or_else(PoisonError::into_inner)
    }

    fn reject_legacy_records(&self) -> Result<(), String> {
        let entries = fs::read_dir(&self.root)
            .map_err(|error| format!("message store {}: {error}", self.root.display()))?;
        let mut legacy = Vec::new();
        for entry in entries {
            let entry =
                entry.map_err(|error| format!("message store {}: {error}", self.root.display()))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.ends_with(".json") {
                continue;
            }
            if parse_message_file(name).is_none() && parse_receipt_file(name).is_none() {
                legacy.push(name.to_owned());
            }
        }
        if legacy.is_empty() {
            return Ok(());
        }
        legacy.sort();
        Err(format!(
            "message store {} holds {} record(s) from the retired handoff mailbox, starting with `{}`: drain them with the previous daemon or remove the directory before starting this daemon",
            self.root.display(),
            legacy.len(),
            legacy[0]
        ))
    }

    fn message_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.message.json"))
    }
    fn receipt_path(&self, id: &str, subject: &str) -> PathBuf {
        self.root.join(format!("{id}.{subject}.receipt.json"))
    }
}

fn file_len(path: &Path) -> u64 {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

fn parse_cursor(cursor: &str) -> Result<Position, Error> {
    let invalid = || Error::Invalid("page cursor is not an acceptance position".into());
    let (created, id) = cursor.split_once(':').ok_or_else(invalid)?;
    let created_at = created.parse::<u64>().map_err(|_| invalid())?;
    if !is_id(id) {
        return Err(invalid());
    }
    Ok(Position {
        created_at,
        id: id.to_owned(),
    })
}

fn parse_message_file(name: &str) -> Option<&str> {
    let id = name.strip_suffix(".message.json")?;
    is_id(id).then_some(id)
}

fn parse_receipt_file(name: &str) -> Option<(&str, &str)> {
    let stem = name.strip_suffix(".receipt.json")?;
    let (id, subject) = stem.split_once('.')?;
    (is_id(id) && is_id(subject)).then_some((id, subject))
}

/// What makes two publications the same one. The daemon assigns acceptance
/// time, so a producer retrying identical content is not a conflict.
fn same_publication(left: &Message, right: &Message) -> bool {
    left.id == right.id
        && left.producer == right.producer
        && left.recipients == right.recipients
        && left.body == right.body
}

fn validate_message(message: &Message) -> Result<(), Error> {
    if message.version != MESSAGE_VERSION {
        return Err(Error::Invalid("unsupported message record version".into()));
    }
    if !is_id(&message.id) {
        return Err(Error::Invalid("message id must be a canonical UUID".into()));
    }
    validate_identity(&message.producer)?;
    if message.recipients.is_empty() {
        return Err(Error::Invalid(
            "a message needs at least one recipient".into(),
        ));
    }
    if message.recipients.len() > MAX_RECIPIENTS {
        return Err(Error::Invalid(format!(
            "a message may address at most {MAX_RECIPIENTS} recipients"
        )));
    }
    if message.recipients.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(Error::Invalid(
            "recipients must be sorted and unique subject ids".into(),
        ));
    }
    for recipient in &message.recipients {
        if !is_id(recipient) {
            return Err(Error::Invalid(
                "recipient subject must be a canonical UUID".into(),
            ));
        }
    }
    validate_body(&message.body)?;
    if message.created_at_unix_ms > crate::model::MAX_SAFE_JSON_INTEGER {
        return Err(Error::Invalid(
            "message timestamp exceeds the safe JSON integer range".into(),
        ));
    }
    Ok(())
}

fn validate_body(body: &PublishBody) -> Result<(), Error> {
    let PublishBody::Handoff {
        correlation_id,
        summary,
        content,
    } = body;
    if correlation_id.as_deref().is_some_and(|id| {
        id.is_empty() || id.len() > MAX_CORRELATION_ID_BYTES || id.chars().any(char::is_control)
    }) {
        return Err(Error::Invalid(format!(
            "correlation id must be nonempty, at most {MAX_CORRELATION_ID_BYTES} bytes, and contain no control characters"
        )));
    }
    if summary.chars().count() > MAX_SUMMARY_CHARS {
        return Err(Error::Invalid(format!(
            "message summary exceeds {MAX_SUMMARY_CHARS} characters"
        )));
    }
    if content.len() > MAX_CONTENT_BYTES {
        return Err(Error::Invalid(format!(
            "message content exceeds {MAX_CONTENT_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_identity(identity: &AgentIdentity) -> Result<(), Error> {
    for (label, value) in [
        ("registration", &identity.registration_id),
        ("session", &identity.session_id),
        ("claim", &identity.claim_id),
    ] {
        if value.is_empty() || value.chars().any(char::is_control) {
            return Err(Error::Invalid(format!("{label} id is invalid")));
        }
    }
    if !is_id(&identity.subject_id) {
        return Err(Error::Invalid("subject id must be a canonical UUID".into()));
    }
    Ok(())
}

/// Whether this subject is the sender's recorded parent.
pub fn is_parent_recipient(parent_subject_id: Option<&str>, recipient: &str) -> bool {
    parent_subject_id == Some(recipient)
}

/// A canonical UUID, which is also the only shape an id takes on disk.
pub fn is_id(value: &str) -> bool {
    SessionUuid::parse(value).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::public_contracts::PublishBody;

    fn identity(subject: &str) -> AgentIdentity {
        AgentIdentity {
            registration_id: format!("registration-{subject}"),
            session_id: format!("session-{subject}"),
            subject_id: subject.to_owned(),
            claim_id: format!("claim-{subject}"),
        }
    }

    fn message(id: &str, recipients: &[&str], content: &str) -> Message {
        Message {
            version: MESSAGE_VERSION,
            id: id.to_owned(),
            producer: identity("11111111-1111-4111-8111-111111111111"),
            recipients: recipients
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            body: PublishBody::Handoff {
                correlation_id: Some("run-1".into()),
                summary: "worker completed".into(),
                content: content.into(),
            },
            created_at_unix_ms: 1_000,
        }
    }

    fn open() -> (PathBuf, MessageStore) {
        let state = std::env::temp_dir().join(format!(
            "radar-messages-{}",
            crate::control_plane::store::random_uuid()
        ));
        let store = MessageStore::open(&state).unwrap();
        (state, store)
    }

    const P: &str = "22222222-2222-4222-8222-222222222222";
    const Q: &str = "33333333-3333-4333-8333-333333333333";
    const M1: &str = "44444444-4444-4444-8444-444444444444";
    const M2: &str = "55555555-5555-4555-8555-555555555555";

    #[test]
    fn a_message_is_written_once_with_its_whole_recipient_set() {
        let (_state, store) = open();
        let published = message(M1, &[P, Q], "body");
        assert!(store.publish(&published).unwrap().1);
        assert!(
            !store.publish(&published).unwrap().1,
            "a retry returns the accepted message"
        );
        assert_eq!(store.pending(P).len(), 1);
        assert_eq!(store.pending(Q).len(), 1);
        let mut changed = message(M1, &[P, Q], "different body");
        changed.created_at_unix_ms = 1_000;
        assert_eq!(store.publish(&changed), Err(Error::Conflict));
    }

    #[test]
    fn one_recipient_receipt_leaves_the_other_pending() {
        let (_state, store) = open();
        store.publish(&message(M1, &[P, Q], "body")).unwrap();
        let (receipt, created) = store.receive(M1, &identity(P), 2_000).unwrap();
        assert_eq!((receipt.message_id.as_str(), created), (M1, true));
        assert_eq!(
            store.receive(M1, &identity(P), 2_500).unwrap().0,
            receipt,
            "a repeated receipt returns the first one"
        );
        assert!(store.pending(P).is_empty());
        assert_eq!(store.pending(Q).len(), 1);
        assert_eq!(
            store.get(M1, Q).unwrap().0.body,
            message(M1, &[P, Q], "body").body,
            "content stays retrievable after another recipient's receipt"
        );
    }

    #[test]
    fn attention_follows_only_this_recipients_mailbox() {
        let (_state, store) = open();
        let quiet = store.attention(Q);
        assert_eq!(
            (quiet.pending, quiet.revision.as_str()),
            (0, quiet.revision.as_str())
        );
        store.publish(&message(M1, &[P], "body")).unwrap();
        let waiting = store.attention(P);
        assert_eq!(waiting.pending, 1);
        assert_ne!(waiting.revision, quiet.revision);
        assert_eq!(
            store.attention(Q).revision,
            quiet.revision,
            "another subject's delivery is not Q's attention"
        );
        store.receive(M1, &identity(P), 2_000).unwrap();
        assert_eq!(store.attention(P).pending, 0);
        assert_ne!(
            store.attention(P).revision,
            quiet.revision,
            "receipt is a mailbox change"
        );
    }

    #[test]
    fn a_rebuilt_store_reports_the_same_pending_work_without_an_announcement() {
        let (state, store) = open();
        store.publish(&message(M1, &[P], "body")).unwrap();
        let before = store.attention(P);
        drop(store);
        let rebuilt = MessageStore::open(&state).unwrap();
        let after = rebuilt.attention(P);
        assert_eq!(after.pending, before.pending);
        assert_ne!(
            after.revision, before.revision,
            "a new instance never reuses an earlier revision"
        );
        assert_eq!(rebuilt.pending(P).len(), 1);
    }

    #[test]
    fn pages_continue_from_an_acceptance_position() {
        let (_state, store) = open();
        let mut first = message(M1, &[P], "one");
        first.created_at_unix_ms = 1_000;
        let mut second = message(M2, &[P], "two");
        second.created_at_unix_ms = 2_000;
        store.publish(&second).unwrap();
        store.publish(&first).unwrap();
        let (page, next) = store.page(P, None, 1).unwrap();
        assert_eq!(page[0], M1);
        assert_eq!(
            next.as_deref(),
            Some("1000:44444444-4444-4444-8444-444444444444")
        );
        let (page, next) = store.page(P, next.as_deref(), 1).unwrap();
        assert_eq!(page[0], M2);
        assert_eq!(next, None);
    }

    #[test]
    fn a_full_mailbox_refuses_the_whole_publication() {
        let (_state, store) = open();
        for index in 0..MAX_PENDING_PER_SUBJECT {
            let id = format!("{:08x}-0000-4000-8000-000000000000", index);
            store.publish(&message(&id, &[P], "body")).unwrap();
        }
        let refused = store.publish(&message(M1, &[P, Q], "body"));
        assert_eq!(refused, Err(Error::Capacity));
        assert!(
            store.pending(Q).is_empty(),
            "a refused publication delivers to nobody"
        );
        assert_eq!(store.pending(P).len(), MAX_PENDING_PER_SUBJECT);
    }

    #[test]
    fn a_retired_mailbox_is_refused_rather_than_migrated() {
        let state = std::env::temp_dir().join(format!(
            "radar-messages-{}",
            crate::control_plane::store::random_uuid()
        ));
        let root = state.join("communications");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join(format!("{M1}.json")), b"{}").unwrap();
        let error = match MessageStore::open(&state) {
            Ok(_) => panic!("legacy records were accepted"),
            Err(error) => error,
        };
        assert!(error.contains("retired handoff mailbox"), "{error}");
    }

    #[test]
    fn a_rebuilt_store_remembers_which_recipients_hold_a_message() {
        let (state, store) = open();
        store.publish(&message(M1, &[P, Q], "body")).unwrap();
        store.receive(M1, &identity(P), 2_000).unwrap();
        drop(store);
        let rebuilt = MessageStore::open(&state).unwrap();
        assert!(rebuilt.pending(P).is_empty());
        assert_eq!(rebuilt.pending(Q).len(), 1);
        assert!(rebuilt.get(M1, P).unwrap().1.is_some());
    }

    #[test]
    fn a_message_is_retired_once_every_recipient_has_held_it_for_the_window() {
        let (state, store) = open();
        store.publish(&message(M1, &[P], "body")).unwrap();
        store.receive(M1, &identity(P), 2_000).unwrap();
        let mut later = message(M2, &[Q], "body");
        later.created_at_unix_ms = 2_000 + RECEIVED_RETENTION_MS as u64 + 1;
        store.publish(&later).unwrap();
        assert_eq!(store.get(M1, P), Err(Error::NotFound));
        let root = state.join("communications");
        assert!(!root.join(format!("{M1}.message.json")).exists());
        assert!(!root.join(format!("{M1}.{P}.receipt.json")).exists());
        assert_eq!(
            store.pending(Q).len(),
            1,
            "work still pending is never retired"
        );
    }

    #[test]
    fn a_receipt_left_by_an_interrupted_expiry_never_revives_its_message() {
        let (state, store) = open();
        store.publish(&message(M1, &[P], "body")).unwrap();
        store.receive(M1, &identity(P), 2_000).unwrap();
        drop(store);
        let root = state.join("communications");
        fs::remove_file(root.join(format!("{M1}.message.json"))).unwrap();
        let rebuilt = MessageStore::open(&state).unwrap();
        assert!(rebuilt.pending(P).is_empty());
        assert!(
            !root.join(format!("{M1}.{P}.receipt.json")).exists(),
            "the orphaned receipt is swept"
        );
    }
}
