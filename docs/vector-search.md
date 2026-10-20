# How Hades notes search works

An explainer for the semantic search shipped in Notes phase 2. It covers what a vector search is, how Hades builds and queries one, and why it's set up this way, including where it isn't the best choice. For exact API details see [`API_CONTRACT.md`](../API_CONTRACT.md), "Notes search (phase 2)". For the original design see [`notes-design.md`](notes-design.md).

## 1. The idea in one paragraph

Keyword search finds notes that contain the words you typed. Semantic search finds notes that *mean* what you typed, even if they use different words. To do that, Hades turns text into an **embedding**: a list of 1,024 numbers produced by an AI model (Amazon Titan Text Embeddings V2). The model is trained so that texts with similar meaning get similar lists of numbers. "Cache the signing keys" and "avoid refetching the JWKS" end up close together, even though they share no words. Searching means turning your query into the same kind of list and finding the stored lists closest to it.

```
"avoid refetching the JWKS"  ──Titan──►  [0.012, -0.044, 0.091, … 1,024 numbers]
                                                  │  "which stored vectors point in
                                                  ▼   nearly the same direction?"
  chunk "## Caching\nCache the signing keys…"   [0.010, -0.051, 0.087, …]  ← close: a match
  chunk "## Groceries\nEggs, milk…"             [-0.073, 0.022, -0.015, …] ← far: not a match
```

"Closeness" is **cosine similarity**: the angle between two vectors, ignoring their length. Titan is asked to return *normalized* vectors (length 1), so cosine similarity is just a dot product, which is cheap. Hades reports it as a score where 1.0 means "same direction" and around 0 means "unrelated".

## 2. The two halves: indexing and searching

```
WRITE PATH (background)                                READ PATH (when you search)
───────────────────────                                ───────────────────────────
You save a note                                        You type "avoid refetching JWKS"
  │                                                      │
  ▼                                                      ▼
API writes content to S3 (versioned)                   API: embed the query with Titan (1 call)
then metadata to DynamoDB                                │
  │                                                      ▼
  ▼  DynamoDB stream (only after the write commits)    S3 Vectors: top 30 nearest chunks,
Indexer Lambda                                           filtered to YOUR user id
  1. read the exact S3 version                           │
  2. split into chunks by heading                        ▼
  3. embed each chunk with Titan                       Group chunks by note, keep each note's
  4. store vectors in S3 Vectors                         best chunk (its snippet + heading)
  5. mark the note "indexed"                             │
                                                         ▼
                                                       Merge with title/path matches → results
```

### Indexing: what happens after you save
1. **The trigger is the DynamoDB stream, not S3 events.** A note's file is written to S3 *before* its metadata, so an S3 event could fire while the metadata doesn't exist yet. The indexer would then skip the note forever. Stream records fire only after the metadata write commits, and they carry the exact S3 version and content hash to index. So every saved note gets indexed exactly as saved.
2. **Chunking.** Embedding a whole long note as one vector blurs it: a note about ten topics becomes a vector that's vaguely about all ten. Hades instead splits each note into chunks:
   - Split on Markdown headings, ignoring `#` lines inside code blocks. Each chunk remembers its heading path, e.g. `Design > Storage`. That's what the search results show.
   - Merge tiny sections (under 200 characters) into the next one, so a lone heading doesn't become a meaningless vector.
   - Split big sections (over 2,800 characters, roughly 700 tokens) at blank lines. Consecutive pieces overlap by 300 characters, so an idea sitting on a boundary still appears whole in at least one piece.
   - What gets embedded is `title + heading + chunk text`, not the bare text. A short chunk like "Use the cached copy." then still carries its context ("Design > Auth").
3. **Embedding.** Each chunk goes to Titan, 8 at a time, giving 1,024 numbers per chunk.
4. **Storing.** Each vector goes into S3 Vectors under the key `<your user id>#<note path>#<chunk number>`, with metadata:
   - **Filterable:** `sub` (your user id), `path`, `project`. Queries can filter on these.
   - **Stored only:** `heading`, `snippet` (the first ~300 characters), `title`, `updatedAt`. These are returned with results so the UI can show them without fetching the note.
5. **Cleanup and the "indexed" flag.**
   - If the note now has fewer chunks than last time, the leftover vectors are deleted.
   - The indexer then records `indexedHash = contentHash` on the note, but **only if the note hasn't changed since indexing started** (a conditional write). If you edited it mid-index, the write is skipped, and the newer edit's stream record reindexes it. This is what drives the "Indexing…" indicator in the UI.
   - Deleting or moving a note removes its vectors.
   - A one-off **backfill** indexes notes that existed before search was deployed.

### Searching: what happens when you type
1. The query (up to 500 characters) is embedded with the same model and settings. Using the same model is essential, because vectors from different models aren't comparable.
2. S3 Vectors returns the **30 nearest chunks**, filtered to `sub = <your user id>`, plus your project if you picked one. The user filter is built on the server from your login token in exactly one function. It never comes from the request, so you can't see anyone else's notes even by crafting a request. A second check drops any result belonging to another user.
3. The 30 chunks are **grouped by note**. A note matching in three places appears once, scored by its best chunk, and shows that chunk's heading and snippet.
4. **Name matches are merged in first.** Notes whose title or path contains your text (ignoring case) rank above semantic matches. Semantic search can be oddly bad at exact names: searching "Design.md" should simply find `Design.md`.
5. Results are trimmed to the requested limit (the UI asks for 20) and returned with `match: "name"` or `"semantic"`, a score, the heading and the snippet.

