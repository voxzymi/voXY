# Alight Motion samples

Drop the files you export from your own copy of Alight Motion here:

- `.alightmotion` project exports,
- preset and element XML exports, and
- clipboard or share-link payloads saved as text files.

Name each file after what it shows, e.g. `01-empty.alightmotion`,
`02-one-solid.alightmotion`, `03-position-two-keys.alightmotion`, and put the
matching rendered video next to it with the same stem (e.g.
`03-position-two-keys.mp4`).

Run `cargo xtask am-inspect tests/am-samples/*` to see what each file
contains; `cargo test -p voxy-am-compat --test samples` checks that every
file round-trips byte-for-byte.

Whether these files are committed to the repository is your call: they
contain your own project content. They can also stay local; the tests skip
when the folder is empty.
