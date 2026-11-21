# std — the Kestrel standard library

Every `.ks` file here is built into `ksc` and used with `use std.<path>;`:
`std/collections/HashMap.ks` is `use std.collections.HashMap;`.

Folders are lowercase and group files by subject (`text`, `collections`,
`io`, `math`); files are PascalCase. Rebuilding `ksc` (`cargo build`) picks up
new and changed files.
