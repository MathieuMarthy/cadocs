# ADR 0007: Full-text search with FTS5 and stemming in Rust

Status: accepted

## Context

Search must work for multilingual content, French first. FTS5's built-in `porter` stemmer is English only. `unicode61` folds case and diacritics, but does not stem. Custom FTS5 tokenizers are C callbacks registered through `fts5_api`, which means `unsafe` code, an awkward fit with sqlx, and registration on each connection.

## Decision

- An FTS5 table with `tokenize = "unicode61 remove_diacritics 2"`.
- **Stem in Rust, not in the tokenizer.** At index time the job extracts the text, normalizes it and stems each word with `rust-stemmers` (Snowball, about 18 languages). The stemmed text is what FTS5 indexes. The same pipeline runs on query terms.
- Original text is stored separately, so excerpts are built from real words and not from stems.
- The stemming language is set per workspace, with an option per space later. Language detection is not attempted.
- A prefix index (`prefix='2 3'`) serves search-as-you-type.
- If substring search proves necessary, a second table with the `trigram` tokenizer can be added. It is left out for now because it multiplies index size.

## Consequences

- No `unsafe` and no FFI, and the logic is ordinary Rust that can be unit tested.
- `snippet()` and `highlight()` cannot be used directly on stemmed text, so excerpts are computed in Rust.
- Changing a language requires a reindex, which is a job.
- A page written in two languages is stemmed with one of them. Exact-word matching still works for most words.
- The PostgreSQL version in V2 uses `tsvector` and its own language configurations, behind the same trait.

## Rejected alternatives

- **Custom FTS5 tokenizer through FFI.** Cleaner in SQL, but unsafe code per connection for the same result.
- **tantivy.** Excellent, but a second index with its own memory footprint and binary cost.
- **Meilisearch or Elasticsearch.** External services, out of the question.
- **`LIKE` queries.** No ranking, and a full scan.
