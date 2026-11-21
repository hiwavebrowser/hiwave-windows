//! HIWAVE PATCH (sort-v8): V8's `Array.prototype.sort`, ported from its Torque source
//! (`third_party/v8/builtins/array-sort.tq`: TimSort with binary insertion for short runs).
//!
//! ECMA-262 leaves the order unspecified when the comparator is not a consistent total order
//! (`(a, b) => a > b`, a NaN result, `Math.random() - 0.5`), so what a page sees is whatever V8
//! happens to do, and pages depend on it (observable-plot keeps a different top-N otherwise). The
//! comparator is called with the same arguments in the same order as V8 does, so even a comparator
//! with side effects (a log, a counter) sees the same sequence. For a consistent comparator the
//! result is the unique stable order, as before.
//!
//! The caller removes `undefined`s and holes first (V8 never passes `undefined` to the comparator)
//! and puts them back after the sorted values.

use crate::JsResult;
use std::cmp::Ordering;

type Cmp<'a, T> = &'a mut dyn FnMut(&T, &T) -> JsResult<Ordering>;

const MIN_GALLOP_WINS: usize = 7;
const SHORT_ARRAY: usize = 8;

struct Run {
    base: usize,
    len: usize,
}

struct Sorter<'a, T: Clone> {
    work: &'a mut Vec<T>,
    cmp: Cmp<'a, T>,
    min_gallop: usize,
    runs: Vec<Run>,
}

/// Sorts `work` in place the way V8 does. On `Err` the comparator is not called again and `work`
/// is unspecified.
pub(super) fn v8_timsort<T: Clone>(work: &mut Vec<T>, cmp: Cmp<'_, T>, short_insertion: bool) -> JsResult<()> {
    let length = work.len();
    if length < 2 {
        return Ok(());
    }
    let mut s = Sorter { work, cmp, min_gallop: MIN_GALLOP_WINS, runs: Vec::new() };
    // Measured on Chrome 148.0.7778.216: `Array.prototype.sort` on fewer than 8 elements goes
    // straight to binary insertion, with no run detection (a sorted 7-element array costs 10
    // comparator calls, 8 costs 7); `toSorted` always detects runs first.
    if short_insertion && length < SHORT_ARRAY {
        return s.binary_insertion_sort(0, 0, length);
    }
    let min_run = compute_min_run_length(length);
    let mut low = 0;
    let mut remaining = length;
    while remaining != 0 {
        let mut current = s.count_and_make_run(low, low + remaining)?;
        if current < min_run {
            let forced = min_run.min(remaining);
            s.binary_insertion_sort(low, low + current, low + forced)?;
            current = forced;
        }
        s.runs.push(Run { base: low, len: current });
        s.merge_collapse()?;
        low += current;
        remaining -= current;
    }
    s.merge_force_collapse()
}

