# IPFS Delegation Worker

Cloudflare Worker that verifies a signed transaction and then uploads the CAR
file to Filebase. Once Filebase succeeds, it pins the resulting CID on Pinata
in the background when Pinata pinning is enabled.

## API

```json
POST /
{
  "cid": "<expected-root-cid>",
  "signedTxXdr": "<signed transaction xdr>",
  "car": "<base64-car-bytes>"
}
```

The worker verifies the upload request by:

- it verifies the Stellar transaction signature from `signedTxXdr`
- it checks the transaction has at least one operation
- it recalculates the root CID from the uploaded CAR and checks it matches
- it uploads to Filebase with exponential backoff retries
- it can pin that CID on Pinata asynchronously with exponential backoff retries

## Returns JSON

```json
{
  "success": true,
  "cid": "<cid>"
}
```

- If Filebase upload fails, `success` is `false` and the HTTP status is `502`.
- Pinata pinning is disabled by default.
- If enabled, Pinata pinning does not block the response. It runs after
  Filebase succeeds.
- Filebase retries 3 times with exponential backoff.
- When enabled, Pinata pin-by-CID retries 3 times with exponential backoff in
  the background.
- If the CAR payload exceeds `MAX_CAR_BYTES`, `success` is `false` and the
  HTTP status is `413`.
- If the caller (by IP or by signing account) is over budget, `success` is
  `false`, the HTTP status is `429`, and a `Retry-After` header (seconds) is
  set.
- If the rate-limit store (Workers KV) can't be read, the worker fails
  closed: `success` is `false` and the HTTP status is `503`. It never falls
  back to unlimited access.

## Rate Limiting

Every upload is billed to this project's Filebase/Pinata accounts, so the
worker enforces budgets per caller **before** doing any upload work:

- A hard per-request size cap (`MAX_CAR_BYTES`), checked twice: once cheaply
  from the base64 payload length before signature verification, and once
  exactly against the decoded CAR bytes. The client-declared size is never
  trusted on its own.
- A rolling-window request-count and cumulative-byte budget, tracked
  independently for the caller's IP (`CF-Connecting-IP`) and for the
  Stellar account that signed the transaction. A request must fit under
  *both* budgets on *both* dimensions or it's rejected with `429`.
- Counters live in the `RATE_LIMIT_KV` Workers KV namespace (see
  `src/rateLimit.ts`). Any failure to read/write that namespace denies the
  request (`503`) rather than allowing it through.

All limits are plain `[vars]` in `wrangler.toml` (not secrets) so they can be
tuned per environment without a redeploy of secrets:

| Var                                | Default | Meaning                              |
| ----------------------------------- | ------- | ------------------------------------- |
| `MAX_CAR_BYTES`                     | 25 MiB  | Max CAR size per request              |
| `RATE_LIMIT_WINDOW_SECONDS`         | 3600    | Rolling window length                 |
| `IP_RATE_LIMIT_MAX_REQUESTS`        | 20      | Max requests per IP per window        |
| `IP_RATE_LIMIT_MAX_BYTES`           | 200 MiB | Max cumulative bytes per IP per window|
| `ACCOUNT_RATE_LIMIT_MAX_REQUESTS`   | 10      | Max requests per account per window   |
| `ACCOUNT_RATE_LIMIT_MAX_BYTES`      | 100 MiB | Max cumulative bytes per account/window|

### Provisioning the KV namespace

Local `wrangler dev` simulates `RATE_LIMIT_KV` automatically — no setup
needed. Before deploying to testnet/production, create a real namespace per
environment and paste the printed id into `wrangler.toml`:

```bash
bunx wrangler kv namespace create RATE_LIMIT_KV --env testnet
bunx wrangler kv namespace create RATE_LIMIT_KV --env production
```

### Load testing

```bash
cd dapp/workers/ipfs-delegation
bun run test:load
```

Fires uploads back-to-back against a running worker (defaults to
`http://localhost:8787`) until one is rejected, and asserts the rejection is
a `429` with a `Retry-After` header.

## Development

Add your provider tokens to `.dev.vars`:

```bash
FILEBASE_TOKEN=<filebase_api_token>
ENABLE_PINATA_PINNING=false
PINATA_JWT=<optional_pinata_jwt>
PINATA_GROUP_ID=<optional_pinata_group_id>
```

### Start the Worker

```bash
cd dapp/workers/ipfs-delegation
bun install
bun run dev
```

### Test the Worker

In another terminal:

```bash
cd dapp/workers/ipfs-delegation
bun run test
```

Or against deployed environments (see next section):

```bash
ENV=DEV bun run test  # Use testnet environment
ENV=PROD bun run test # Use production environment
```

The test script generates a CAR, signs a local Stellar test transaction, and
submits the same JSON payload the dapp sends. A successful local test confirms
the blocking Filebase upload path. When Pinata pinning is enabled, that step
runs asynchronously after the response is returned.

## Deployment

### Prerequisites

```bash
bunx wrangler login
```

### Security

All secrets are stored in Cloudflare Secrets. Set them with wrangler:

```bash
# Development
bunx wrangler secret put FILEBASE_TOKEN --env testnet
bunx wrangler secret put ENABLE_PINATA_PINNING --env testnet
bunx wrangler secret put PINATA_JWT --env testnet
bunx wrangler secret put PINATA_GROUP_ID --env testnet

# Production
bunx wrangler secret put FILEBASE_TOKEN --env production
bunx wrangler secret put ENABLE_PINATA_PINNING --env production
bunx wrangler secret put PINATA_JWT --env production
bunx wrangler secret put PINATA_GROUP_ID --env production
```

### Development (Testnet)

```bash
bunx wrangler deploy --env testnet
```

Deploys to `https://ipfs-testnet.tansu.dev`

### Production (Mainnet)

```bash
bunx wrangler deploy --env production
```

Deploys to `https://ipfs.tansu.dev`
