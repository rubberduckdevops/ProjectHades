# Hades API Contract

Shared by `backend/` (Rust/Axum on Lambda), `frontend/` (React + Cloudscape) and `infra/` (CDK).

## Routing
- All API routes live under `/api`. The Axum router is nested at `/api`.
  - In production, CloudFront forwards `/api/*` unchanged to the API Gateway HTTP API, which forwards to the Lambda.
  - Locally, the server listens on `http://localhost:3000`, so the base URL is `http://localhost:3000/api`.
- `GET /api/health` is unauthenticated and returns `200 "ok"`.

## Auth
- Every route except `/api/health` (journal, notes, search, todos and projects) requires `Authorization: Bearer <Cognito ACCESS token>`.
- The backend validates the token against the user pool's JWKS. It checks for RS256, `iss = https://cognito-idp.{region}.amazonaws.com/{userPoolId}`, `exp`, `token_use == "access"` and `client_id == COGNITO_CLIENT_ID`.
- The userId is the token's `sub`.
- A missing or invalid token gets `401 {"error": "..."}`.

## DynamoDB
- Table name comes from env `TABLE_NAME`. CDK names it `hades_journal`.
- `pk` (S) = `JOURNAL#<sub>`
- `sk` (S) = ISO-8601 UTC timestamp with millis, e.g. `2026-10-19T14:03:22.123Z`.
- `body` (S) holds the entry text.
- `project` (S, optional) holds the free-text project. It's absent for general entries, including every entry saved before projects existed.

## Entry shape
```json
{ "id": "2026-10-19T14:03:22.123Z", "timestamp": "2026-10-19T14:03:22.123Z", "project": "hades", "body": "text" }
```
`id` and `timestamp` both equal the sort key. `project` is always present, and is `null` for a general entry.

## Endpoints
### `GET /api/journal?from=<ISO>&to=<ISO>`
- Returns `200 [entry, ...]`, sorted by timestamp ascending.
- `from` and `to` are optional RFC 3339 strings. They default to the start and end of the current UTC day.
- Both bounds are inclusive. The backend normalizes them to the `sk` format before querying.
- The frontend sends the user's *local* midnight-to-midnight range converted to UTC ISO, e.g. `new Date(y,m,d).toISOString()`. It shows one day at a time, today by default, and `/journal?date=YYYY-MM-DD` opens a past day.
- An invalid bound gets `400`.

### `GET /api/journal/{timestamp}`
- Returns `200 entry`, or `404 {"error":"not found"}`.
- An unparseable timestamp gets `400`.

### `PUT /api/journal`: upsert
Body: `{ "body": "text", "project"?: "text", "id"?: "<RFC3339 timestamp>" }`
- With no `id`, it inserts a new entry with `sk = now` and returns `201 entry`.
- With an `id`, it must parse as RFC 3339 (otherwise `400`). The id is normalized to `sk` format and the backend calls `PutItem` at that key.
  - Returns `200 entry` if an existing item was replaced.
  - Returns `201 entry` if the item is new.
- `body` must be non-empty after trimming and at most 10 KB. Otherwise `400`.
- `project` is optional free text. It's trimmed, and a blank or missing value means a general entry (`null`). It can be at most 100 bytes. Otherwise `400`.
- An upsert by `id` replaces the whole entry, so leaving out `project` clears it.
- `pk` always comes from the token. Clients can never pick the user.

## Notes (phase 1)
Design: [`docs/notes-design.md`](docs/notes-design.md). Phase 1 covers create, read, update, delete and move. Search and indexing are not included yet.

### Storage
- **Content** lives in S3 bucket `NOTES_BUCKET` (versioned) at key `users/<sub>/notes/<path>`, with `Content-Type: text/markdown; charset=utf-8`.
- **Metadata** lives in the existing table:
  - `pk = NOTE#<sub>`, `sk = <path>`
  - `title` (S), `project` (S, optional), `tags` (L of S, optional)
  - `contentHash` (S, lowercase hex sha256 of the UTF-8 content), `s3VersionId` (S)
  - `size` (N, bytes), `createdAt` (S), `updatedAt` (S). Timestamps use the journal's `sk` format: ISO-8601 UTC with millis.

