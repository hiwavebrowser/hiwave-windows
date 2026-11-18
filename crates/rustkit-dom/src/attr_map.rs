//! An element's attributes, in the order they were added (DOM §4.9: an
//! element has an attribute *list*).
//!
//! The order is observable: `attributes[i]`, `getAttributeNames()`,
//! `outerHTML` and every serializer write it, and Chrome keeps the order
//! the parser saw or script set. Setting a name the element has keeps its
//! place; removing it and setting it again puts it last.
//!
//! A vector of pairs with a linear search by name. Elements have few
//! attributes (the browser's own chrome page averages 1.4, a list-heavy
//! page six), and comparing a handful of short names is no slower than
//! hashing one: see `attr_cost_bench` for the measured parse and lookup
//! times against the `HashMap` this replaced. The method names are that
//! map's, so the callers read as before.
//!
//! Past `NAME_SET_FROM` attributes the names are also kept in a set: a
//! name that is not there is known without the search, so building an
//! element with a hundred thousand attributes (a generated page, or a
//! hostile one) is linear and not a search of the list for each.

/// Attribute names to values, in insertion order.
#[derive(Debug, Clone, Default)]
pub struct AttrMap {
    pairs: Vec<(String, String)>,
    /// Every name in `pairs`, from the first time there are more than
    /// `NAME_SET_FROM` until the map is cleared. Exactly those names:
    /// every change to `pairs` changes it too.
    names: Option<std::collections::HashSet<String>>,
}

const NAME_SET_FROM: usize = 32;

/// The same names with the same values, in any order: what the map this
/// replaced compared, and what DOM `isEqualNode` asks (DOM §4.4).
impl PartialEq for AttrMap {
    fn eq(&self, other: &Self) -> bool {
        self.pairs.len() == other.pairs.len()
            && self.pairs.iter().all(|(name, value)| other.get(name) == Some(value))
    }
}

impl Eq for AttrMap {}

impl AttrMap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self { pairs: Vec::with_capacity(capacity), names: None }
    }

    pub fn len(&self) -> usize {
        self.pairs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.pairs.is_empty()
    }

    /// False only when the name is known not to be here.
    fn may_have(&self, name: &str) -> bool {
        self.names.as_ref().is_none_or(|names| names.contains(name))
    }

    fn position(&self, name: &str) -> Option<usize> {
        if !self.may_have(name) {
            return None;
        }
        self.pairs.iter().position(|(n, _)| n == name)
    }

    pub fn get(&self, name: &str) -> Option<&String> {
        self.position(name).map(|at| &self.pairs[at].1)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut String> {
        self.position(name).map(|at| &mut self.pairs[at].1)
    }

    pub fn contains_key(&self, name: &str) -> bool {
        self.position(name).is_some()
    }

    /// Set `name`: a name already here keeps its place and takes the new
    /// value (the old one is returned); a new name goes last.
    pub fn insert(&mut self, name: String, value: String) -> Option<String> {
        match self.position(&name) {
            Some(at) => Some(std::mem::replace(&mut self.pairs[at].1, value)),
            None => {
                // The set is kept from this size on, and once there is
                // one it is told of every name, however few are left: a
                // name the set does not have is taken as absent.
                if self.names.is_some() || self.pairs.len() >= NAME_SET_FROM {
                    let pairs = &self.pairs;
                    self.names
                        .get_or_insert_with(|| pairs.iter().map(|(n, _)| n.clone()).collect())
                        .insert(name.clone());
                }
                self.pairs.push((name, value));
                None
            }
        }
    }

    /// Remove `name`; the attributes after it keep their order.
    pub fn remove(&mut self, name: &str) -> Option<String> {
        let at = self.position(name)?;
        if let Some(names) = &mut self.names {
            names.remove(name);
        }
        Some(self.pairs.remove(at).1)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&String, &mut String) -> bool) {
        self.pairs.retain_mut(|(n, v)| keep(n, v));
        if self.names.is_some() {
            self.names = Some(self.pairs.iter().map(|(n, _)| n.clone()).collect());
        }
    }

    pub fn clear(&mut self) {
        self.pairs.clear();
        self.names = None;
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&String, &String)> + ExactSizeIterator + Clone {
        self.pairs.iter().map(|(n, v)| (n, v))
    }

    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &String> + ExactSizeIterator + Clone {
        self.pairs.iter().map(|(n, _)| n)
    }

    pub fn values(&self) -> impl DoubleEndedIterator<Item = &String> + ExactSizeIterator + Clone {
        self.pairs.iter().map(|(_, v)| v)
    }
}

