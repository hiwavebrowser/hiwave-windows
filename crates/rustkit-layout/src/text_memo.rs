//! The bounded per-thread memos behind text measurement
//! (`measure_text_with_spacing`) and shaping (`TextShaper::shape`).
//!
//! Each memo has a budget: entries for the measurement memo, bytes for the
//! shaped-text memo (SHAPED-CAP: each entry weighs its key, its shaped run
//! and its bookkeeping). The clock gets 15/16 of the budget, the queue of
//! turned-away strings (below) 1/16, so the memo never holds more than its
//! budget. While the clock has room a miss is simply kept. When it is full,
//! a miss is admitted only in place of entries that have not been used since
//! the clock hand last passed them (CLOCK, second chance): the hand looks at
//! ONE slot per miss; a slot used since the last look gets its mark cleared
//! and the new string is not kept in the clock; an unused one is evicted, and
//! so are the unused slots right after it while the new string still does
//! not fit (entries differ in size); the hand stops at the first used one.
//!
//! A string the clock turns away goes to a small FIFO of recent strings.
//! Layout asks for the same string several times in a row (the forced-layout
//! fixtures: about ten lookups per text per pass); without the FIFO each of
//! those lookups missed again and moved the hand again, so the hand lapped
//! the clock within a pass and evicted the entries it was meant to keep. A
//! string served from the FIFO moves into the clock when there is room or
//! when the slot under the hand is unused, without moving the hand over used
//! slots, so a page that keeps re-reading turned-away strings gets them into
//! the main table and a cyclic scan of held strings is not disturbed.
//!
//! Why not clear when full (the old rule): a layout pass over more distinct
//! strings than the cap filled the memo, wiped it, refilled it, so the next
//! pass found almost nothing and re-shaped every string (forced layout per
//! element went from 5.8 to 19.9 us on a Mac between 16,300 and 16,500 text
//! rows, TEXT-CACHE-STEP). Why not LRU: a pass is a cyclic scan, and any
//! least-recently-used order misses every string of a scan longer than its
//! capacity. Here the strings already held keep being hit and are kept, so a
//! pass over `n` distinct strings hits the `cap` it holds and shapes about
//! `n - cap`, for `n` up to about twice the cap (the hand needs a whole pass
//! to come round; a pass with more than `cap` distinct misses laps it and the
//! hits fall away). A relayout shapes each row under at least two keys
//! (layout, display list), which is why the shaped memo is bounded by bytes
//! large enough for long pages rather than by 16,384 entries.
//!
//! A page whose strings all differ from the ones held (a navigation after a
//! text-heavy page) refills it within about one lap: the first lap clears the
//! old marks, the second replaces them.

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::rc::Rc;

/// Bytes of bookkeeping one entry of a `TextMemo<K, V>` costs on top of its
/// key and value: the map slot and its control byte, the clock ring slot and
/// the key's `Rc` counts. Table and ring growth slack (both double) is not
/// counted.
pub(crate) fn bookkeeping_bytes<K, V>() -> usize {
    std::mem::size_of::<(Rc<K>, Entry<V>)>()
        + 1
        + std::mem::size_of::<Option<Rc<K>>>()
        + 2 * std::mem::size_of::<usize>()
}

/// A bounded memo with CLOCK eviction (see the module documentation).
pub(crate) struct TextMemo<K, V> {
    entries: HashMap<Rc<K>, Entry<V>>,
    /// The keys in clock order; a slot is `None` after its entry was evicted
    /// to make room for a larger one (reused through `free`).
    ring: Vec<Option<Rc<K>>>,
    free: Vec<usize>,
    hand: usize,
    /// What the clock may hold, in the units of `weigh`, and what it holds.
    budget: usize,
    used: usize,
    /// The last strings the clock turned away, kept for their next few
    /// lookups (oldest dropped first; see the module documentation).
    /// `recent_order` may still name a string that left `recent` (promoted
    /// into the clock); such names are skipped when they come up.
    recent: HashMap<Rc<K>, (V, usize)>,
    recent_order: VecDeque<Rc<K>>,
    recent_budget: usize,
    recent_used: usize,
    /// The weight of one entry: 1 for an entry-counted memo, its bytes for
    /// a byte-bounded one.
    weigh: fn(&K, &V) -> usize,
    /// The `@font-face` set generation the entries were made under.
    generation: u64,
}

