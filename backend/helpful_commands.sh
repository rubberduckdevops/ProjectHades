#!/usr/bin/env bash
# Handy commands for running and poking the backend locally. Not meant to be run top-to-bottom;
# copy/paste the bits you need. Requires: aws CLI, curl, jq.

# --- Environment (see ../API_CONTRACT.md) -------------------------------------------------
export AWS_REGION=us-west-2
export TABLE_NAME=hades_journal
export NOTES_BUCKET=xxxxxxxx-notesbucket-xxxxxxxx   # from the CDK stack outputs (versioned bucket)
export COGNITO_USER_POOL_ID=us-west-2_XXXXXXXXX      # from the CDK stack outputs
export COGNITO_CLIENT_ID=xxxxxxxxxxxxxxxxxxxxxxxxxx  # from the CDK stack outputs
export VECTOR_BUCKET=hades-vectors-xxxxxxxx          # from the CDK stack outputs (S3 Vectors bucket)
export VECTOR_INDEX=notes
# export EMBED_MODEL_ID=amazon.titan-embed-text-v2:0  # default
export CORS_ORIGINS=http://localhost:5173
export PORT=3000
export RUST_LOG=info,tower_http=debug
# Optional: point at DynamoDB Local instead of AWS (the SDK reads this natively)
# export AWS_ENDPOINT_URL_DYNAMODB=http://localhost:8000
# Notes need a *versioned* bucket: writes fail with 500 if S3 returns no VersionId.

# --- Run ----------------------------------------------------------------------------------
cargo run --bin backend

# --- Get a Cognito ACCESS token (the app client must allow USER_PASSWORD_AUTH) -----------
export HADES_USER=me@example.com
read -rsp "Password: " HADES_PASSWORD; echo
export TOKEN=$(aws cognito-idp initiate-auth \
  --region "$AWS_REGION" \
  --auth-flow USER_PASSWORD_AUTH \
  --client-id "$COGNITO_CLIENT_ID" \
  --auth-parameters USERNAME="$HADES_USER",PASSWORD="$HADES_PASSWORD" \
  --query AuthenticationResult.AccessToken --output text)

API=http://localhost:3000/api
AUTH="Authorization: Bearer $TOKEN"

# --- Requests -----------------------------------------------------------------------------
# Health (no auth)
curl -s "$API/health"; echo

# List today's entries (current UTC day by default)
curl -s -H "$AUTH" "$API/journal" | jq

# List an explicit range (inclusive bounds, RFC 3339)
curl -s -H "$AUTH" "$API/journal?from=2026-10-19T00:00:00.000Z&to=2026-10-19T23:59:59.999Z" | jq

# Create a new entry (no id) -> 201
curl -s -X PUT "$API/journal" -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"body":"Hello!"}' -w '\n%{http_code}\n'

# Get one entry by timestamp (= id) -> 200 or 404
TS=2026-10-19T14:03:22.123Z
curl -s -H "$AUTH" "$API/journal/$TS" | jq

# Upsert by id -> 200 if it replaced an existing entry, 201 if new
curl -s -X PUT "$API/journal" -H "$AUTH" -H 'Content-Type: application/json' \
  -d "{\"id\":\"$TS\",\"body\":\"Edited text\"}" -w '\n%{http_code}\n'

# No token -> 401
curl -s "$API/journal" -w '\n%{http_code}\n'

# --- Notes (see "Notes (phase 1)" in ../API_CONTRACT.md) ------------------------------------
# Paths go in the URL as normal segments, each percent-encoded on its own: "My Ideas" -> My%20Ideas

# List all notes (sorted by path) / only a folder
curl -s -H "$AUTH" "$API/notes" | jq
curl -s -H "$AUTH" "$API/notes?prefix=Projects/" | jq

# Create (no baseHash) -> 201 NoteMeta, or 409 {error, currentHash} if it exists
curl -s -X PUT "$API/notes/Projects/My%20Ideas/Design.md" -H "$AUTH" -H 'Content-Type: application/json' \
  -d '{"content":"---\ntags: [arch]\nproject: hades\n---\n# Design\n\nHello"}' -w '\n%{http_code}\n'

