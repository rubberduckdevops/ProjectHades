# Hades API Contract

Shared by `backend/` (Rust/Axum on Lambda), `frontend/` (React + Cloudscape) and `infra/` (CDK).

## Routing
- All API routes live under `/api`. The Axum router is nested at `/api`.
  - In production, CloudFront forwards `/api/*` unchanged to the API Gateway HTTP API, which forwards to the Lambda.
  - Locally, the server listens on `http://localhost:3000`, so the base URL is `http://localhost:3000/api`.
- `GET /api/health` is unauthenticated and returns `200 "ok"`.

## Auth
- Every `/api/journal*` route requires `Authorization: Bearer <Cognito ACCESS token>`.
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
- The frontend sends the user's *local* midnight-to-midnight range converted to UTC ISO, e.g. `new Date(y,m,d).toISOString()`.
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
  "contentHash": "<64 hex>", "size": 1234, "createdAt": "2026-10-19T14:03:22.123Z", "updatedAt": "2026-10-19T15:00:00.000Z" }
// Note = NoteMeta + "content"
```
`project` is always present, possibly `null`, and `tags` is always an array.

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

## Errors
All errors are JSON: `{ "error": "message" }` with status 400, 401, 404, 409 or 500. A `409` from the notes routes also includes `currentHash` when the note exists.

## Backend env vars
| Var | Required | Default |
|---|---|---|
| `TABLE_NAME` | yes | |
| `NOTES_BUCKET` | yes | |
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