/// Collected in order; a name given twice keeps its first place and its
/// last value (the HTML tokenizer has already dropped a tag's duplicates).
impl FromIterator<(String, String)> for AttrMap {
    fn from_iter<I: IntoIterator<Item = (String, String)>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let mut map = Self::with_capacity(iter.size_hint().0);
        for (name, value) in iter {
            map.insert(name, value);
        }
        map
    }
}

impl Extend<(String, String)> for AttrMap {
    fn extend<I: IntoIterator<Item = (String, String)>>(&mut self, iter: I) {
        for (name, value) in iter {
            self.insert(name, value);
        }
    }
}

impl IntoIterator for AttrMap {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<(String, String)>;
    fn into_iter(self) -> Self::IntoIter {
        self.pairs.into_iter()
    }
}

impl<'a> IntoIterator for &'a AttrMap {
    type Item = (&'a String, &'a String);
    type IntoIter = std::iter::Map<std::slice::Iter<'a, (String, String)>, fn(&'a (String, String)) -> (&'a String, &'a String)>;
    fn into_iter(self) -> Self::IntoIter {
        self.pairs.iter().map(|(n, v)| (n, v))
    }
}

/// From a map with no order of its own: by name, so the result is the same
/// every time.
impl From<std::collections::HashMap<String, String>> for AttrMap {
    fn from(map: std::collections::HashMap<String, String>) -> Self {
        let mut pairs: Vec<(String, String)> = map.into_iter().collect();
        pairs.sort();
        let names = (pairs.len() > NAME_SET_FROM).then(|| pairs.iter().map(|(n, _)| n.clone()).collect());
        Self { pairs, names }
    }
}

impl<const N: usize> From<[(String, String); N]> for AttrMap {
    fn from(pairs: [(String, String); N]) -> Self {
        pairs.into_iter().collect()
    }
}

impl std::ops::Index<&str> for AttrMap {
    type Output = String;
    fn index(&self, name: &str) -> &String {
        self.get(name).expect("no such attribute")
    }
}

#[cfg(test)]
mod tests {
    use super::AttrMap;

    /// R1 LOW: building a long list must not be quadratic. 100,000 distinct
    /// names are a 1.5 MB start tag, which a generated or hostile page can
    /// send; scanning the list for each one was 10^10 comparisons.
    #[test]
    fn a_hundred_thousand_attributes_build_in_linear_time() {
        let started = std::time::Instant::now();
        let mut map = AttrMap::new();
        for i in 0..100_000 {
            map.insert(format!("data-a{i}"), String::new());
        }
        assert_eq!(map.len(), 100_000);
        assert_eq!(map.get("data-a99999").map(String::as_str), Some(""));
        assert!(map.get("data-missing").is_none());
        assert_eq!(map.remove("data-a5").as_deref(), Some(""));
        assert!(!map.contains_key("data-a5"));
        assert_eq!(map.keys().next().map(String::as_str), Some("data-a0"));
        let took = started.elapsed();
        assert!(took < std::time::Duration::from_secs(5), "100,000 inserts took {took:?}");
    }

    /// What every lookup rests on: the name set, once there is one, holds
    /// exactly the names of the pairs, and no name is there twice.
    fn assert_consistent(map: &AttrMap, model: &[(String, String)], step: &str) {
        let pairs: Vec<(String, String)> = map.iter().map(|(n, v)| (n.clone(), v.clone())).collect();
        assert_eq!(pairs, model, "{step}: the pairs, in order");
        let mut seen = std::collections::HashSet::new();
        for (name, value) in map.iter() {
            assert!(seen.insert(name.clone()), "{step}: {name} is there twice");
            assert_eq!(map.get(name), Some(value), "{step}: get({name}) finds what keys() lists");
            assert!(map.contains_key(name), "{step}: contains_key({name})");
            assert_eq!(&map[name.as_str()], value, "{step}: index {name}");
        }
        if let Some(names) = &map.names {
            assert_eq!(names, &seen, "{step}: the name set is the pairs' names");
        }
        assert!(map.get("never-set").is_none(), "{step}");
    }

