//! Authenticated Go checksum database receipts. Trust comes from the verifier
//! supplied by the caller, never from a key stored in a project's lock file.
//! Formats follow golang.org/x/mod/sumdb/{note,tlog,dirhash} and RFC 6962.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, MutexGuard};

use base64::{Engine, engine::general_purpose::STANDARD};
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{RegistryTransport, go_path, zip_entries};

mod merkle;

pub const OFFICIAL_VERIFIER: &str =
    "sum.golang.org+033de0ae+Ac4zctda0e5eza+HJyk9SxEdh+s3Ux18htTTAD8OuAn8";
const MAX_NOTE_BYTES: usize = 16 * 1024;
const MAX_RECORDS: usize = 1024;
const MAX_EVIDENCE_BYTES: usize = 16 * 1024 * 1024;
type Hash = [u8; 32];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Tree {
    size: u64,
    hash: Hash,
}

fn encode_hash(hash: Hash) -> String {
    STANDARD.encode(hash)
}

fn decode_hash(value: &str) -> Result<Hash, String> {
    STANDARD
        .decode(value)
        .map_err(|_| "sumdb hash is not base64")?
        .try_into()
        .map_err(|_| "sumdb hash must contain 32 bytes".into())
}

/// Explicit verifier injection supports registry protocol tests and author
/// tooling. Released project commands always select `official()`.
#[derive(Clone)]
pub struct Verifier {
    name: String,
    key_hash: [u8; 4],
    key: [u8; 32],
}

impl Verifier {
    pub fn official() -> Self {
        Self::new(OFFICIAL_VERIFIER).expect("fixed official sumdb verifier")
    }

    pub fn new(encoded: &str) -> Result<Self, String> {
        // '+' also occurs inside base64; only the first two separators delimit.
        let (name, rest) = encoded.split_once('+').ok_or("invalid sumdb verifier")?;
        let (key_hash, encoded_key) = rest.split_once('+').ok_or("invalid sumdb verifier")?;
        if name.is_empty() || name.chars().any(char::is_whitespace) || key_hash.len() != 8 {
            return Err("invalid sumdb verifier identity".into());
        }
        let expected = u32::from_str_radix(key_hash, 16)
            .map_err(|_| "invalid sumdb key hash")?
            .to_be_bytes();
        let key = STANDARD
            .decode(encoded_key)
            .map_err(|_| "invalid sumdb verifier encoding")?;
        if key.len() != 33 || key[0] != 1 {
            return Err("sumdb requires an Ed25519 verifier".into());
        }
        let mut hash = Sha256::new();
        hash.update(name.as_bytes());
        hash.update(b"\n");
        hash.update(&key);
        if hash.finalize()[..4] != expected {
            return Err("sumdb verifier key hash mismatch".into());
        }
        Ok(Self {
            name: name.into(),
            key_hash: expected,
            key: key[1..].try_into().expect("validated Ed25519 key length"),
        })
    }

    fn checkpoint(&self, note: &str) -> Result<Tree, String> {
        if note.len() > MAX_NOTE_BYTES || note.bytes().any(|byte| byte < 32 && byte != b'\n') {
            return Err("invalid sumdb checkpoint note".into());
        }
        let (text, signatures) = note
            .rsplit_once("\n\n")
            .ok_or("sumdb checkpoint lacks signatures")?;
        let message = format!("{text}\n");
        if !signatures.ends_with('\n') {
            return Err("unterminated sumdb signature".into());
        }
        let mut verified = false;
        for (index, line) in signatures.lines().enumerate() {
            if index >= 100 {
                return Err("sumdb checkpoint has too many signatures".into());
            }
            let (name, encoded) = line
                .strip_prefix("— ")
                .and_then(|line| line.split_once(' '))
                .ok_or("malformed sumdb signature")?;
            let signature = STANDARD
                .decode(encoded)
                .map_err(|_| "invalid sumdb signature encoding")?;
            if name.is_empty() || name.chars().any(char::is_whitespace) || signature.len() < 5 {
                return Err("malformed sumdb signature identity".into());
            }
            if name == self.name && signature[..4] == self.key_hash {
                UnparsedPublicKey::new(&ED25519, self.key)
                    .verify(message.as_bytes(), &signature[4..])
                    .map_err(|_| "sumdb checkpoint signature verification failed")?;
                verified = true;
            }
        }
        if !verified {
            return Err("sumdb checkpoint has no trusted signature".into());
        }
        let mut lines = text.lines();
        if lines.next() != Some("go.sum database tree") {
            return Err("unsupported sumdb checkpoint format".into());
        }
        let size_text = lines.next().ok_or("sumdb checkpoint lacks tree size")?;
        let size = decimal(size_text)?;
        let hash = decode_hash(lines.next().ok_or("sumdb checkpoint lacks root hash")?)?;
        let empty: Hash = Sha256::digest([]).into();
        if size == 0 && hash != empty {
            return Err("invalid sumdb empty root".into());
        }
        Ok(Tree { size, hash })
    }
}

