//! RFC 6962 tree proofs and Go's height-eight tile addressing.
//! Protocol reference: golang.org/x/mod/sumdb/tlog (BSD-3-Clause).

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use super::{Hash, Tree, decode_hash, encode_hash};
use crate::libs::RegistryTransport;

pub(super) fn leaf(bytes: &[u8]) -> Hash {
    let mut hash = Sha256::new();
    hash.update([0]);
    hash.update(bytes);
    hash.finalize().into()
}

fn node(left: Hash, right: Hash) -> Hash {
    let mut hash = Sha256::new();
    hash.update([1]);
    hash.update(left);
    hash.update(right);
    hash.finalize().into()
}

fn split(size: u64) -> u64 {
    1 << (63 - (size - 1).leading_zeros())
}

pub(super) fn check_record(
    proof: &[String],
    tree: Tree,
    id: u64,
    bytes: &[u8],
) -> Result<(), String> {
    if id >= tree.size || proof.len() > 63 {
        return Err("sumdb record proof has invalid bounds".into());
    }
    fn root(proof: &[String], size: u64, id: u64, value: Hash) -> Result<Hash, String> {
        if size == 1 {
            return if proof.is_empty() {
                Ok(value)
            } else {
                Err("sumdb inclusion proof has surplus hashes".into())
            };
        }
        let (last, rest) = proof
            .split_last()
            .ok_or("sumdb inclusion proof is incomplete")?;
        let sibling = decode_hash(last)?;
        let left = split(size);
        if id < left {
            Ok(node(root(rest, left, id, value)?, sibling))
        } else {
            Ok(node(sibling, root(rest, size - left, id - left, value)?))
        }
    }
    if root(proof, tree.size, id, leaf(bytes))? != tree.hash {
        return Err("sumdb inclusion proof does not match signed checkpoint".into());
    }
    Ok(())
}

pub(super) fn check_consistency(proof: &[String], older: Tree, newer: Tree) -> Result<(), String> {
    if older.size > newer.size || proof.len() > 63 {
        return Err("sumdb consistency proof has invalid bounds".into());
    }
    if older.size == 0 {
        let empty: Hash = Sha256::digest([]).into();
        return if proof.is_empty() && older.hash == empty {
            Ok(())
        } else {
            Err("sumdb empty checkpoint is invalid".into())
        };
    }
    fn roots(
        proof: &[String],
        lo: u64,
        size: u64,
        old_size: u64,
        old_hash: Hash,
    ) -> Result<(Hash, Hash), String> {
        if old_size == size {
            return if lo == 0 && proof.is_empty() {
                Ok((old_hash, old_hash))
            } else if lo != 0 && proof.len() == 1 {
                let hash = decode_hash(&proof[0])?;
                Ok((hash, hash))
            } else {
                Err("sumdb consistency proof has surplus or missing hashes".into())
            };
        }
        let (last, rest) = proof
            .split_last()
            .ok_or("sumdb consistency proof is incomplete")?;
        let sibling = decode_hash(last)?;
        let left = split(size);
        if old_size <= left {
            let (old, new) = roots(rest, lo, left, old_size, old_hash)?;
            Ok((old, node(new, sibling)))
        } else {
            let (old, new) = roots(rest, lo + left, size - left, old_size - left, old_hash)?;
            Ok((node(sibling, old), node(sibling, new)))
        }
    }
    let (old, new) = roots(proof, 0, newer.size, older.size, older.hash)?;
    if old != older.hash || new != newer.hash {
        return Err("sumdb consistency proof detects a conflicting history".into());
    }
    Ok(())
}

/// Fetched tiles are only inputs to proofs; no byte is trusted merely because
/// it is cached. Persisted Evidence contains only completely verified proofs.
pub(super) struct Tiles<'a> {
    transport: &'a dyn RegistryTransport,
    cache: BTreeMap<String, Vec<u8>>,
}

impl<'a> Tiles<'a> {
    pub(super) fn new(transport: &'a dyn RegistryTransport) -> Self {
        Self {
            transport,
            cache: BTreeMap::new(),
        }
    }

