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

## Entry shape
```json
{ "id": "2026-10-19T14:03:22.123Z", "timestamp": "2026-10-19T14:03:22.123Z", "body": "text" }
```
`id` and `timestamp` both equal the sort key.

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
Body: `{ "body": "text", "id"?: "<RFC3339 timestamp>" }`
- With no `id`, it inserts a new entry with `sk = now` and returns `201 entry`.
- With an `id`, it must parse as RFC 3339 (otherwise `400`). The id is normalized to `sk` format and the backend calls `PutItem` at that key.
  - Returns `200 entry` if an existing item was replaced.
  - Returns `201 entry` if the item is new.
- `body` must be non-empty after trimming and at most 10 KB. Otherwise `400`.
- `pk` always comes from the token. Clients can never pick the user.

## Errors
All errors are JSON: `{ "error": "message" }` with status 400, 401, 404 or 500.

## Backend env vars
| Var | Required | Default |
|---|---|---|
| `TABLE_NAME` | yes | |
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