fn decimal(value: &str) -> Result<u64, String> {
    let number = value
        .parse::<u64>()
        .map_err(|_| "invalid sumdb record/tree number")?;
    if number > i64::MAX as u64 || value != number.to_string() {
        return Err("noncanonical sumdb record/tree number".into());
    }
    Ok(number)
}

struct Lookup<'a> {
    id: u64,
    text: &'a str,
    checkpoint: &'a str,
}

fn lookup(value: &str) -> Result<Lookup<'_>, String> {
    if value.len() > MAX_NOTE_BYTES || value.bytes().any(|byte| byte < 32 && byte != b'\n') {
        return Err("invalid sumdb lookup text".into());
    }
    let (id, rest) = value
        .split_once('\n')
        .ok_or("sumdb lookup lacks record number")?;
    let end = rest
        .find("\n\n")
        .ok_or("sumdb lookup lacks record terminator")?;
    Ok(Lookup {
        id: decimal(id)?,
        text: &rest[..end + 1],
        checkpoint: &rest[end + 2..],
    })
}

fn hashes(record: &str, module: &str, version: &str) -> Result<(String, String), String> {
    let version = format!("v{version}");
    let mod_version = format!("{version}/go.mod");
    let mut values = BTreeMap::new();
    for line in record.lines() {
        let parts = line.split(' ').collect::<Vec<_>>();
        if parts.len() != 3
            || parts[0] != module
            || (parts[1] != version && parts[1] != mod_version)
        {
            return Err("sumdb lookup record has a different module/version identity".into());
        }
        decode_hash(
            parts[2]
                .strip_prefix("h1:")
                .ok_or("sumdb record requires h1 hashes")?,
        )?;
        if values.insert(parts[1], parts[2]).is_some() {
            return Err("sumdb lookup repeats a module checksum".into());
        }
    }
    Ok((
        values
            .get(mod_version.as_str())
            .ok_or("sumdb lookup lacks go.mod hash")?
            .to_string(),
        values
            .get(version.as_str())
            .ok_or("sumdb lookup lacks module ZIP hash")?
            .to_string(),
    ))
}

fn dirhash(files: &[(String, Vec<u8>)]) -> Result<String, String> {
    let mut files = files.iter().collect::<Vec<_>>();
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut seen = BTreeSet::new();
    let mut hash = Sha256::new();
    for (name, bytes) in files {
        if name.contains('\n') || !seen.insert(name) {
            return Err("invalid or duplicate Go dirhash filename".into());
        }
        hash.update(format!("{}  {name}\n", crate::libs::sha256(bytes)));
    }
    Ok(format!("h1:{}", STANDARD.encode(hash.finalize())))
}

pub fn mod_hash(bytes: &[u8]) -> String {
    // The Go protocol hashes a synthetic directory containing only go.mod.
    dirhash(&[("go.mod".into(), bytes.to_vec())]).expect("fixed dirhash filename")
}

