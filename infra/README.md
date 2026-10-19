# Hades infrastructure (AWS CDK, TypeScript)

The `HadesStack` deploys these resources (default region `us-west-2`, or set `CDK_DEFAULT_REGION`):

- **DynamoDB** table `hades_journal` with `pk`/`sk` strings, on-demand billing and PITR. The table is retained when the stack is deleted.
- **Cognito** user pool with email sign-in and no self sign-up. It has a public web client that allows SRP and USER_PASSWORD auth.
- **Lambda**, built from `../backend` (Rust, arm64, `provided.al2023`) with `cargo-lambda`.
- **API Gateway HTTP API**, with a `$default` route to the Lambda. Auth is enforced in the app.
- **S3 + CloudFront** for the SPA:
  - `/*` is served from S3 through OAC. A CloudFront Function rewrites extension-less paths to `/index.html`.
  - `/api/*` is proxied to the HTTP API with no caching.
- `config.json` is written into the site bucket at deploy time: `{ apiUrl: "/api", region, userPoolId, userPoolClientId }`.

## Prerequisites

- Node 20+ and npm. The CDK CLI is a dev dependency, so use `npx cdk`.
- Rust toolchain (`rustup`).
- **cargo-lambda** and **zig**. cargo-lambda uses zig to cross-compile. Install with one of:
  - `pip install cargo-lambda`, which also installs the `ziglang` package. With uv, use `uv tool install cargo-lambda`.
  - `cargo install --locked cargo-lambda`, then install zig separately (for example `pip install ziglang`, or your package manager).

  Check with `cargo lambda --version` and `zig version`. If zig only came from pip, cargo-lambda may not find it. In that case, put the zig binary on `PATH`, e.g. `ln -s <venv>/lib/python3.x/site-packages/ziglang/zig ~/.local/bin/zig`.
- AWS credentials for the target account (`aws configure` / `AWS_PROFILE`).

## First-time setup

```sh
cd infra
npm install
npx cdk bootstrap        # once per account/region
```

## Deploy

Build the frontend first. If `frontend/dist` is missing, the site deployment is skipped and synth prints a warning.

```sh
cd frontend && npm install && npm run build
cd ../infra && npx cdk deploy
```

`npx cdk synth` builds the Rust Lambda with cargo-lambda. The first build takes several minutes.

The stack outputs are `CloudFrontUrl`, `HttpApiUrl`, `TableName`, `UserPoolId`, `UserPoolClientId` and `Region`. To fetch them later:

```sh
aws cloudformation describe-stacks --stack-name HadesStack \
  --query 'Stacks[0].Outputs' --output table
```

## Create a user

Self sign-up is disabled, so an admin has to create users:

```sh
POOL_ID=<UserPoolId output>
EMAIL=you@example.com

aws cognito-idp admin-create-user \
  --user-pool-id "$POOL_ID" \
  --username "$EMAIL" \
  --user-attributes Name=email,Value="$EMAIL" Name=email_verified,Value=true \
  --message-action SUPPRESS

aws cognito-idp admin-set-user-password \
  --user-pool-id "$POOL_ID" \
  --username "$EMAIL" \
  --password 'A-strong-Passw0rd!' \
  --permanent
```

## Get an access token (for curl testing)

```sh
CLIENT_ID=<UserPoolClientId output>

TOKEN=$(aws cognito-idp initiate-auth \
  --auth-flow USER_PASSWORD_AUTH \
  --client-id "$CLIENT_ID" \
  --auth-parameters USERNAME="$EMAIL",PASSWORD='A-strong-Passw0rd!' \
  --query 'AuthenticationResult.AccessToken' --output text)

curl -H "Authorization: Bearer $TOKEN" https://<CloudFrontUrl>/api/journal
```

The backend needs the **access** token, not the ID token.

## Local development

Local development runs against the deployed DynamoDB table and Cognito pool, using your local AWS credentials.

Backend (listens on `http://localhost:3000`, routes under `/api`):

```sh
export AWS_REGION=us-west-2
export TABLE_NAME=hades_journal
export COGNITO_USER_POOL_ID=<UserPoolId output>
export COGNITO_CLIENT_ID=<UserPoolClientId output>
export CORS_ORIGINS=http://localhost:5173   # default
cd backend && cargo run
```

Frontend:

```sh
cd frontend
cp public/config.example.json public/config.json
# edit public/config.json: set region, userPoolId, userPoolClientId from the outputs;
# keep apiUrl = "http://localhost:3000/api"
npm run dev
```

## Notes

- The table and user pool use `RemovalPolicy.RETAIN`, so `cdk destroy` leaves them in place. The site bucket is destroyed and emptied.
- Never commit `frontend/public/config.json`. In production, the deployed `config.json` from CDK overrides any file with the same name in `dist`.