    /// An element that had more than 32 attributes, lost some and gained
    /// one: the new one is found. (Live: an `<svg>` shape of a page with
    /// many attributes, serialized after a paint rule took two of them
    /// away and added `style`; the serializer's index panicked.)
    #[test]
    fn a_name_added_after_the_map_shrank_below_the_set_size_is_found() {
        let mut map = AttrMap::new();
        for i in 0..34 {
            map.insert(format!("data-a{i}"), i.to_string());
        }
        for i in 0..4 {
            assert!(map.remove(&format!("data-a{i}")).is_some());
        }
        assert!(map.insert("style".to_string(), "fill:red".to_string()).is_none());
        assert_eq!(map.get("style").map(String::as_str), Some("fill:red"));
        assert_eq!(&map["style"], "fill:red");
        assert!(map.insert("style".to_string(), "again".to_string()).is_some(), "set again, not added twice");
        assert_eq!(map.len(), 31);
    }

    /// Every way a map changes, in random order, against a plain list:
    /// insert, remove, retain, extend, collect, clone, clear, get_mut.
    #[test]
    fn the_name_set_agrees_with_the_pairs_through_any_sequence_of_changes() {
        // A fixed-seed generator: the same sequences every run.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = move |below: u64| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) % below
        };
        fn set(model: &mut Vec<(String, String)>, name: &str, value: &str) {
            match model.iter_mut().find(|(n, _)| n == name) {
                Some(pair) => pair.1 = value.to_string(),
                None => model.push((name.to_string(), value.to_string())),
            }
        }
        for round in 0..200 {
            let mut map = AttrMap::new();
            let mut model: Vec<(String, String)> = Vec::new();
            for op in 0..400 {
                let name = format!("a{}", next(48));
                let value = format!("v{op}");
                let step = format!("round {round} op {op}");
                match next(100) {
                    0..=44 => {
                        let before = model.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone());
                        assert_eq!(map.insert(name.clone(), value.clone()), before, "{step}: insert");
                        set(&mut model, &name, &value);
                    }
                    45..=79 => {
                        let at = model.iter().position(|(n, _)| *n == name);
                        assert_eq!(map.remove(&name), at.map(|at| model.remove(at).1), "{step}: remove");
                    }
                    80..=84 => {
                        let cut = next(10);
                        map.retain(|n, _| n.as_bytes().last().is_some_and(|b| u64::from(*b - b'0') != cut));
                        model.retain(|(n, _)| n.as_bytes().last().is_some_and(|b| u64::from(*b - b'0') != cut));
                    }
                    85..=89 => {
                        let more: Vec<(String, String)> =
                            (0..next(40)).map(|i| (format!("a{}", next(48)), format!("e{i}"))).collect();
                        for (n, v) in &more {
                            set(&mut model, n, v);
                        }
                        map.extend(more);
                    }
                    90..=93 => {
                        map = map.iter().map(|(n, v)| (n.clone(), v.clone())).collect();
                    }
                    94..=96 => {
                        map = map.clone();
                    }
                    97 => {
                        map.clear();
                        model.clear();
                    }
                    _ => {
                        if let Some(v) = map.get_mut(&name) {
                            v.push('!');
                        }
                        if let Some(pair) = model.iter_mut().find(|(n, _)| *n == name) {
                            pair.1.push('!');
                        }
                    }
                }
                assert_consistent(&map, &model, &step);
            }
        }
    }

    fn names(map: &AttrMap) -> String {
        map.keys().cloned().collect::<Vec<_>>().join(",")
    }

    #[test]
    fn the_order_is_the_order_of_insertion() {
        let mut map: AttrMap =
            [("z", "1"), ("a", "2"), ("m", "3")].into_iter().map(|(n, v)| (n.to_string(), v.to_string())).collect();
        assert_eq!(names(&map), "z,a,m");
        // An existing name keeps its place.
        assert_eq!(map.insert("z".into(), "again".into()).as_deref(), Some("1"));
        assert_eq!(names(&map), "z,a,m");
        assert_eq!(map.get("z").map(String::as_str), Some("again"));
        // Removed and set again: last.
        assert_eq!(map.remove("a").as_deref(), Some("2"));
        assert_eq!(map.insert("a".into(), "back".into()), None);
        assert_eq!(names(&map), "z,m,a");
        assert_eq!(map.len(), 3);
        assert!(map.contains_key("m") && !map.contains_key("q"));
    }
}