pub fn zip_hash(module: &str, version: &str, bytes: &[u8]) -> Result<String, String> {
    let files = zip_entries(bytes)?;
    let prefix = format!("{module}@v{version}/");
    if files.iter().any(|(path, _)| !path.starts_with(&prefix)) {
        return Err("Go module archive has a mismatched module root".into());
    }
    dirhash(&files)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub module: String,
    pub version: String,
    pub go_mod: String,
    pub lookup: String,
    pub inclusion: Vec<String>,
    pub consistency: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Evidence {
    pub previous: Option<String>,
    pub checkpoint: String,
    pub consistency: Vec<String>,
    pub records: Vec<Record>,
}

#[derive(Debug)]
pub struct Verified {
    pub checkpoint: String,
    checksums: BTreeMap<(String, String), String>,
}

impl Verified {
    pub fn verify_zip(&self, module: &str, version: &str, bytes: &[u8]) -> Result<(), String> {
        let expected = self
            .checksums
            .get(&(module.into(), version.into()))
            .ok_or("Go module lacks authenticated sumdb evidence")?;
        if zip_hash(module, version, bytes)? != *expected {
            return Err("Go module ZIP does not match authenticated sumdb h1".into());
        }
        Ok(())
    }

    pub fn contains(&self, module: &str, version: &str) -> bool {
        self.checksums
            .contains_key(&(module.into(), version.into()))
    }
}

impl Evidence {
    pub fn verify(&self, verifier: &Verifier) -> Result<Verified, String> {
        if self.records.len() > MAX_RECORDS
            || serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_EVIDENCE_BYTES
        {
            return Err("sumdb evidence exceeds its size budget".into());
        }
        let mut current = verifier.checkpoint(&self.checkpoint)?;
        if let Some(previous) = &self.previous {
            let older = verifier.checkpoint(previous)?;
            if older.size > current.size {
                return Err("sumdb checkpoint rollback rejected".into());
            }
            merkle::check_consistency(&self.consistency, older, current)?;
        } else if !self.consistency.is_empty() {
            return Err("sumdb first checkpoint has an unexpected consistency proof".into());
        }
        let mut latest = self.checkpoint.clone();
        let mut checksums = BTreeMap::new();
        for record in &self.records {
            let entry = lookup(&record.lookup)?;
            let tree = verifier.checkpoint(entry.checkpoint)?;
            merkle::check_record(&record.inclusion, tree, entry.id, entry.text.as_bytes())?;
            let (older, newer) = if tree.size < current.size {
                (tree, current)
            } else {
                (current, tree)
            };
            merkle::check_consistency(&record.consistency, older, newer)?;
            let (mod_h1, zip_h1) = hashes(entry.text, &record.module, &record.version)?;
            if mod_hash(record.go_mod.as_bytes()) != mod_h1 {
                return Err("Go go.mod does not match authenticated sumdb h1".into());
            }
            if checksums
                .insert((record.module.clone(), record.version.clone()), zip_h1)
                .is_some()
            {
                return Err("sumdb evidence repeats a module/version".into());
            }
            if tree.size > current.size {
                current = tree;
                latest = entry.checkpoint.into();
            }
        }
        Ok(Verified {
            checkpoint: latest,
            checksums,
        })
    }
}

/// One explicit resolution sequence owns the checkpoint. The project lock is
/// its persistent anchor; deleting that lock intentionally starts first trust.
pub struct ChecksumDatabase {
    verifier: Verifier,
    latest: Mutex<Option<String>>,
}

impl ChecksumDatabase {
    pub fn verifier(&self) -> &Verifier {
        &self.verifier
    }

    pub fn new(verifier: Verifier, previous: Option<String>) -> Result<Self, String> {
        if let Some(note) = &previous {
            verifier.checkpoint(note)?;
        }
        Ok(Self {
            verifier,
            latest: Mutex::new(previous),
        })
    }

    pub fn official(previous: Option<String>) -> Result<Self, String> {
        Self::new(Verifier::official(), previous)
    }

    pub(super) fn begin<'a>(
        &'a self,
        transport: &'a dyn RegistryTransport,
    ) -> Result<Session<'a>, String> {
        let guard = self
            .latest
            .lock()
            .map_err(|_| "sumdb checkpoint state poisoned")?;
        let checkpoint = String::from_utf8(transport.get_sumdb("/latest", MAX_NOTE_BYTES)?)
            .map_err(|_| "sumdb checkpoint is not UTF-8")?;
        let current = self.verifier.checkpoint(&checkpoint)?;
        let mut tiles = merkle::Tiles::new(transport);
        let consistency = if let Some(previous) = &*guard {
            let older = self.verifier.checkpoint(previous)?;
            if older.size > current.size {
                return Err("sumdb checkpoint rollback rejected".into());
            }
            tiles.consistency(older, current)?
        } else {
            Vec::new()
        };
        let evidence = Evidence {
            previous: guard.clone(),
            checkpoint: checkpoint.clone(),
            consistency,
            records: Vec::new(),
        };
        Ok(Session {
            verifier: &self.verifier,
            transport,
            guard,
            tiles,
            current,
            latest: checkpoint,
            evidence,
        })
    }
}