### Paths
A path is the note's ID: a vault-style relative path like `Projects/Hades/Design.md`. A valid path:
- ends in `.md` and is 1–512 bytes;
- has segments separated by `/`, none of them empty, `.` or `..`;
- has no leading or trailing `/`, no backslash, and no control characters (U+0000–U+001F, U+007F);
- has no segment starting or ending with whitespace.

An invalid path gets `400`. Paths are case-sensitive.

**In URLs**, the path goes into the URL as normal path segments, after `/api/notes/`. Each segment is percent-encoded on its own (`encodeURIComponent` per segment, joined with `/`). Example: `/api/notes/Projects/My%20Ideas/Design.md`. The backend decodes it and validates it with the rules above.

### Metadata derivation (server-side, on every write)
Optional YAML frontmatter can sit at the top of the content, between `---` lines. From it:
- **`title`** is the frontmatter `title`, else the first `# ` H1 line in the body, else the filename without `.md`.
- **`project`** is the frontmatter `project` (string), trimmed. Blank means `null`.
- **`tags`** is the frontmatter `tags`, as a list of strings or a single comma-separated string. Values are trimmed, blanks dropped and a leading `#` stripped, giving `[]` if none.

Frontmatter that fails to parse is ignored (treated as body), never an error.

### Shapes
```json
// NoteMeta
{ "path": "Projects/Hades/Design.md", "title": "Design", "project": "hades", "tags": ["arch"],
  "contentHash": "<64 hex>", "size": 1234, "createdAt": "2026-10-19T14:03:22.123Z", "updatedAt": "2026-10-19T15:00:00.000Z",
  "indexed": true }
// Note = NoteMeta + "content"
```
- `project` is always present, possibly `null`, and `tags` is always an array.
- `indexed` (phase 2) is `true` when the search index reflects the current content, meaning the stored `indexedHash` equals `contentHash`. While it's `false`, the note won't appear in semantic search yet, though title and path matches still work.

### Endpoints
#### `GET /api/notes?prefix=<folder/>`
- Returns `200 [NoteMeta]`, sorted by path ascending.
- `prefix` is optional and matches with `begins_with` on the path.

#### `GET /api/notes/{path...}`
- Returns `200 Note`, or `404`.

#### `PUT /api/notes/{path...}`
Body: `{ "content": "...", "baseHash"?: "<64 hex>" }`. `content` can be at most 1 MB (1,048,576 bytes); otherwise `400`. Empty content is allowed.
- **`baseHash` absent** means create-only:
  - `201 NoteMeta` if created.
  - `409 { "error": "note already exists", "currentHash": "<hash>" }` if the note exists.
- **`baseHash` present** means update-only:
  - `200 NoteMeta` if the stored `contentHash` equals `baseHash`.
  - `409 { "error": "note was changed", "currentHash": "<hash>" }` if it doesn't.
  - `404` if the note doesn't exist.
- **If the content is identical** to what's stored (same hash, and `baseHash` matches), it returns `200` without writing a new S3 version.
- **Write order:**
  1. S3 `PutObject`.
  2. Conditional DynamoDB put:
     - creating: `attribute_not_exists(sk)`;
     - updating: `contentHash = :baseHash`.
  3. If the condition fails, the backend deletes the S3 object version it just wrote (`DeleteObject` with `VersionId`) and returns `409`.

#### `DELETE /api/notes/{path...}?baseHash=<hash>`
- `baseHash` is required (`400` if missing).
- Returns `204` on success, `409 { error, currentHash }` on a mismatch, `404` if the note doesn't exist.
- Order: conditional delete of the metadata item first, then S3 `DeleteObject` without a version ID, which leaves a delete marker so history is kept.