## 3. Why it's built this way

**"Optimized" here means optimized for this situation:** one person (or a few), up to a few thousand notes, near-zero cost when idle, and nothing to patch or babysit. It does *not* mean the fastest or highest-quality search possible. Section 4 covers what you'd change if those goals change.

| Decision | Why | What it costs you |
|---|---|---|
| **S3 Vectors** as the vector store | Pay per use with **no idle cost**: storage plus queries for a personal vault is cents per month. Serverless, with no cluster to size or patch, and in CloudFormation, so it's in the CDK stack. | Queries are slower than an in-memory vector database: sub-second, rather than single-digit milliseconds. You won't notice that typing into a search box; a high-traffic app would. |
| **Titan Text Embeddings V2** via Bedrock | Runs in your account and region, with nothing to host and no third-party API key. About **$0.02 per million tokens**, so indexing an entire vault costs a few cents. 1,024 dimensions is its highest-fidelity setting, and normalized vectors make cosine distance a cheap dot product. | Not the top-ranked embedding model on public benchmarks. Others may retrieve slightly better, but cost more, live outside AWS, or both. |
| **Chunk by heading** with merge, split and overlap | Your notes are Markdown, and headings are the author's own topic boundaries. Each vector is about one thing, and results can say *where* in the note the match is. | Notes with no headings fall back to size-based splits, which can be less tidy. |
| **DynamoDB stream** trigger with conditional write-back | Correct by construction: every committed save is indexed from the exact version saved, retries are safe (already-indexed records are skipped), and a mid-index edit is never falsely marked done. | A few seconds of delay between saving and being searchable, shown as "Indexing…". |
| **Name matches first**, then semantic | Covers semantic search's blind spot for exact titles and paths, using metadata Hades already has. No second search system needed. | Name matching is substring-only: no typo tolerance or stemming. |
| **One shared index** with a server-built user filter | Simple infrastructure: one index created by CDK, not one per user at sign-up. Isolation is enforced in one tested function. | Isolation relies on that code being right, rather than on separate indexes. It's tested, including a check that a crafted project value can't inject a filter operator. |
| **Top 30 chunks**, grouped to notes | 30 chunks comfortably yields 10–20 distinct notes even when one note matches in several places. | A note whose best chunk ranks 31st won't appear. That's fine at this scale. |

### What it costs to run (rough, for a few-thousand-note vault)
- **Initial indexing:** about 1–2 million tokens, or about $0.02–$0.04 of Titan.
- **Each search:** one tiny Titan call (a fraction of a cent per thousand searches) and one S3 Vectors query.
- **Storage:** 4 KB per chunk vector plus metadata, so even 10,000 chunks is around 40–50 MB. That's cents per month.
- **Idle:** nothing runs and nothing bills by the hour.

**For comparison:** Amazon OpenSearch Serverless, the classic AWS choice for vector search, has a minimum charge of roughly **$175/month** even with zero queries. That alone was the deciding factor.

## 4. Limits, and what you'd change if needs grow

These are deliberate trade-offs, not bugs. Each has a known upgrade path.

- **No keyword relevance ranking.** Name matching only looks at titles and paths. A search for an exact rare term that appears only inside a note body depends on semantic search catching it, which embeddings usually do but not always. *Upgrade:* add full-text keyword scoring (BM25) over bodies and blend it with the semantic score ("hybrid search").
- **No reranking.** Results are ordered by vector similarity alone. *Upgrade:* pass the top ~30 to a reranker model (Bedrock has some), which reads each query–chunk pair properly. Better ordering, at more cost and latency per search.
- **Changing the embedding model means reindexing everything.** Vectors from different models aren't comparable, and the index's dimension is fixed. *How:* point `EMBED_MODEL_ID` at the new model, create a new index (the name `notes` is fixed, so rename it or delete it by hand), then run the backfill.
- **Latency.** Sub-second is fine for a search box. *Upgrade if it ever matters:* a dedicated vector database (OpenSearch, pgvector on Aurora, or a hosted service) is faster, but you'd pay for always-on capacity.
- **Journal entries aren't searchable.** This was deferred on purpose. The same pipeline could index them.

## 5. Where it lives in the code

| Piece | File |
|---|---|
| Chunking (headings, merge, split, overlap, snippets) | `backend/src/chunker.rs` |
| Titan embedding call | `backend/src/embed.rs` |
| S3 Vectors put, delete, query and the vector key/metadata | `backend/src/vectors.rs` |
| Indexer reconcile, write-back, backfill | `backend/src/indexer.rs`, entry point `backend/src/bin/indexer.rs` |
| Search filter, grouping, name merge | `backend/src/search.rs`, endpoint `backend/src/handler/search.rs` |
| Infrastructure (stream, vector bucket and index, indexer Lambda, IAM) | `infra/lib/hades-stack.ts` |
| Search box and indexing indicator | `frontend/src/components/NotesSearch.tsx`, `frontend/src/pages/NoteEditorPage.tsx` |
| Tuning constants | top-level `const`s in `chunker.rs` (200 / 2,800 / 300 / 300), `search.rs` (top-K 30, limit 10–50), `embed.rs` (1,024 dims), `indexer.rs` (8 concurrent embeds) |