pub(super) struct Session<'a> {
    verifier: &'a Verifier,
    transport: &'a dyn RegistryTransport,
    guard: MutexGuard<'a, Option<String>>,
    tiles: merkle::Tiles<'a>,
    current: Tree,
    latest: String,
    evidence: Evidence,
}

impl Session<'_> {
    pub(super) fn verify_mod(
        &mut self,
        module: &str,
        version: &str,
        source: &[u8],
    ) -> Result<(), String> {
        if let Some(record) = self
            .evidence
            .records
            .iter()
            .find(|record| record.module == module && record.version == version)
        {
            if record.go_mod.as_bytes() != source {
                return Err("Go go.mod changed during resolution".into());
            }
            return Ok(());
        }
        if self.evidence.records.len() >= MAX_RECORDS {
            return Err("sumdb resolution exceeds record limit".into());
        }
        let path = format!("/lookup/{}@v{}", go_path(module), go_path(version));
        let response = String::from_utf8(self.transport.get_sumdb(&path, MAX_NOTE_BYTES)?)
            .map_err(|_| "sumdb lookup is not UTF-8")?;
        let entry = lookup(&response)?;
        let tree = self.verifier.checkpoint(entry.checkpoint)?;
        let inclusion = self.tiles.inclusion(tree.size, entry.id)?;
        merkle::check_record(&inclusion, tree, entry.id, entry.text.as_bytes())?;
        let (older, newer) = if tree.size < self.current.size {
            (tree, self.current)
        } else {
            (self.current, tree)
        };
        let consistency = self.tiles.consistency(older, newer)?;
        let (expected, _) = hashes(entry.text, module, version)?;
        if mod_hash(source) != expected {
            return Err("Go go.mod does not match authenticated sumdb h1".into());
        }
        if tree.size > self.current.size {
            self.current = tree;
            self.latest = entry.checkpoint.into();
        }
        let record = Record {
            module: module.into(),
            version: version.into(),
            go_mod: String::from_utf8(source.to_vec()).map_err(|_| "Go go.mod is not UTF-8")?,
            lookup: response,
            inclusion,
            consistency,
        };
        self.evidence.records.push(record);
        if serde_json::to_vec(&self.evidence)
            .map_err(|e| e.to_string())?
            .len()
            > MAX_EVIDENCE_BYTES
        {
            return Err("sumdb evidence exceeds its size budget".into());
        }
        Ok(())
    }

    pub(super) fn verify_zip(
        &self,
        module: &str,
        version: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        let record = self
            .evidence
            .records
            .iter()
            .find(|record| record.module == module && record.version == version)
            .ok_or("Go module ZIP has no authenticated metadata")?;
        let (_, expected) = hashes(lookup(&record.lookup)?.text, module, version)?;
        if zip_hash(module, version, bytes)? != expected {
            return Err("Go module ZIP does not match authenticated sumdb h1".into());
        }
        Ok(())
    }

    pub(super) fn finish(mut self) -> Evidence {
        *self.guard = Some(self.latest);
        self.evidence
    }
}

pub fn verify_chain(evidence: &[Evidence], verifier: &Verifier) -> Result<Verified, String> {
    let mut latest = None;
    let mut checksums = BTreeMap::new();
    if evidence.len() > MAX_RECORDS {
        return Err("sumdb resolution history exceeds its limit".into());
    }
    let mut bytes = 0usize;
    for receipt in evidence {
        bytes = bytes
            .checked_add(
                serde_json::to_vec(receipt)
                    .map_err(|error| error.to_string())?
                    .len(),
            )
            .ok_or("sumdb resolution history size overflow")?;
        if bytes > MAX_EVIDENCE_BYTES {
            return Err("sumdb resolution history exceeds its size budget".into());
        }
        if let Some(previous) = &latest
            && receipt.previous.as_ref() != Some(previous)
        {
            return Err("sumdb resolution checkpoints are not connected".into());
        }
        let verified = receipt.verify(verifier)?;
        for (identity, hash) in verified.checksums {
            if let Some(old) = checksums.insert(identity, hash.clone())
                && old != hash
            {
                return Err("sumdb resolutions disagree about module content".into());
            }
        }
        latest = Some(verified.checkpoint);
    }
    Ok(Verified {
        checkpoint: latest.unwrap_or_default(),
        checksums,
    })
}