pub(crate) struct Entry<V> {
    value: V,
    /// Used since the hand last passed this slot.
    referenced: bool,
    /// This entry's weight (`weigh`).
    weight: usize,
}

impl<K: Hash + Eq, V: Clone> TextMemo<K, V> {
    /// A memo of at most `cap` entries in the clock plus `cap / 16` recently
    /// turned-away ones.
    pub(crate) fn new(cap: usize) -> Self {
        assert!(cap > 0);
        Self::with_budget(cap, (cap / 16).max(1), |_, _| 1)
    }

    /// A memo whose entries weigh `weigh(key, value)` (bytes) and which holds
    /// at most `total` of them in all: 15/16 in the clock, 1/16 in the queue
    /// of turned-away strings.
    pub(crate) fn with_byte_budget(total: usize, weigh: fn(&K, &V) -> usize) -> Self {
        let recent = (total / 16).max(1);
        assert!(total > recent);
        Self::with_budget(total - recent, recent, weigh)
    }

    fn with_budget(budget: usize, recent_budget: usize, weigh: fn(&K, &V) -> usize) -> Self {
        Self {
            entries: HashMap::new(),
            ring: Vec::new(),
            free: Vec::new(),
            hand: 0,
            budget,
            used: 0,
            recent: HashMap::new(),
            recent_order: VecDeque::new(),
            recent_budget,
            recent_used: 0,
            weigh,
            generation: 0,
        }
    }

