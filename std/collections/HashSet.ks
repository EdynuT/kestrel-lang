// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * A hash set: unique items stored in a hash map.
 *
 * The set owns its items: `add` takes them. `contains` and `remove` only
 * look at the item they are given (it stays with whoever calls them).
 *
 * @since 0.7
 * @author Edynu
 * @see HashSet.add
 * @see HashSet.contains
 * @see HashSet.remove
 */

use std.collections.HashMap;

public class HashSet<T> {
    map: HashMap<T, bool>;

    /* An empty set: `HashSet<int>()`. */
    public proc HashSet() -> HashSet<T> {
        return HashSet<T> { map: HashMap<T, bool>() };
    }

    /* Adds `item` to the set; true if it was not there yet. */
    public proc add(&self, item: T) -> bool {
        previous: bool? = self.map.put(give item, true);
        return previous == null;
    }

    /* Whether the set has `item`. */
    public proc contains(&self, item: &T) -> bool {
        return self.map.contains(&item);
    }

    /* Takes `item` out of the set; true if it was there. */
    public proc remove(&self, item: &T) -> bool {
        removed: bool? = self.map.remove(&item);
        return removed != null;
    }

    /* How many items there are. */
    public proc len(&self) -> int {
        return self.map.len();
    }
}
