/**
 * Per-IP and per-signing-account rate limiting for the upload endpoint,
 * backed by Workers KV. Uses a weighted two-bucket (current + previous
 * fixed window) counter, which approximates a sliding window without
 * needing per-request log entries in KV.
 *
 * Both the request-count and cumulative-byte budgets are enforced per
 * identity dimension (IP and account); a request is allowed only if it
 * fits under both budgets on both dimensions. Any KV read/write failure
 * is treated as a denial (fail closed), never as unlimited access.
 */

export interface RateLimitEnv {
  RATE_LIMIT_KV: KVNamespace;
  RATE_LIMIT_WINDOW_SECONDS?: string;
  MAX_CAR_BYTES?: string;
  IP_RATE_LIMIT_MAX_REQUESTS?: string;
  IP_RATE_LIMIT_MAX_BYTES?: string;
  ACCOUNT_RATE_LIMIT_MAX_REQUESTS?: string;
  ACCOUNT_RATE_LIMIT_MAX_BYTES?: string;
}

const DEFAULT_WINDOW_SECONDS = 60 * 60;
const DEFAULT_MAX_CAR_BYTES = 25 * 1024 * 1024;
const DEFAULT_IP_MAX_REQUESTS = 20;
const DEFAULT_IP_MAX_BYTES = 200 * 1024 * 1024;
const DEFAULT_ACCOUNT_MAX_REQUESTS = 10;
const DEFAULT_ACCOUNT_MAX_BYTES = 100 * 1024 * 1024;

export interface RateLimitConfig {
  windowSeconds: number;
  maxCarBytes: number;
  ip: { maxRequests: number; maxBytes: number };
  account: { maxRequests: number; maxBytes: number };
}

function positiveNumber(value: string | undefined, fallback: number): number {
  const parsed = value ? Number(value) : NaN;
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

export function loadRateLimitConfig(env: Partial<RateLimitEnv>): RateLimitConfig {
  return {
    windowSeconds: positiveNumber(
      env.RATE_LIMIT_WINDOW_SECONDS,
      DEFAULT_WINDOW_SECONDS,
    ),
    maxCarBytes: positiveNumber(env.MAX_CAR_BYTES, DEFAULT_MAX_CAR_BYTES),
    ip: {
      maxRequests: positiveNumber(
        env.IP_RATE_LIMIT_MAX_REQUESTS,
        DEFAULT_IP_MAX_REQUESTS,
      ),
      maxBytes: positiveNumber(env.IP_RATE_LIMIT_MAX_BYTES, DEFAULT_IP_MAX_BYTES),
    },
    account: {
      maxRequests: positiveNumber(
        env.ACCOUNT_RATE_LIMIT_MAX_REQUESTS,
        DEFAULT_ACCOUNT_MAX_REQUESTS,
      ),
      maxBytes: positiveNumber(
        env.ACCOUNT_RATE_LIMIT_MAX_BYTES,
        DEFAULT_ACCOUNT_MAX_BYTES,
      ),
    },
  };
}

interface WindowBucket {
  requests: number;
  bytes: number;
}

async function readBucket(
  kv: KVNamespace,
  key: string,
): Promise<WindowBucket> {
  const raw = await kv.get(key);
  if (!raw) {
    return { requests: 0, bytes: 0 };
  }

  const parsed = JSON.parse(raw);
  return {
    requests: typeof parsed.requests === "number" ? parsed.requests : 0,
    bytes: typeof parsed.bytes === "number" ? parsed.bytes : 0,
  };
}

interface BudgetCheck {
  allowed: boolean;
  retryAfterSeconds: number;
}

async function checkAndConsumeBudget(
  kv: KVNamespace,
  identityKey: string,
  requestBytes: number,
  windowSeconds: number,
  maxRequests: number,
  maxBytes: number,
): Promise<BudgetCheck> {
  const now = Math.floor(Date.now() / 1000);
  const currentBucketId = Math.floor(now / windowSeconds);
  const elapsedInWindow = now - currentBucketId * windowSeconds;
  const weight = 1 - elapsedInWindow / windowSeconds;

  const currentKey = `rl:${identityKey}:${currentBucketId}`;
  const previousKey = `rl:${identityKey}:${currentBucketId - 1}`;

  const [current, previous] = await Promise.all([
    readBucket(kv, currentKey),
    readBucket(kv, previousKey),
  ]);

  const estimatedRequests = previous.requests * weight + current.requests;
  const estimatedBytes = previous.bytes * weight + current.bytes;

  if (
    estimatedRequests + 1 > maxRequests ||
    estimatedBytes + requestBytes > maxBytes
  ) {
    return {
      allowed: false,
      retryAfterSeconds: windowSeconds - elapsedInWindow,
    };
  }

  await kv.put(
    currentKey,
    JSON.stringify({
      requests: current.requests + 1,
      bytes: current.bytes + requestBytes,
    } satisfies WindowBucket),
    { expirationTtl: windowSeconds * 2 },
  );

  return { allowed: true, retryAfterSeconds: 0 };
}

export interface RateLimitIdentity {
  ip: string;
  accountId: string;
}

export type RateLimitResult =
  | { allowed: true }
  | {
      allowed: false;
      reason: "rate_limited" | "storage_unavailable";
      retryAfterSeconds: number;
    };

export async function authorizeUpload(
  kv: KVNamespace,
  identity: RateLimitIdentity,
  requestBytes: number,
  config: RateLimitConfig,
): Promise<RateLimitResult> {
  try {
    const [ipResult, accountResult] = await Promise.all([
      checkAndConsumeBudget(
        kv,
        `ip:${identity.ip}`,
        requestBytes,
        config.windowSeconds,
        config.ip.maxRequests,
        config.ip.maxBytes,
      ),
      checkAndConsumeBudget(
        kv,
        `acct:${identity.accountId}`,
        requestBytes,
        config.windowSeconds,
        config.account.maxRequests,
        config.account.maxBytes,
      ),
    ]);

    if (!ipResult.allowed || !accountResult.allowed) {
      return {
        allowed: false,
        reason: "rate_limited",
        retryAfterSeconds: Math.ceil(
          Math.max(ipResult.retryAfterSeconds, accountResult.retryAfterSeconds),
        ),
      };
    }

    return { allowed: true };
  } catch (error) {
    console.error("Rate limit check failed, denying upload:", error);
    return {
      allowed: false,
      reason: "storage_unavailable",
      retryAfterSeconds: 60,
    };
  }
}