    /// Drop every entry when the installed `@font-face` set has changed
    /// since they were made: a family name can measure differently then.
    pub(crate) fn sync_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.entries.clear();
            self.ring.clear();
            self.free.clear();
            self.hand = 0;
            self.used = 0;
            self.recent.clear();
            self.recent_order.clear();
            self.recent_used = 0;
            self.generation = generation;
        }
    }

    /// What the memo holds, in its budget's units (bytes for a byte-bounded
    /// memo): the clock and the queue together.
    #[cfg(test)]
    pub(crate) fn weight(&self) -> usize {
        self.used + self.recent_used
    }

    pub(crate) fn get(&mut self, key: &K) -> Option<V> {
        if memos_off() {
            return None;
        }
        if let Some(entry) = self.entries.get_mut(key) {
            entry.referenced = true;
            return Some(entry.value.clone());
        }
        let (value, weight) = self.recent.get(key)?;
        let (value, weight) = (value.clone(), *weight);
        // Promote it when that costs no used entry its place.
        if self.used + weight <= self.budget || self.slot_under_hand_is_unused() {
            let (key, (v, w)) = self.recent.remove_entry(key).expect("just found");
            self.recent_used -= w;
            if let Err((key, v)) = self.admit(key, v, w, false) {
                self.recent_used += w;
                self.recent.insert(key, (v, w));
            }
        }
        Some(value)
    }

    /// Keep `value` for `key` if there is room, or in place of entries not
    /// used since the hand last passed them (see the module documentation);
    /// otherwise in the queue of turned-away strings.
    pub(crate) fn insert(&mut self, key: K, value: V) {
        if memos_off() {
            return;
        }
        let weight = (self.weigh)(&key, &value);
        if let Some(entry) = self.entries.get_mut(&key) {
            self.used -= entry.weight;
            if self.used + weight <= self.budget {
                entry.value = value;
                entry.referenced = true;
                entry.weight = weight;
                self.used += weight;
                return;
            }
            // Grew past the budget: let it go through the clock again.
            let (old, _) = self.entries.remove_entry(&key).expect("just found");
            let slot = self
                .ring
                .iter()
                .position(|k| k.as_ref().is_some_and(|k| Rc::ptr_eq(k, &old)))
                .expect("every entry has a ring slot");
            self.ring[slot] = None;
            self.free.push(slot);
        }
        if let Some((_, (_, w))) = self.recent.remove_entry(&key) {
            self.recent_used -= w;
        }
        if let Err((key, value)) = self.admit(Rc::new(key), value, weight, true) {
            self.turn_away(key, value, weight);
        }
    }

    /// The next occupied ring slot at or after the hand (the hand moves to
    /// it). Only called while the clock holds something.
    fn next_occupied(&mut self) -> usize {
        let n = self.ring.len();
        while self.ring[self.hand].is_none() {
            self.hand = (self.hand + 1) % n;
        }
        self.hand
    }

    fn slot_under_hand_is_unused(&mut self) -> bool {
        if self.used == 0 {
            return false;
        }
        let slot = self.next_occupied();
        let key = self.ring[slot].as_ref().expect("occupied");
        !self.entries[&**key].referenced
    }

    /// Put `key` into the clock, evicting unused entries at the hand while it
    /// does not fit. On a miss (`clear_mark`) a used slot met first has its
    /// mark cleared (its second chance); a promotion never clears a mark.
    /// Gives the string back when it was not kept.
    fn admit(
        &mut self,
        key: Rc<K>,
        value: V,
        weight: usize,
        clear_mark: bool,
    ) -> Result<(), (Rc<K>, V)> {
        if weight > self.budget {
            return Err((key, value));
        }
        let mut first = true;
        while self.used + weight > self.budget {
            // used > 0 here, so the ring has an occupied slot.
            let slot = self.next_occupied();
            self.hand = (slot + 1) % self.ring.len();
            let victim_key = self.ring[slot].as_ref().expect("occupied");
            let victim = self
                .entries
                .get_mut(&**victim_key)
                .expect("every ring key has an entry");
            if victim.referenced {
                if first && clear_mark {
                    victim.referenced = false;
                } else {
                    // Leave the hand on the used slot it did not pass.
                    self.hand = slot;
                }
                return Err((key, value));
            }
            let victim_key = self.ring[slot].take().expect("occupied");
            let victim = self.entries.remove(&*victim_key).expect("entry");
            self.used -= victim.weight;
            self.free.push(slot);
            first = false;
        }
        match self.free.pop() {
            Some(slot) => self.ring[slot] = Some(Rc::clone(&key)),
            None => self.ring.push(Some(Rc::clone(&key))),
        }
        self.used += weight;
        self.entries.insert(
            key,
            Entry {
                value,
                referenced: true,
                weight,
            },
        );
        Ok(())
    }

    /// Keep a string the clock turned away in the FIFO, dropping the oldest
    /// ones while it does not fit.
    fn turn_away(&mut self, key: Rc<K>, value: V, weight: usize) {
        if weight > self.recent_budget {
            return;
        }
        while self.recent_used + weight > self.recent_budget {
            let Some(old) = self.recent_order.pop_front() else {
                break;
            };
            // Skip a name whose string was promoted (or replaced) since.
            let current = self
                .recent
                .get_key_value(&*old)
                .is_some_and(|(k, _)| Rc::ptr_eq(k, &old));
            if current {
                let (_, w) = self.recent.remove(&*old).expect("just found");
                self.recent_used -= w;
            }
        }
        // Names of promoted strings pile up only while nothing is dropped;
        // keep the queue within twice the strings it holds.
        if self.recent_order.len() > 2 * self.recent.len() + 16 {
            let recent = &self.recent;
            self.recent_order.retain(|k| {
                recent
                    .get_key_value(&**k)
                    .is_some_and(|(held, _)| Rc::ptr_eq(held, k))
            });
        }
        self.recent_order.push_back(Rc::clone(&key));
        self.recent_used += weight;
        self.recent.insert(key, (value, weight));
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
}