# Read -> 200 Note (metadata + content) or 404
curl -s -H "$AUTH" "$API/notes/Projects/My%20Ideas/Design.md" | jq

# Update with the last-seen hash -> 200, 409 {error, currentHash} if changed, 404 if missing
HASH=$(curl -s -H "$AUTH" "$API/notes/Projects/My%20Ideas/Design.md" | jq -r .contentHash)
curl -s -X PUT "$API/notes/Projects/My%20Ideas/Design.md" -H "$AUTH" -H 'Content-Type: application/json' \
  -d "{\"content\":\"# Design\\n\\nEdited\",\"baseHash\":\"$HASH\"}" -w '\n%{http_code}\n'

# Upload a local Markdown file as a new note (jq builds the JSON safely)
jq -Rs '{content: .}' ./Some.md | curl -s -X PUT "$API/notes/Inbox/Some.md" -H "$AUTH" \
  -H 'Content-Type: application/json' --data-binary @- -w '\n%{http_code}\n'

# Move / rename -> 200 NoteMeta at the new path; 409 "target exists" if `to` exists
HASH=$(curl -s -H "$AUTH" "$API/notes/Projects/My%20Ideas/Design.md" | jq -r .contentHash)
curl -s -X POST "$API/notes/move" -H "$AUTH" -H 'Content-Type: application/json' \
  -d "{\"from\":\"Projects/My Ideas/Design.md\",\"to\":\"Projects/Hades/Design.md\",\"baseHash\":\"$HASH\"}" | jq

# Delete (baseHash required) -> 204, 409 on mismatch, 404 if missing
HASH=$(curl -s -H "$AUTH" "$API/notes/Projects/Hades/Design.md" | jq -r .contentHash)
curl -s -X DELETE "$API/notes/Projects/Hades/Design.md?baseHash=$HASH" -H "$AUTH" -w '%{http_code}\n'

# Inspect storage directly
aws s3api list-object-versions --bucket "$NOTES_BUCKET" --prefix "users/" --max-items 20
aws dynamodb query --table-name "$TABLE_NAME" --key-condition-expression 'pk = :pk' \
  --expression-attribute-values '{":pk":{"S":"NOTE#<sub>"}}'

# --- Search (see "Notes search (phase 2)" in ../API_CONTRACT.md) ---------------------------
# Name matches (title/path) first, then semantic matches. `project` and `limit` (1-50) optional.
curl -s -G -H "$AUTH" "$API/search" --data-urlencode "q=where do notes live" | jq
curl -s -G -H "$AUTH" "$API/search" --data-urlencode "q=storage" -d project=hades -d limit=5 | jq
# Is a note indexed yet? (`indexed` flips to true once the indexer has caught up)
curl -s -H "$AUTH" "$API/notes" | jq -r '.[] | "\(.indexed)\t\(.path)"'

# --- Indexer ------------------------------------------------------------------------------
# Backfill: reindex every note whose indexedHash != contentHash. Returns counts.
INDEXER_FN=xxxxxxxx-IndexerFunction-xxxxxxxx   # from the CDK stack outputs / Lambda console
aws lambda invoke --region "$AWS_REGION" --function-name "$INDEXER_FN" \
  --cli-binary-format raw-in-base64-out --payload '{"backfill": true}' /dev/stdout | jq

# Tail the indexer's logs
aws logs tail "/aws/lambda/$INDEXER_FN" --region "$AWS_REGION" --follow

# Inspect vectors for a note (keys are <sub>#<path>#<n>)
aws s3vectors list-vectors --vector-bucket-name "$VECTOR_BUCKET" --index-name "$VECTOR_INDEX" \
  --return-metadata --max-results 10 | jq '.vectors[] | {key, metadata}'

# Run the indexer locally against real AWS (reads the same env vars minus Cognito)
cargo lambda watch --bin indexer   # then, in another shell:
cargo lambda invoke indexer --data-ascii '{"backfill": true}'

# --- Tests ----------------------------------------------------------------------------------
cargo test
# End-to-end against moto (S3 + DynamoDB; Bedrock/S3 Vectors are faked in-process)
uvx --from 'moto[server]' moto_server -p 5055 &
HADES_E2E_ENDPOINT=http://127.0.0.1:5055 cargo test e2e -- --ignored --test-threads=1
