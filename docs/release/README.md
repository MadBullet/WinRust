# Release notes

`notes.md` in this directory is the **only** source of a release's body: the
release workflow passes it to `gh release create --notes-file`. Editing the
text on the GitHub release page alone is therefore not enough - the next
release would copy the stale file again.

## Language policy

**From 0.9.3 on, every release is published in two languages: English first,
then Russian.**

The order and the labels are not cosmetic:

1. The English section comes first and is prefixed with a note saying that the
   changelog is in English and that the Russian translation follows
   (`> **Language note: ...**`).
2. A `---` horizontal rule separates the two halves.
3. The Russian section follows the rule, prefixed with
   `> **Примечание о языке: ...**` saying it is the same changelog in Russian.

Why: the audience for these components is not only Russian-speaking. Someone
who finds the project through a search in English must be able to read what
changed without translating anything, and a reader who works in Russian must
get the same text, not a summary. Neither half may be dropped or abbreviated.

Each half is a complete changelog on its own - same headings, same bullets,
same tables. A one-line "see the English above" is not a translation.

## Checklist for a release

- [ ] `Cargo.toml` `version` matches the tag.
- [ ] `notes.md` starts with the English section and its language note.
- [ ] `notes.md` ends with the Russian section and its language note.
- [ ] Both halves describe the same set of changes; the tables match.
- [ ] The `build`, `fmt`, `release`, `clippy` and `test` jobs are green on the
      tagged commit before the tag is pushed.