#[cfg(test)]
thread_local! {
    static MEMOS_OFF: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[inline]
fn memos_off() -> bool {
    #[cfg(test)]
    {
        MEMOS_OFF.with(std::cell::Cell::get)
    }
    #[cfg(not(test))]
    {
        false
    }
}

/// Test-only: run `f` with every text memo on this thread bypassed (no
/// lookups, no inserts), so a result can be compared with the cached one.
/// The switch is put back as it was even when `f` panics.
#[cfg(test)]
pub(crate) fn without_text_memos<R>(f: impl FnOnce() -> R) -> R {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            MEMOS_OFF.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(MEMOS_OFF.with(|c| c.replace(true)));
    f()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One pass over `keys`: returns the misses (each miss is then inserted,
    /// as the callers do).
    fn pass(memo: &mut TextMemo<u32, u32>, keys: impl Iterator<Item = u32>) -> usize {
        let mut misses = 0;
        for k in keys {
            if memo.get(&k).is_none() {
                misses += 1;
                memo.insert(k, k * 2);
            }
        }
        misses
    }

    #[test]
    fn a_pass_over_fewer_strings_than_the_cap_misses_none_the_second_time() {
        let mut memo = TextMemo::new(8);
        assert_eq!(pass(&mut memo, 0..8), 8);
        assert_eq!(pass(&mut memo, 0..8), 0);
        assert_eq!(memo.get(&3), Some(6));
    }

    #[test]
    fn a_pass_over_more_strings_than_the_cap_keeps_the_held_ones() {
        // 40 distinct keys through 32 slots (and a FIFO of 2), pass after
        // pass: the 32 held stay hits, only the 8 that do not fit are shaped
        // again (clear-when-full re-shaped all 40 every pass).
        let mut memo = TextMemo::new(32);
        assert_eq!(pass(&mut memo, 0..40), 40);
        for _ in 0..5 {
            assert_eq!(pass(&mut memo, 0..40), 8);
            assert_eq!(memo.len(), 32);
        }
    }

    #[test]
    fn a_string_looked_up_again_right_after_it_was_turned_away_is_a_hit() {
        // Layout asks for the same text several times in a row. A turned-away
        // string is served from the FIFO, and the repeats do not move the
        // hand, so the held entries survive.
        let mut memo = TextMemo::new(32);
        let repeated = |k: u32| std::iter::repeat(k).take(10);
        assert_eq!(pass(&mut memo, (0..40).flat_map(repeated)), 40);
        for _ in 0..5 {
            assert_eq!(pass(&mut memo, (0..40).flat_map(repeated)), 8);
            assert_eq!(memo.len(), 32);
        }
    }

    #[test]
    fn strings_no_longer_used_are_replaced_within_two_laps() {
        let mut memo = TextMemo::new(32);
        pass(&mut memo, 0..32);
        // A new set of 8 (more than the FIFO's 2): the first lap clears the
        // old marks, the second replaces old entries; after that every new
        // key is held.
        let mut rounds = 0;
        while pass(&mut memo, 100..108) > 0 {
            rounds += 1;
            assert!(rounds <= 16, "new keys never admitted");
        }
        for k in 100..108 {
            assert_eq!(memo.get(&k), Some(k * 2));
        }
        assert_eq!(memo.len(), 32);
    }

    #[test]
    fn a_new_font_generation_drops_every_entry() {
        let mut memo = TextMemo::new(8);
        memo.sync_generation(1);
        pass(&mut memo, 0..4);
        memo.sync_generation(1);
        assert_eq!(memo.len(), 4);
        memo.sync_generation(2);
        assert_eq!(memo.len(), 0);
        assert_eq!(pass(&mut memo, 0..4), 4);
    }

    #[test]
    fn the_memo_never_holds_more_than_its_cap() {
        let mut memo = TextMemo::new(32);
        for round in 0..20u32 {
            pass(&mut memo, round * 7..round * 7 + 45);
            assert!(memo.len() <= 32);
            assert_eq!(
                memo.ring.iter().filter(|k| k.is_some()).count(),
                memo.entries.len()
            );
            assert!(memo.recent.len() <= 2);
            assert_eq!(memo.weight(), memo.entries.len() + memo.recent.len());
        }
    }

    #[test]
    fn bypassed_memos_neither_answer_nor_keep() {
        let mut memo = TextMemo::new(8);
        pass(&mut memo, 0..2);
        without_text_memos(|| {
            assert_eq!(memo.get(&0), None);
            memo.insert(5, 10);
        });
        assert_eq!(memo.get(&0), Some(0));
        assert_eq!(memo.get(&5), None);
    }

    #[test]
    fn a_byte_bounded_memo_never_holds_more_than_its_budget() {
        // Each entry weighs its value; sizes from 1 to 40 through a budget of
        // 160 (150 in the clock, 10 in the queue), with repeats.
        let mut memo: TextMemo<u32, u32> = TextMemo::with_byte_budget(160, |_, v| *v as usize);
        let weight = |k: u32| 1 + (k * 7) % 40;
        for round in 0..30u32 {
            for k in round * 3..round * 3 + 25 {
                for _ in 0..3 {
                    if memo.get(&k).is_none() {
                        memo.insert(k, weight(k));
                    }
                }
                assert!(memo.used <= 150, "clock holds {}", memo.used);
                assert!(memo.recent_used <= 10, "queue holds {}", memo.recent_used);
                assert!(memo.weight() <= 160);
                assert_eq!(
                    memo.used,
                    memo.entries.values().map(|e| e.weight).sum::<usize>()
                );
                assert_eq!(
                    memo.recent_used,
                    memo.recent.values().map(|(_, w)| *w).sum::<usize>()
                );
            }
        }
        assert!(
            memo.used > 75,
            "the clock stayed nearly empty: {}",
            memo.used
        );
    }

    #[test]
    fn a_large_string_replaces_several_unused_small_ones() {
        let mut memo: TextMemo<u32, u32> = TextMemo::with_byte_budget(32, |_, v| *v as usize);
        // Clock budget 30: three entries of 10, then a lap of misses clears
        // their marks (each miss is turned away).
        for k in 0..3 {
            memo.insert(k, 10);
        }
        for k in 10..13 {
            memo.insert(k, 1);
        }
        assert_eq!(memo.len(), 3);
        // 25 does not fit in place of one: the hand evicts unused slots until
        // it does.
        memo.insert(99, 25);
        assert!(memo.entries.contains_key(&99));
        assert!(memo.used <= 30);
    }

    #[test]
    fn the_test_switch_is_restored_when_the_closure_panics() {
        let mut memo = TextMemo::new(8);
        let caught = std::panic::catch_unwind(|| without_text_memos(|| panic!("inside")));
        assert!(caught.is_err());
        // The memos answer again on this thread.
        memo.insert(1, 2);
        assert_eq!(memo.get(&1), Some(2));
    }

    #[test]
    fn a_string_re_read_from_the_queue_moves_into_an_unused_slot() {
        // The memo is full of strings the page no longer uses: a lap of
        // misses has cleared every mark. A string the clock turned away and
        // the page keeps reading is promoted into the main table instead of
        // living in the queue.
        let mut memo = TextMemo::new(32);
        pass(&mut memo, 0..32);
        // 32 misses, each turned away: their looks clear all 32 marks.
        pass(&mut memo, 100..132);
        assert_eq!(memo.get(&131), Some(262));
        assert!(
            memo.entries.contains_key(&131),
            "a re-read turned-away string stayed in the queue while the slot under the hand was unused"
        );
        assert_eq!(memo.len(), 32);
    }
}
