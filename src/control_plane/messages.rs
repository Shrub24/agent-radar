use std::fmt::Write as _;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

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
pub struct MessageStore {
    root: PathBuf,
    /// Distinguishes this daemon instance: a revision earned in an earlier
    /// process never compares equal to one from this process.
    instance: String,
    lock: Mutex<()>,
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
        // Never reused: a revision earned by an earlier process must not compare
        // equal to one from this process.
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
            lock: Mutex::new(()),
        };
        store.reject_legacy_records()?;
        Ok(store)
    }

    /// Accepts one immutable message, addressed to its whole recipient set.
    pub fn publish(&self, message: &Message) -> Result<(Message, bool), Error> {
        validate_message(message)?;
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_received(message.created_at_unix_ms as i64)?;
        let path = self.message_path(&message.id);
        if path.exists() {
            let existing = self.read_message(&message.id)?;
            if same_publication(&existing, message) {
                return Ok((existing, false));
            }
            return Err(Error::Conflict);
        }
        let records = self.read_all()?;
        let stored_bytes = self.stored_bytes(&records);
        if records.len() >= MAX_STORED_MESSAGES || stored_bytes >= MAX_STORED_BYTES {
            return Err(Error::Capacity);
        }
        for recipient in &message.recipients {
            if records
                .iter()
                .filter(|record| {
                    record.recipients.contains(recipient)
                        && !self.receipt_path(&record.id, recipient).exists()
                })
                .count()
                >= MAX_PENDING_PER_SUBJECT
            {
                return Err(Error::Capacity);
            }
        }
        let bytes = serde_json::to_vec(message)
            .map_err(|error| Error::Internal(format!("encode message: {error}")))?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err(Error::Invalid("message record is too large".into()));
        }
        if stored_bytes.saturating_add(bytes.len() as u64) > MAX_STORED_BYTES {
            return Err(Error::Capacity);
        }
        write_atomic(&self.root, &path, &bytes, MAX_RECORD_BYTES).map_err(Error::Internal)?;
        Ok((message.clone(), true))
    }

    /// Pending deliveries for one recipient, oldest acceptance first.
    pub fn pending(&self, subject: &str) -> Result<Vec<Message>, Error> {
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_received(crate::control_plane::store::now_ms())?;
        self.pending_unlocked(subject)
    }

    /// One bounded page. The cursor is the last returned acceptance position,
    /// so a page stays valid even after another recipient receives its copy.
    pub fn page(
        &self,
        subject: &str,
        after: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<Message>, Option<String>), Error> {
        let limit = limit.clamp(1, MAX_PAGE);
        let mut pending = self.pending(subject)?;
        pending.sort_by(acceptance_order);
        if let Some(cursor) = after.map(parse_cursor).transpose()? {
            pending.retain(|message| accept_position(message) > cursor);
        }
        let more = pending.len() > limit;
        pending.truncate(limit);
        let next = more.then(|| pending.last().map(format_cursor).unwrap_or_default());
        Ok((pending, next))
    }

    pub fn get(&self, id: &str, subject: &str) -> Result<(Message, Option<MessageReceipt>), Error> {
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_received(crate::control_plane::store::now_ms())?;
        let message = self.read_message(id)?;
        if !message
            .recipients
            .iter()
            .any(|recipient| recipient == subject)
        {
            return Err(Error::WrongRecipient);
        }
        let receipt = self.read_receipt(id, subject)?;
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
        let _guard = self.lock.lock().unwrap_or_else(|error| error.into_inner());
        self.prune_received(crate::control_plane::store::now_ms())?;
        let message = self.read_message(id)?;
        if !message
            .recipients
            .iter()
            .any(|addressed| addressed == &recipient.subject_id)
        {
            return Err(Error::WrongRecipient);
        }
        if let Some(receipt) = self.read_receipt(id, &recipient.subject_id)? {
            return Ok((receipt, false));
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
            &self.receipt_path(id, &recipient.subject_id),
            &bytes,
            MAX_RECEIPT_BYTES,
        )
        .map_err(Error::Internal)?;
        Ok((receipt, true))
    }

    /// What one recipient has waiting. The revision is opaque: it changes when
    /// this subject's pending set changes and never on another subject's work.
    pub fn attention(&self, subject: &str) -> Result<Attention, Error> {
        let mut pending = self.pending(subject)?;
        pending.sort_by(acceptance_order);
        let mut state = String::new();
        for message in &pending {
            let _ = write!(state, "{}:{};", message.id, message.created_at_unix_ms);
        }
        let revision = format!(
            "{}:{}",
            self.instance,
            crate::content::digest(format!("{subject}\0{state}").as_bytes())
        );
        Ok(Attention {
            revision,
            pending: pending.len() as u64,
        })
    }

    fn pending_unlocked(&self, subject: &str) -> Result<Vec<Message>, Error> {
        let mut records: Vec<Message> = self
            .read_all()?
            .into_iter()
            .filter(|record| {
                record
                    .recipients
                    .iter()
                    .any(|recipient| recipient == subject)
                    && !self.receipt_path(&record.id, subject).exists()
            })
            .collect();
        records.sort_by(acceptance_order);
        Ok(records)
    }

    fn read_all(&self) -> Result<Vec<Message>, Error> {
        let mut records = Vec::new();
        for entry in fs::read_dir(&self.root)
            .map_err(|error| Error::Internal(format!("list messages: {error}")))?
        {
            let entry =
                entry.map_err(|error| Error::Internal(format!("list messages: {error}")))?;
            let Some(id) = entry
                .file_name()
                .to_str()
                .and_then(parse_message_file)
                .map(str::to_owned)
            else {
                continue;
            };
            records.push(self.read_message(&id)?);
            if records.len() > MAX_STORED_MESSAGES {
                return Err(Error::Internal(
                    "message store exceeds its configured record bound".into(),
                ));
            }
        }
        Ok(records)
    }

    fn stored_bytes(&self, records: &[Message]) -> u64 {
        let mut total = 0u64;
        for record in records {
            if let Ok(metadata) = fs::symlink_metadata(self.message_path(&record.id)) {
                total = total.saturating_add(metadata.len());
            }
            for recipient in &record.recipients {
                if let Ok(metadata) = fs::symlink_metadata(self.receipt_path(&record.id, recipient))
                {
                    total = total.saturating_add(metadata.len());
                }
            }
        }
        total
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
    fn prune_received(&self, now_ms: i64) -> Result<(), Error> {
        let cutoff = now_ms.saturating_sub(RECEIVED_RETENTION_MS).max(0) as u64;
        for message in self.read_all()? {
            let mut receipts = Vec::with_capacity(message.recipients.len());
            for recipient in &message.recipients {
                match self.read_receipt(&message.id, recipient)? {
                    Some(receipt) => receipts.push(receipt),
                    None => break,
                }
            }
            if receipts.len() != message.recipients.len() {
                continue;
            }
            if receipts
                .iter()
                .all(|receipt| receipt.received_at_unix_ms < cutoff)
            {
                for recipient in &message.recipients {
                    fs::remove_file(self.receipt_path(&message.id, recipient)).map_err(
                        |error| Error::Internal(format!("expire message receipt: {error}")),
                    )?;
                }
                fs::remove_file(self.message_path(&message.id))
                    .map_err(|error| Error::Internal(format!("expire message: {error}")))?;
            }
        }
        Ok(())
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

/// Acceptance order, with the message id as the tie-break, so paging is total.
fn acceptance_order(left: &Message, right: &Message) -> std::cmp::Ordering {
    accept_position(left).cmp(&accept_position(right))
}

fn accept_position(message: &Message) -> (u64, &str) {
    (message.created_at_unix_ms, message.id.as_str())
}

fn format_cursor(message: &Message) -> String {
    format!("{}:{}", message.created_at_unix_ms, message.id)
}

fn parse_cursor(cursor: &str) -> Result<(u64, &str), Error> {
    let (created, id) = cursor
        .split_once(':')
        .ok_or_else(|| Error::Invalid("page cursor is not an acceptance position".into()))?;
    let created = created
        .parse::<u64>()
        .map_err(|_| Error::Invalid("page cursor is not an acceptance position".into()))?;
    if !is_id(id) {
        return Err(Error::Invalid(
            "page cursor is not an acceptance position".into(),
        ));
    }
    Ok((created, id))
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
        assert_eq!(store.pending(P).unwrap().len(), 1);
        assert_eq!(store.pending(Q).unwrap().len(), 1);
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
        assert!(store.pending(P).unwrap().is_empty());
        assert_eq!(store.pending(Q).unwrap().len(), 1);
        assert_eq!(
            store.get(M1, Q).unwrap().0.body,
            message(M1, &[P, Q], "body").body,
            "content stays retrievable after another recipient's receipt"
        );
    }

    #[test]
    fn attention_follows_only_this_recipients_mailbox() {
        let (_state, store) = open();
        let quiet = store.attention(Q).unwrap();
        assert_eq!(
            (quiet.pending, quiet.revision.as_str()),
            (0, quiet.revision.as_str())
        );
        store.publish(&message(M1, &[P], "body")).unwrap();
        let waiting = store.attention(P).unwrap();
        assert_eq!(waiting.pending, 1);
        assert_ne!(waiting.revision, quiet.revision);
        assert_eq!(
            store.attention(Q).unwrap().revision,
            quiet.revision,
            "another subject's delivery is not Q's attention"
        );
        store.receive(M1, &identity(P), 2_000).unwrap();
        assert_eq!(store.attention(P).unwrap().pending, 0);
        assert_ne!(
            store.attention(P).unwrap().revision,
            quiet.revision,
            "receipt is a mailbox change"
        );
    }

    #[test]
    fn a_rebuilt_store_reports_the_same_pending_work_without_an_announcement() {
        let (state, store) = open();
        store.publish(&message(M1, &[P], "body")).unwrap();
        let before = store.attention(P).unwrap();
        drop(store);
        let rebuilt = MessageStore::open(&state).unwrap();
        let after = rebuilt.attention(P).unwrap();
        assert_eq!(after.pending, before.pending);
        assert_ne!(
            after.revision, before.revision,
            "a new instance never reuses an earlier revision"
        );
        assert_eq!(rebuilt.pending(P).unwrap().len(), 1);
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
        assert_eq!(page[0].id, M1);
        assert_eq!(
            next.as_deref(),
            Some("1000:44444444-4444-4444-8444-444444444444")
        );
        let (page, next) = store.page(P, next.as_deref(), 1).unwrap();
        assert_eq!(page[0].id, M2);
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
            store.pending(Q).unwrap().is_empty(),
            "a refused publication delivers to nobody"
        );
        assert_eq!(store.pending(P).unwrap().len(), MAX_PENDING_PER_SUBJECT);
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
}