#### `POST /api/notes/move`
Body: `{ "from": "<path>", "to": "<path>", "baseHash": "<hash>" }`.
- Returns `200 NoteMeta` (at the new path).
- `404` if `from` is missing, `409 { error, currentHash }` on a hash mismatch, `409 { "error": "target exists" }` if `to` exists, `400` if the paths are invalid or equal.
- Steps:
  1. Copy the S3 object to the new key.
  2. A DynamoDB `TransactWriteItems` that both:
     - creates the `to` metadata (`attribute_not_exists(sk)`);
     - deletes the `from` metadata (conditional on `contentHash = :baseHash`).

     Using a transaction means the metadata never ends up half-moved.
  3. Delete the `from` object, leaving a delete marker.
  4. If the transaction fails, delete the copied S3 object.
- `createdAt` is kept, and `updatedAt` is set to now.

### Routing note
`POST /api/notes/move` is a static route. It never conflicts with a note path, because note paths end in `.md`.

## Notes search (phase 2)
Design: [`docs/notes-design.md`](docs/notes-design.md), sections "Indexing" and "Search".

### Extra metadata attributes
These are written **only by the indexer**:
- `indexedHash` (S): the `contentHash` that was last indexed.
- `chunkCount` (N): the number of vectors stored for the note.

The API's update (`UpdateItem SET …`) must not touch these two, so they survive edits and `indexed` flips to `false` until the indexer catches up. Create and move write fresh items, so neither attribute is present until the note is indexed.

### Indexing pipeline
- **Trigger:** a DynamoDB stream on the table (`NEW_AND_OLD_IMAGES`) feeds the **indexer** Lambda, filtered to `pk` starting with `NOTE#`. It is *not* triggered by S3 events: the S3 object is written before the metadata, so an S3 event could arrive before the metadata exists.
- **Reconcile from the record.** For each record, using `sub = pk minus "NOTE#"` and `path = sk`:
  - **REMOVE:** delete vectors `<sub>#<path>#0 … #<chunkCount-1>`, taking `chunkCount` from the old image, or 0 if it's absent. This also covers the old path after a move.
  - **INSERT / MODIFY:** if `indexedHash == contentHash` in the new image, there's nothing to do. This includes the indexer's own write-back. Otherwise:
    1. Get the S3 object at `users/<sub>/notes/<path>` with `VersionId = s3VersionId`, the exact version.
    2. Chunk it, embed it, and `PutVectors`.
    3. Delete the vectors `#<new count> … #<old chunkCount-1>`.
    4. Write back with `UpdateItem SET indexedHash = :h, chunkCount = :n` under the condition `contentHash = :h`. If the condition fails, the note changed meanwhile, and the newer stream record will reindex it, so this is ignored.
- **Errors:** errors are reported per record (`ReportBatchItemFailures`), the stream retries, and records go to an SQS dead-letter queue after 3 attempts.
- **Backfill:** invoking the indexer directly with `{"backfill": true}` reindexes every `NOTE#` item where `indexedHash != contentHash`. This is for notes created before phase 2.

### Chunking (indexer)
1. Strip the frontmatter.
2. Split on Markdown headings (`#`–`######`, outside code fences). Each chunk's `heading` is its heading path joined with ` > `, e.g. `Design > Storage`. Text before the first heading uses the note title.
3. Merge sections under about 200 characters into the next section.
4. Split sections over about 2,800 characters (roughly 700 tokens) on blank lines, then hard-wrap. Consecutive pieces overlap by about 300 characters.
5. Embed `"<title>\n<heading>\n\n<chunk text>"` so short chunks keep their context.

An empty note gives 0 chunks: no vectors, but `indexedHash` is still written.