fn compute_min_run_length(mut n: usize) -> usize {
    let mut r = 0;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

impl<T: Clone> Sorter<'_, T> {
    fn compare(&mut self, a: &T, b: &T) -> JsResult<Ordering> {
        (self.cmp)(a, b)
    }

    /// Compare of two elements of the work array, by index.
    fn cmp_at(&mut self, i: usize, j: usize) -> JsResult<Ordering> {
        let (a, b) = (self.work[i].clone(), self.work[j].clone());
        self.compare(&a, &b)
    }

    /// The length of the run starting at `low_arg` (strictly descending runs are reversed).
    fn count_and_make_run(&mut self, low_arg: usize, high: usize) -> JsResult<usize> {
        let low = low_arg + 1;
        if low == high {
            return Ok(1);
        }
        let mut run_length = 2;
        let is_descending = self.cmp_at(low, low - 1)? == Ordering::Less;
        let mut previous = low;
        for idx in low + 1..high {
            let order = self.cmp_at(idx, previous)?;
            if is_descending {
                if order != Ordering::Less {
                    break;
                }
            } else if order == Ordering::Less {
                break;
            }
            previous = idx;
            run_length += 1;
        }
        if is_descending {
            self.work[low_arg..low_arg + run_length].reverse();
        }
        Ok(run_length)
    }

    fn binary_insertion_sort(&mut self, low: usize, start_arg: usize, high: usize) -> JsResult<()> {
        let mut start = if low == start_arg { start_arg + 1 } else { start_arg };
        while start < high {
            let mut left = low;
            let mut right = start;
            let pivot = self.work[right].clone();
            while left < right {
                let mid = left + ((right - left) >> 1);
                let mid_element = self.work[mid].clone();
                if self.compare(&pivot, &mid_element)? == Ordering::Less {
                    right = mid;
                } else {
                    left = mid + 1;
                }
            }
            let mut p = start;
            while p > left {
                self.work[p] = self.work[p - 1].clone();
                p -= 1;
            }
            self.work[left] = pivot;
            start += 1;
        }
        Ok(())
    }

    /// `work[dst..dst + n] = work[src..src + n]`, correct for overlapping ranges.
    fn copy_within(&mut self, src: usize, dst: usize, n: usize) {
        if dst <= src {
            for k in 0..n {
                self.work[dst + k] = self.work[src + k].clone();
            }
        } else {
            for k in (0..n).rev() {
                self.work[dst + k] = self.work[src + k].clone();
            }
        }
    }

    fn run_invariant_established(&self, n: usize) -> bool {
        if n < 2 {
            return true;
        }
        self.runs[n - 2].len > self.runs[n - 1].len + self.runs[n].len
    }

    fn merge_collapse(&mut self) -> JsResult<()> {
        while self.runs.len() > 1 {
            let mut n = self.runs.len() - 2;
            if !self.run_invariant_established(n + 1) || !self.run_invariant_established(n) {
                if self.runs[n - 1].len < self.runs[n + 1].len {
                    n -= 1;
                }
                self.merge_at(n)?;
            } else if self.runs[n].len <= self.runs[n + 1].len {
                self.merge_at(n)?;
            } else {
                break;
            }
        }
        Ok(())
    }

    fn merge_force_collapse(&mut self) -> JsResult<()> {
        while self.runs.len() > 1 {
            let mut n = self.runs.len() - 2;
            if n > 0 && self.runs[n - 1].len < self.runs[n + 1].len {
                n -= 1;
            }
            self.merge_at(n)?;
        }
        Ok(())
    }

    fn merge_at(&mut self, i: usize) -> JsResult<()> {
        let (mut base_a, mut length_a) = (self.runs[i].base, self.runs[i].len);
        let (base_b, mut length_b) = (self.runs[i + 1].base, self.runs[i + 1].len);
        self.runs[i].len = length_a + length_b;
        if i + 3 == self.runs.len() {
            self.runs[i + 1] = Run { base: self.runs[i + 2].base, len: self.runs[i + 2].len };
        }
        self.runs.pop();

        // Where does b start in a? Elements in a before that are already in place.
        let key_right = self.work[base_b].clone();
        let k = self.gallop_right(Which::Work, &key_right, base_a, length_a, 0)?;
        base_a += k;
        length_a -= k;
        if length_a == 0 {
            return Ok(());
        }
        // Where does a end in b? Elements in b after that are already in place.
        let key_left = self.work[base_a + length_a - 1].clone();
        length_b = self.gallop_left(Which::Work, &key_left, base_b, length_b, length_b - 1)?;
        if length_b == 0 {
            return Ok(());
        }
        if length_a <= length_b {
            self.merge_low(base_a, length_a, base_b, length_b)
        } else {
            self.merge_high(base_a, length_a, base_b, length_b)
        }
    }

    fn arr<'b>(&'b self, which: Which, temp: &'b [T]) -> &'b [T] {
        match which {
            Which::Work => self.work.as_slice(),
            Which::Temp => temp,
        }
    }

    /// `GallopLeft`: the leftmost position in the sorted run `array[base..base + length]` at which
    /// `key` can go before the first element not less than it. `temp` is the temporary run copy
    /// for `Which::Temp`.
    fn gallop_left_in(&mut self, temp: &[T], which: Which, key: &T, base: usize, length: usize, hint: usize) -> JsResult<usize> {
        let mut last_ofs: isize = 0;
        let mut offset: isize = 1;
        let hint_i = hint as isize;
        let length_i = length as isize;
        let at = |s: &Self, i: isize| -> T { s.arr(which, temp)[base + i as usize].clone() };
        let base_hint = at(self, hint_i);
        if self.compare(&base_hint, key)? == Ordering::Less {
            // a[hint] < key: gallop right until a[hint + last_ofs] < key <= a[hint + offset].
            let max_ofs = length_i - hint_i;
            while offset < max_ofs {
                let e = at(self, hint_i + offset);
                if self.compare(&e, key)? != Ordering::Less {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            last_ofs += hint_i;
            offset += hint_i;
        } else {
            // key <= a[hint]: gallop left until a[hint - offset] < key <= a[hint - last_ofs].
            let max_ofs = hint_i + 1;
            while offset < max_ofs {
                let e = at(self, hint_i - offset);
                if self.compare(&e, key)? == Ordering::Less {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            let tmp = last_ofs;
            last_ofs = hint_i - offset;
            offset = hint_i - tmp;
        }
        // Now a[last_ofs] < key <= a[offset]: binary search in (last_ofs, offset].
        last_ofs += 1;
        while last_ofs < offset {
            let m = last_ofs + ((offset - last_ofs) >> 1);
            let e = at(self, m);
            if self.compare(&e, key)? == Ordering::Less {
                last_ofs = m + 1;
            } else {
                offset = m;
            }
        }
        Ok(offset as usize)
    }

    fn gallop_right_in(&mut self, temp: &[T], which: Which, key: &T, base: usize, length: usize, hint: usize) -> JsResult<usize> {
        let mut last_ofs: isize = 0;
        let mut offset: isize = 1;
        let hint_i = hint as isize;
        let length_i = length as isize;
        let at = |s: &Self, i: isize| -> T { s.arr(which, temp)[base + i as usize].clone() };
        let base_hint = at(self, hint_i);
        if self.compare(key, &base_hint)? == Ordering::Less {
            // key < a[hint]: gallop left until a[hint - offset] <= key < a[hint - last_ofs].
            let max_ofs = hint_i + 1;
            while offset < max_ofs {
                let e = at(self, hint_i - offset);
                if self.compare(key, &e)? != Ordering::Less {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            let tmp = last_ofs;
            last_ofs = hint_i - offset;
            offset = hint_i - tmp;
        } else {
            // a[hint] <= key: gallop right until a[hint + last_ofs] <= key < a[hint + offset].
            let max_ofs = length_i - hint_i;
            while offset < max_ofs {
                let e = at(self, hint_i + offset);
                if self.compare(key, &e)? == Ordering::Less {
                    break;
                }
                last_ofs = offset;
                offset = (offset << 1) + 1;
            }
            if offset > max_ofs {
                offset = max_ofs;
            }
            last_ofs += hint_i;
            offset += hint_i;
        }
        last_ofs += 1;
        while last_ofs < offset {
            let m = last_ofs + ((offset - last_ofs) >> 1);
            let e = at(self, m);
            if self.compare(key, &e)? == Ordering::Less {
                offset = m;
            } else {
                last_ofs = m + 1;
            }
        }
        Ok(offset as usize)
    }

    fn gallop_left(&mut self, which: Which, key: &T, base: usize, length: usize, hint: usize) -> JsResult<usize> {
        self.gallop_left_in(&[], which, key, base, length, hint)
    }

    fn gallop_right(&mut self, which: Which, key: &T, base: usize, length: usize, hint: usize) -> JsResult<usize> {
        self.gallop_right_in(&[], which, key, base, length, hint)
    }

    fn merge_low(&mut self, base_a: usize, mut length_a: usize, base_b: usize, mut length_b: usize) -> JsResult<()> {
        let temp: Vec<T> = self.work[base_a..base_a + length_a].to_vec();
        let mut dest = base_a;
        let mut cursor_temp = 0usize;
        let mut cursor_b = base_b;

        self.work[dest] = self.work[cursor_b].clone();
        dest += 1;
        cursor_b += 1;

        enum End {
            Succeed,
            CopyB,
        }
        let end = 'merge: {
            length_b -= 1;
            if length_b == 0 {
                break 'merge End::Succeed;
            }
            if length_a == 1 {
                break 'merge End::CopyB;
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut wins_a = 0usize;
                let mut wins_b = 0usize;
                // Straightforward merge until one run appears to win consistently.
                loop {
                    let (x, y) = (self.work[cursor_b].clone(), temp[cursor_temp].clone());
                    if self.compare(&x, &y)? == Ordering::Less {
                        self.work[dest] = self.work[cursor_b].clone();
                        dest += 1;
                        cursor_b += 1;
                        wins_b += 1;
                        length_b -= 1;
                        wins_a = 0;
                        if length_b == 0 {
                            break 'merge End::Succeed;
                        }
                        if wins_b >= min_gallop {
                            break;
                        }
                    } else {
                        self.work[dest] = temp[cursor_temp].clone();
                        dest += 1;
                        cursor_temp += 1;
                        wins_a += 1;
                        length_a -= 1;
                        wins_b = 0;
                        if length_a == 1 {
                            break 'merge End::CopyB;
                        }
                        if wins_a >= min_gallop {
                            break;
                        }
                    }
                }
                // One run is winning so consistently that galloping may be a huge win.
                min_gallop += 1;
                let mut first = true;
                while wins_a >= MIN_GALLOP_WINS || wins_b >= MIN_GALLOP_WINS || first {
                    first = false;
                    min_gallop = (min_gallop.saturating_sub(1)).max(1);
                    self.min_gallop = min_gallop;

                    let key = self.work[cursor_b].clone();
                    wins_a = self.gallop_right_in(&temp, Which::Temp, &key, cursor_temp, length_a, 0)?;
                    if wins_a > 0 {
                        for k in 0..wins_a {
                            self.work[dest + k] = temp[cursor_temp + k].clone();
                        }
                        dest += wins_a;
                        cursor_temp += wins_a;
                        length_a -= wins_a;
                        if length_a == 1 {
                            break 'merge End::CopyB;
                        }
                        // length_a == 0 cannot happen for a consistent comparator, but ours may not be one.
                        if length_a == 0 {
                            break 'merge End::Succeed;
                        }
                    }
                    self.work[dest] = self.work[cursor_b].clone();
                    dest += 1;
                    cursor_b += 1;
                    length_b -= 1;
                    if length_b == 0 {
                        break 'merge End::Succeed;
                    }

                    let key = temp[cursor_temp].clone();
                    wins_b = self.gallop_left(Which::Work, &key, cursor_b, length_b, 0)?;
                    if wins_b > 0 {
                        self.copy_within(cursor_b, dest, wins_b);
                        dest += wins_b;
                        cursor_b += wins_b;
                        length_b -= wins_b;
                        if length_b == 0 {
                            break 'merge End::Succeed;
                        }
                    }
                    self.work[dest] = temp[cursor_temp].clone();
                    dest += 1;
                    cursor_temp += 1;
                    length_a -= 1;
                    if length_a == 1 {
                        break 'merge End::CopyB;
                    }
                }
                min_gallop += 1; // Penalize for leaving galloping mode.
                self.min_gallop = min_gallop;
            }
        };
        match end {
            End::Succeed => {
                for k in 0..length_a {
                    self.work[dest + k] = temp[cursor_temp + k].clone();
                }
            }
            End::CopyB => {
                self.copy_within(cursor_b, dest, length_b);
                self.work[dest + length_b] = temp[cursor_temp].clone();
            }
        }
        Ok(())
    }

    fn merge_high(&mut self, base_a: usize, mut length_a: usize, base_b: usize, mut length_b: usize) -> JsResult<()> {
        let temp: Vec<T> = self.work[base_b..base_b + length_b].to_vec();
        // Signed cursors: they step below zero when a run is used up.
        let mut dest = (base_b + length_b) as isize - 1;
        let mut cursor_temp = length_b as isize - 1;
        let mut cursor_a = (base_a + length_a) as isize - 1;

        self.work[dest as usize] = self.work[cursor_a as usize].clone();
        dest -= 1;
        cursor_a -= 1;

        enum End {
            Succeed,
            CopyA,
        }
        let end = 'merge: {
            length_a -= 1;
            if length_a == 0 {
                break 'merge End::Succeed;
            }
            if length_b == 1 {
                break 'merge End::CopyA;
            }
            let mut min_gallop = self.min_gallop;
            loop {
                let mut wins_a = 0usize;
                let mut wins_b = 0usize;
                loop {
                    let (x, y) = (temp[cursor_temp as usize].clone(), self.work[cursor_a as usize].clone());
                    if self.compare(&x, &y)? == Ordering::Less {
                        self.work[dest as usize] = self.work[cursor_a as usize].clone();
                        dest -= 1;
                        cursor_a -= 1;
                        wins_a += 1;
                        length_a -= 1;
                        wins_b = 0;
                        if length_a == 0 {
                            break 'merge End::Succeed;
                        }
                        if wins_a >= min_gallop {
                            break;
                        }
                    } else {
                        self.work[dest as usize] = temp[cursor_temp as usize].clone();
                        dest -= 1;
                        cursor_temp -= 1;
                        wins_b += 1;
                        length_b -= 1;
                        wins_a = 0;
                        if length_b == 1 {
                            break 'merge End::CopyA;
                        }
                        if wins_b >= min_gallop {
                            break;
                        }
                    }
                }
                min_gallop += 1;
                let mut first = true;
                while wins_a >= MIN_GALLOP_WINS || wins_b >= MIN_GALLOP_WINS || first {
                    first = false;
                    min_gallop = (min_gallop.saturating_sub(1)).max(1);
                    self.min_gallop = min_gallop;

                    let key = temp[cursor_temp as usize].clone();
                    let k = self.gallop_right(Which::Work, &key, base_a, length_a, length_a - 1)?;
                    wins_a = length_a - k;
                    if wins_a > 0 {
                        dest -= wins_a as isize;
                        cursor_a -= wins_a as isize;
                        self.copy_within((cursor_a + 1) as usize, (dest + 1) as usize, wins_a);
                        length_a -= wins_a;
                        if length_a == 0 {
                            break 'merge End::Succeed;
                        }
                    }
                    self.work[dest as usize] = temp[cursor_temp as usize].clone();
                    dest -= 1;
                    cursor_temp -= 1;
                    length_b -= 1;
                    if length_b == 1 {
                        break 'merge End::CopyA;
                    }

                    let key = self.work[cursor_a as usize].clone();
                    let k = self.gallop_left_in(&temp, Which::Temp, &key, 0, length_b, length_b - 1)?;
                    wins_b = length_b - k;
                    if wins_b > 0 {
                        dest -= wins_b as isize;
                        cursor_temp -= wins_b as isize;
                        for j in 0..wins_b {
                            self.work[(dest + 1) as usize + j] = temp[(cursor_temp + 1) as usize + j].clone();
                        }
                        length_b -= wins_b;
                        if length_b == 1 {
                            break 'merge End::CopyA;
                        }
                        // length_b == 0 cannot happen for a consistent comparator, but ours may not be one.
                        if length_b == 0 {
                            break 'merge End::Succeed;
                        }
                    }
                    self.work[dest as usize] = self.work[cursor_a as usize].clone();
                    dest -= 1;
                    cursor_a -= 1;
                    length_a -= 1;
                    if length_a == 0 {
                        break 'merge End::Succeed;
                    }
                }
                min_gallop += 1;
                self.min_gallop = min_gallop;
            }
        };
        match end {
            End::Succeed => {
                if length_b > 0 {
                    let start = (dest + 1) as usize - length_b;
                    for j in 0..length_b {
                        self.work[start + j] = temp[j].clone();
                    }
                }
            }
            End::CopyA => {
                dest -= length_a as isize;
                cursor_a -= length_a as isize;
                self.copy_within((cursor_a + 1) as usize, (dest + 1) as usize, length_a);
                self.work[dest as usize] = temp[cursor_temp as usize].clone();
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Which {
    Work,
    Temp,
}

/// V8's `%TypedArray%.prototype.sort` with a comparator: a top-down merge sort that alternates the
/// roles of its two work arrays on each level (`typed-array-sort.tq`, `TypedArrayMergeSort`). A
/// left element is kept when the comparator says `<= 0` (stable). On `Err` the comparator is not
/// called again and `values` is unspecified.
pub(super) fn v8_typed_merge_sort<T: Clone>(values: &mut Vec<T>, cmp: Cmp<'_, T>) -> JsResult<()> {
    let len = values.len();
    if len < 2 {
        return Ok(());
    }
    let mut work1 = values.clone();
    let mut work2 = values.clone();
    typed_merge(&mut work2, 0, len, &mut work1, cmp)?;
    *values = work1;
    Ok(())
}

fn typed_merge<T: Clone>(source: &mut Vec<T>, from: usize, to: usize, target: &mut Vec<T>, cmp: Cmp<'_, T>) -> JsResult<()> {
    let middle = from + ((to - from) >> 1);
    // On the next level the source becomes the target and the other way round.
    if middle - from > 1 {
        typed_merge(target, from, middle, source, cmp)?;
    }
    if to - middle > 1 {
        typed_merge(target, middle, to, source, cmp)?;
    }
    let (mut left, mut right) = (from, middle);
    for target_index in from..to {
        if left < middle && right >= to {
            target[target_index] = source[left].clone();
            left += 1;
        } else if left < middle {
            let (l, r) = (source[left].clone(), source[right].clone());
            if cmp(&l, &r)? != Ordering::Greater {
                target[target_index] = l;
                left += 1;
            } else {
                target[target_index] = r;
                right += 1;
            }
        } else {
            target[target_index] = source[right].clone();
            right += 1;
        }
    }
    Ok(())
}
