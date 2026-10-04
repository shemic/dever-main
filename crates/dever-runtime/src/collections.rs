use std::hash::Hash;
use std::sync::Arc;

use indexmap::IndexMap;

#[derive(Clone, Debug)]
pub struct List<T>(Arc<Vec<T>>);

impl<T> List<T> {
    pub fn new(values: Vec<T>) -> Self {
        Self(Arc::new(values))
    }
    pub fn values(&self) -> &[T] {
        &self.0
    }
}

impl<T: Clone> List<T> {
    pub fn into_first(self) -> Option<T> {
        match Arc::try_unwrap(self.0) {
            Ok(values) => values.into_iter().next(),
            Err(values) => values.first().cloned(),
        }
    }

    pub fn into_values(self) -> IntoValues<T> {
        match Arc::try_unwrap(self.0) {
            Ok(values) => IntoValues::Owned(values.into_iter()),
            Err(values) => IntoValues::Shared { values, index: 0 },
        }
    }

    pub fn append(mut self, value: T) -> Self {
        Arc::make_mut(&mut self.0).push(value);
        self
    }
}

/// 独占列表转移元素；共享列表按需复制，提前停止不复制剩余元素。
pub enum IntoValues<T> {
    Owned(std::vec::IntoIter<T>),
    Shared { values: Arc<Vec<T>>, index: usize },
}

impl<T: Clone> Iterator for IntoValues<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        match self {
            Self::Owned(values) => values.next(),
            Self::Shared { values, index } => {
                let value = values.get(*index)?.clone();
                *index += 1;
                Some(value)
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = match self {
            Self::Owned(values) => values.len(),
            Self::Shared { values, index } => values.len() - index,
        };
        (remaining, Some(remaining))
    }
}

impl<T: Clone> ExactSizeIterator for IntoValues<T> {}

impl<T: PartialEq> PartialEq for List<T> {
    fn eq(&self, other: &Self) -> bool {
        self.values().len() == other.values().len()
            && self
                .values()
                .iter()
                .zip(other.values())
                .all(|(left, right)| left == right)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapEntry<K, V> {
    pub key: K,
    pub value: V,
}

#[derive(Clone, Debug)]
pub struct Map<K, V>(Arc<IndexMap<K, V>>);

impl<K: Clone + Eq + Hash, V: Clone> Map<K, V> {
    pub fn new(entries: Vec<(K, V)>) -> Result<Self, &'static str> {
        let mut map = IndexMap::with_capacity(entries.len());
        for (key, value) in entries {
            if map.insert(key, value).is_some() {
                return Err("duplicate Map key");
            }
        }
        Ok(Self(Arc::new(map)))
    }

    pub fn from_unique(entries: Vec<(K, V)>) -> Self {
        let mut map = IndexMap::with_capacity(entries.len());
        for (key, value) in entries {
            debug_assert!(map.insert(key, value).is_none());
        }
        Self(Arc::new(map))
    }

    pub fn get(&self, key: &K) -> Option<V> {
        self.0.get(key).cloned()
    }

    pub fn into_get(self, key: &K) -> Option<V> {
        match Arc::try_unwrap(self.0) {
            Ok(mut entries) => entries.swap_remove(key),
            Err(entries) => entries.get(key).cloned(),
        }
    }

    // 将小型 COW 包装内联，使调用方的循环能一起优化所有权检查与插入。
    #[inline]
    pub fn put(mut self, key: K, value: V) -> Self {
        Arc::make_mut(&mut self.0).insert(key, value);
        self
    }

    pub fn remove(mut self, key: &K) -> Self {
        Arc::make_mut(&mut self.0).shift_remove(key);
        self
    }

    pub fn entries(&self) -> List<MapEntry<K, V>> {
        cloned_entries(&self.0)
    }

    pub fn into_entries(self) -> List<MapEntry<K, V>> {
        match Arc::try_unwrap(self.0) {
            Ok(entries) => List::new(
                entries
                    .into_iter()
                    .map(|(key, value)| MapEntry { key, value })
                    .collect(),
            ),
            Err(entries) => cloned_entries(&entries),
        }
    }
}

fn cloned_entries<K: Clone, V: Clone>(entries: &IndexMap<K, V>) -> List<MapEntry<K, V>> {
    List::new(
        entries
            .iter()
            .map(|(key, value)| MapEntry {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
    )
}

impl<K, V> Map<K, V> {
    pub fn pairs(&self) -> impl Iterator<Item = (&K, &V)> {
        self.0.iter()
    }
}

impl<K: PartialEq, V: PartialEq> PartialEq for Map<K, V> {
    fn eq(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self
                .pairs()
                .zip(other.pairs())
                .all(|((ak, av), (bk, bv))| ak == bk && av == bv)
    }
}