### Embeddings
- **Model:** Bedrock `InvokeModel`, model `amazon.titan-embed-text-v2:0`.
- **Request body:** `{"inputText": "...", "dimensions": 1024, "normalize": true}`.
- **Response:** read `embedding` (float32 × 1024).
- **Length cap:** input is capped at 30,000 characters.

### Vectors (S3 Vectors)
- **Location:** vector bucket `VECTOR_BUCKET`, index `VECTOR_INDEX`. The index is `float32`, dimension 1024, `cosine`.
- **Key:** `<sub>#<path>#<n>`.
- **Filterable metadata:** `sub` (string), `path` (string), `project` (string, omitted when `null`).
- **Non-filterable metadata:** `heading` (string), `snippet` (string: the first about 300 characters of the chunk text with whitespace collapsed), `title` (string), `updatedAt` (string).

### `GET /api/search?q=<text>&project=<name>&limit=<n>`
- **Parameters:**
  - `q` is required: 1–500 characters after trimming, otherwise `400`.
  - `project` is optional, an exact match.
  - `limit` is optional: 1–50, default 10, otherwise `400`.
- **Steps:**
  1. Embed `q` with the model and settings above.
  2. Call `QueryVectors(topK = 30, filter, returnMetadata = true, returnDistance = true)`. The filter is `{"sub": {"$eq": <caller sub>}}`, and with `project` it becomes `{"$and": [<sub filter>, {"project": {"$eq": project}}]}`. **The `sub` condition is built server-side in exactly one function and never comes from the request.**
  3. Group the results by `path`. Each note keeps its best chunk, with `score = 1 - distance` for cosine.
  4. Name matches: from the caller's notes list (one metadata `Query`), take every note whose title or path contains `q`, case-insensitively, and that passes the `project` filter. These rank **first**, title matches before path-only matches, each group by `updatedAt` descending.
  5. Merge the two lists, deduplicating by path: a note that's in both appears once, at the name-match position, with its semantic snippet. Drop semantic hits with no metadata (deleted or in flight). Trim to `limit`.
- **Response:** `200` with
```json
[{ "path": "Projects/Hades/Design.md", "title": "Design", "project": "hades",
   "updatedAt": "…", "match": "semantic",
   "score": 0.83, "heading": "Design > Storage", "snippet": "Content lives in S3…" }]
```
  - `match` is `"name"` or `"semantic"`.
  - `score`, `heading` and `snippet` are `null` for a name match with no semantic hit.
  - For a name match, `snippet` falls back to `null`. The client shows the path instead.

## Todos
A small task list. Same auth as every other route; the user always comes from the token.

### Storage
Items live in the existing table:
- `pk = TODO#<sub>`, `sk = <createdAt>` in the journal's `sk` format (ISO-8601 UTC with millis). The sort key is the todo's `id`.
- `title` (S), `done` (BOOL), `updatedAt` (S).
- `project` (S, optional), `due` (S, optional, `YYYY-MM-DD`), `completedAt` (S, optional, set while `done` is true).

### Shape
```json
{ "id": "2026-10-20T14:03:22.123Z", "title": "Renew passport", "project": "home", "due": "2026-11-01",
  "done": false, "createdAt": "2026-10-20T14:03:22.123Z", "completedAt": null, "updatedAt": "2026-10-20T14:03:22.123Z" }
```
`id` equals `createdAt`. `project`, `due` and `completedAt` are always present, possibly `null`.

### Validation
- `title` is trimmed and stored trimmed. It must be non-empty and at most 500 bytes.
- `project` follows the journal's rules: trimmed, blank means `null`, at most 100 bytes.
- `due` is a calendar date, `YYYY-MM-DD`. It has no time or timezone: it's the day you picked.
- Anything else gets `400`.

### Endpoints
#### `GET /api/todos`
Returns `200 [Todo]`: every todo, open and done, sorted by `createdAt` ascending. Ordering for display is up to the client.

#### `POST /api/todos`
Body: `{ "title": "...", "project"?: "...", "due"?: "YYYY-MM-DD" }`. Returns `201 Todo`, with `done: false`.