    fn subtree(&mut self, lo: u64, size: u64, tree_size: u64) -> Result<Hash, String> {
        if size.is_power_of_two() {
            let level = size.trailing_zeros();
            let tile_level = level / 8;
            let tile_scale = tile_level * 8;
            let count = 1u64 << (level % 8);
            let first = lo >> tile_scale;
            let index = first / 256;
            let offset = first % 256;
            let width = (tree_size >> tile_scale)
                .checked_sub(index * 256)
                .ok_or("sumdb tile lies outside checkpoint")?
                .min(256);
            if offset + count > width {
                return Err("sumdb tile does not cover requested subtree".into());
            }
            let mut chunks = vec![format!("{:03}", index % 1000)];
            let mut prefix = index / 1000;
            while prefix > 0 {
                chunks.push(format!("x{:03}", prefix % 1000));
                prefix /= 1000;
            }
            chunks.reverse();
            let full = format!("/tile/8/{tile_level}/{}", chunks.join("/"));
            let path = if width == 256 {
                full.clone()
            } else {
                format!("{full}.p/{width}")
            };
            if !self.cache.contains_key(&path) {
                if self.cache.len() >= 4096 {
                    return Err("sumdb tile cache exceeds its 32 MiB budget".into());
                }
                let bytes = match self.transport.get_sumdb(&path, 8192) {
                    Ok(bytes) => bytes,
                    Err(_) if width < 256 => {
                        let mut bytes = self.transport.get_sumdb(&full, 8192)?;
                        if bytes.len() != 8192 {
                            return Err("sumdb full tile has wrong length".into());
                        }
                        bytes.truncate(width as usize * 32);
                        bytes
                    }
                    Err(error) => return Err(error),
                };
                if bytes.len() != width as usize * 32 {
                    return Err("sumdb tile has wrong length".into());
                }
                self.cache.insert(path.clone(), bytes);
            }
            fn fold(bytes: &[u8]) -> Hash {
                if bytes.len() == 32 {
                    return bytes.try_into().expect("one tile hash");
                }
                let (left, right) = bytes.split_at(bytes.len() / 2);
                node(fold(left), fold(right))
            }
            return Ok(fold(
                &self.cache[&path][offset as usize * 32..(offset + count) as usize * 32],
            ));
        }
        let left = split(size);
        Ok(node(
            self.subtree(lo, left, tree_size)?,
            self.subtree(lo + left, size - left, tree_size)?,
        ))
    }

    pub(super) fn inclusion(&mut self, size: u64, id: u64) -> Result<Vec<String>, String> {
        fn walk(
            tiles: &mut Tiles<'_>,
            lo: u64,
            size: u64,
            id: u64,
            total: u64,
            proof: &mut Vec<String>,
        ) -> Result<(), String> {
            if size == 1 {
                return Ok(());
            }
            let left = split(size);
            let sibling = if id < lo + left {
                walk(tiles, lo, left, id, total, proof)?;
                tiles.subtree(lo + left, size - left, total)?
            } else {
                walk(tiles, lo + left, size - left, id, total, proof)?;
                tiles.subtree(lo, left, total)?
            };
            proof.push(encode_hash(sibling));
            Ok(())
        }
        if id >= size {
            return Err("sumdb record lies outside signed checkpoint".into());
        }
        let mut proof = Vec::new();
        walk(self, 0, size, id, size, &mut proof)?;
        Ok(proof)
    }

    pub(super) fn consistency(&mut self, older: Tree, newer: Tree) -> Result<Vec<String>, String> {
        fn walk(
            tiles: &mut Tiles<'_>,
            lo: u64,
            size: u64,
            old_size: u64,
            total: u64,
            proof: &mut Vec<String>,
        ) -> Result<(), String> {
            if old_size == size {
                if lo != 0 {
                    proof.push(encode_hash(tiles.subtree(lo, size, total)?));
                }
                return Ok(());
            }
            let left = split(size);
            let sibling = if old_size <= left {
                walk(tiles, lo, left, old_size, total, proof)?;
                tiles.subtree(lo + left, size - left, total)?
            } else {
                walk(tiles, lo + left, size - left, old_size - left, total, proof)?;
                tiles.subtree(lo, left, total)?
            };
            proof.push(encode_hash(sibling));
            Ok(())
        }
        let mut proof = Vec::new();
        if older.size != 0 {
            walk(self, 0, newer.size, older.size, newer.size, &mut proof)?;
        }
        check_consistency(&proof, older, newer)?;
        Ok(proof)
    }
}
