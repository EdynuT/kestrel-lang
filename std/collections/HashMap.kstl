// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * A hash map: keys mapped to values.
 *
 * The map owns its keys and values. `put` takes them, `get` lends the value,
 * and `remove` gives both back to whoever calls it.
 *
 * @since 0.7
 * @author Edynu
 * @see HashMap.put
 * @see HashMap.get
 * @see HashMap.remove
 */

/* How many buckets a new map has. */
const BUCKETS: int = 16;

/* One key and its value, and the next pair in the same bucket. The value
 * is kept as `V?` so `remove` can take it out and leave the pair empty. */
class Entry<K, V> {
    public key: K;
    public value: V?;
    public next: Entry<K, V>?;

    public proc Entry(key: K, value: V) -> Entry<K, V> {
        return Entry<K, V> { key: give key, value: give value, next: null };
    }
}

/* The pairs whose key falls here: a linked list, maybe empty. */
class Bucket<K, V> {
    public first: Entry<K, V>?;

    public proc Bucket() -> Bucket<K, V> {
        return Bucket<K, V> { first: null };
    }
}

public class HashMap<K, V> {
    buckets[]: Bucket<K, V>;
    size: int;

    /* An empty map: `HashMap<text, int>()`. */
    public proc HashMap() -> HashMap<K, V> {
        buckets[]: Bucket<K, V> = {};
        walk i in 1..BUCKETS {
            append(buckets, Bucket<K, V>());
        }
        return HashMap<K, V> { buckets: give buckets, size: 0 };
    }

    /* The bucket where `key` goes. */
    proc slot(&self, key: &K) -> int {
        count: int = len(self.buckets);
        slot: int = key.hashCode() % count;
        if slot < 0 {
            slot = slot + count;
        }
        return slot;
    }

    /* The value of `key`, lent; `null` if the map does not have it. */
    public proc get(&self, key: &K) -> &V? {
        bucket: &Bucket<K, V> = &self.buckets[self.slot(&key)];
        current: &Entry<K, V>? = &bucket.first;
        while current != null {
            if current.key.equals(&key) {
                return &current.value;
            }
            current = &current.next;
        }
        return null;
    }

    public proc contains(&self, key: &K) -> bool {
        return self.get(&key) != null;
    }

    /* Takes `key` and its value out, and gives the value back (`null` if
     * the map does not have it). The pairs of the bucket are taken out one
     * by one and the others put back. */
    public proc remove(&self, key: &K) -> V? {
        bucket: &Bucket<K, V> = &self.buckets[self.slot(&key)];
        rest: Entry<K, V>? = give bucket.first;
        found: V? = null;
        while rest != null {
            entry: Entry<K, V> = give rest;
            rest = give entry.next;
            if entry.key.equals(&key) {
                found = give entry.value;
                self.size = self.size - 1;
                burn entry;
            } else {
                entry.next = give bucket.first;
                bucket.first = give entry;
            }
        }
        burn rest; // empty now: a `T?` is burned even so
        return found;
    }

    /* Puts `value` at `key`, and gives back the value that was there (`null` if none). */
    public proc put(&self, key: K, value: V) -> V? {
        previous: V? = self.remove(&key);
        bucket: &Bucket<K, V> = &self.buckets[self.slot(&key)];
        entry: Entry<K, V> = Entry<K, V>(give key, give value);
        entry.next = give bucket.first;
        bucket.first = give entry;
        self.size = self.size + 1;
        return previous;
    }

    /* How many keys there are. */
    public proc len(&self) -> int {
        return self.size;
    }
}
