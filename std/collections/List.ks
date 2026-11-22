// Copyright (c) 2026 Edynu
// SPDX-License-Identifier: GPL-3.0-or-later

/*
 * A list that grows: items kept in order, read by their index (from 0).
 *
 * The list owns its items. `add` takes one (`give x` for an object), `get`
 * lends one, and `remove` and `pop` give one back to whoever calls them.
 *
 * @since 0.7
 * @author Edynu
 * @see List.add
 * @see List.get
 * @see List.remove
 * @see List.pop
 * @see List.len
 * @see List.isEmpty
 * @see List.indexOf
 * @see List.contains
 * @see List.clear
 */

public class List<T> {
    items[]: T;

    /* An empty list: `List<int>()`. */
    public proc List() -> List<T> {
        empty[]: T = {};
        return List<T> { items: give empty };
    }

    /* Puts `item` at the end. */
    public proc add(&self, item: T) {
        append(self.items, give item);
    }

    /* The item at `index`, lent: it stays in the list. */
    public proc get(&self, index: int) -> &T {
        return &self.items[index];
    }

    /* Puts `item` at `index` and gives back the item that was there. */
    public proc set(&self, index: int, item: T) -> T {
        previous: T = remove(self.items, index);
        insert(self.items, index, give item);
        return previous;
    }

    /* Takes out the item at `index` (the ones after it move back one place). */
    public proc remove(&self, index: int) -> T {
        return remove(self.items, index);
    }

    /* Takes out the last item. */
    public proc pop(&self) -> T {
        return pop(self.items);
    }

    /* How many items there are. */
    public proc len(&self) -> int {
        return len(self.items);
    }

    /* Whether the list is empty. */
    public proc isEmpty(&self) -> bool {
        return len(self.items) == 0;
    }

    /* Where the first item equal to `item` is (`equals`), or -1. */
    public proc indexOf(&self, item: &T) -> int {
        for i: int = 0; i < len(self.items); i++ {
            if self.items[i].equals(&item) {
                return i;
            }
        }
        return -1;
    }

    /* Whether some item is equal to `item` (`equals`). */
    public proc contains(&self, item: &T) -> bool {
        return self.indexOf(&item) != -1;
    }

    /* Takes out and burns every item. */
    public proc clear(&self) {
        while len(self.items) > 0 {
            last: T = pop(self.items);
            burn last;
        }
    }
}