#### `PATCH /api/todos/{id}`
Body: any of `{ "title": "...", "project": "..." | null, "due": "YYYY-MM-DD" | null, "done": true | false }`.
- Fields that are left out are unchanged. `null` (or a blank project) clears `project` or `due`.
- `done: true` sets `completedAt` to now, unless the todo was already done (the original time is kept). `done: false` clears `completedAt`.
- `updatedAt` is set to now on every patch.
- Returns `200 Todo`, `404` if the todo doesn't exist, `400` if `id` isn't a timestamp or a field is invalid.

#### `DELETE /api/todos/{id}`
Returns `204`, or `404` if the todo doesn't exist.

## Projects
The free-text `project` used by journal entries, todos and notes, collected into one list per user so the same names are suggested everywhere and every day.

### Storage
Items live in the existing table under `pk = PROJECT#<sub>`:
- `sk = NAME#<lowercased name>`: `name` (S, the spelling last used) and `lastUsedAt` (S, the journal's `sk` format). Names are case-insensitive: `Hades` and `hades` are one project.
- `sk = META#backfill`: a marker, written once the registry has been seeded from existing data.

### Recording uses
These calls record their project (set `name`, `lastUsedAt = now`) after their own write succeeds:
- `PUT /api/journal` with a project;
- `POST /api/todos` with a project, and `PATCH /api/todos/{id}` that sets one;
- `PUT /api/notes/{path...}` whose frontmatter has a project of at most 100 bytes.

This is best-effort: if it fails, the error is logged and the request still succeeds.

### Endpoints
#### `GET /api/projects`
Returns `200 [{ "name": "hades", "lastUsedAt": "2026-10-22T09:15:00.000Z" }]`, most recently used first.

The first call for a user (no `META#backfill` marker) seeds the registry. It reads every journal entry, todo and note, and records each project with its latest use: the entry's timestamp, or the todo's or note's `updatedAt`. A later use that's already recorded is kept. Then it writes the marker.

#### `DELETE /api/projects/{name}`
`name` is percent-encoded and matched case-insensitively. Returns `204`, `404` if it isn't registered, or `400` if it's blank or over 100 bytes. Only the registry item goes: entries, todos and notes keep the name, and using it again registers it again. The backfill never runs twice, so it won't bring the name back.

## Errors
All errors are JSON: `{ "error": "message" }` with status 400, 401, 404, 409 or 500. A `409` from the notes routes also includes `currentHash` when the note exists.

## Backend env vars
| Var | Required | Default |
|---|---|---|
| `TABLE_NAME` | yes | |
| `NOTES_BUCKET` | yes | |
| `VECTOR_BUCKET` | yes (API and indexer) | |
| `VECTOR_INDEX` | yes (API and indexer) | |
| `EMBED_MODEL_ID` | no | `amazon.titan-embed-text-v2:0` |
| `COGNITO_USER_POOL_ID` | yes | |
| `COGNITO_CLIENT_ID` | yes | |
| `AWS_REGION` | no | `us-west-2` |
| `CORS_ORIGINS` | no | `http://localhost:5173` (comma-separated) |
| `PORT` | no (local only) | `3000` |

The server runs as a Lambda via `lambda_http` when `AWS_LAMBDA_RUNTIME_API` is set, and as a TCP server otherwise.

## Frontend runtime config
The frontend fetches `/config.json` at startup. Nothing is baked in at build time, and there are no `VITE_*` vars.
```json
{ "apiUrl": "/api", "region": "us-west-2", "userPoolId": "us-west-2_xxx", "userPoolClientId": "xxxx" }
```
- In production, CDK writes this file into the S3 bucket with `apiUrl: "/api"`.
- Locally, `frontend/public/config.json` is gitignored. Copy it from `config.example.json`, which uses `apiUrl: "http://localhost:3000/api"`.
