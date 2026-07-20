import { describe, it, expect, beforeEach } from "vitest";
import { authorizeUpload, loadRateLimitConfig } from "./rateLimit";

class FakeKVNamespace {
  private store = new Map<string, string>();
  public failGet = false;
  public failPut = false;

  async get(key: string): Promise<string | null> {
    if (this.failGet) {
      throw new Error("KV get failed");
    }
    return this.store.get(key) ?? null;
  }

  async put(key: string, value: string): Promise<void> {
    if (this.failPut) {
      throw new Error("KV put failed");
    }
    this.store.set(key, value);
  }
}

function config(overrides: Partial<ReturnType<typeof loadRateLimitConfig>> = {}) {
  return {
    ...loadRateLimitConfig({
      RATE_LIMIT_WINDOW_SECONDS: "3600",
      MAX_CAR_BYTES: "1000",
      IP_RATE_LIMIT_MAX_REQUESTS: "3",
      IP_RATE_LIMIT_MAX_BYTES: "500",
      ACCOUNT_RATE_LIMIT_MAX_REQUESTS: "2",
      ACCOUNT_RATE_LIMIT_MAX_BYTES: "500",
    } as any),
    ...overrides,
  };
}

describe("loadRateLimitConfig", () => {
  it("falls back to defaults for missing/invalid values", () => {
    const cfg = loadRateLimitConfig({});
    expect(cfg.windowSeconds).toBeGreaterThan(0);
    expect(cfg.maxCarBytes).toBeGreaterThan(0);
    expect(cfg.ip.maxRequests).toBeGreaterThan(0);
  });

  it("ignores non-positive overrides", () => {
    const cfg = loadRateLimitConfig({
      MAX_CAR_BYTES: "-5",
      IP_RATE_LIMIT_MAX_REQUESTS: "not-a-number",
    } as any);
    expect(cfg.maxCarBytes).toBeGreaterThan(0);
    expect(cfg.ip.maxRequests).toBeGreaterThan(0);
  });
});

describe("authorizeUpload", () => {
  let kv: FakeKVNamespace;

  beforeEach(() => {
    kv = new FakeKVNamespace();
  });

  it("allows requests within both budgets", async () => {
    const result = await authorizeUpload(
      kv as unknown as KVNamespace,
      { ip: "1.2.3.4", accountId: "GABC" },
      100,
      config(),
    );
    expect(result.allowed).toBe(true);
  });

  it("denies once the per-account request count is exceeded", async () => {
    const identity = { ip: "1.2.3.4", accountId: "GABC" };
    await authorizeUpload(kv as unknown as KVNamespace, identity, 10, config());
    await authorizeUpload(kv as unknown as KVNamespace, identity, 10, config());
    const third = await authorizeUpload(
      kv as unknown as KVNamespace,
      identity,
      10,
      config(),
    );

    expect(third.allowed).toBe(false);
    if (!third.allowed) {
      expect(third.reason).toBe("rate_limited");
      expect(third.retryAfterSeconds).toBeGreaterThan(0);
    }
  });

  it("denies once the per-IP byte budget is exceeded, even from a fresh account", async () => {
    const cfg = config();
    await authorizeUpload(
      kv as unknown as KVNamespace,
      { ip: "9.9.9.9", accountId: "GACCOUNT1" },
      450,
      cfg,
    );
    const second = await authorizeUpload(
      kv as unknown as KVNamespace,
      { ip: "9.9.9.9", accountId: "GACCOUNT2" },
      100,
      cfg,
    );

    expect(second.allowed).toBe(false);
    if (!second.allowed) {
      expect(second.reason).toBe("rate_limited");
    }
  });

  it("does not let one identity dimension's budget leak into another's", async () => {
    const cfg = config();
    await authorizeUpload(
      kv as unknown as KVNamespace,
      { ip: "1.1.1.1", accountId: "GX" },
      10,
      cfg,
    );
    const otherIp = await authorizeUpload(
      kv as unknown as KVNamespace,
      { ip: "2.2.2.2", accountId: "GY" },
      10,
      cfg,
    );
    expect(otherIp.allowed).toBe(true);
  });

  it("fails closed when KV.get throws", async () => {
    kv.failGet = true;
    const result = await authorizeUpload(
      kv as unknown as KVNamespace,
      { ip: "1.2.3.4", accountId: "GABC" },
      10,
      config(),
    );
    expect(result.allowed).toBe(false);
    if (!result.allowed) {
      expect(result.reason).toBe("storage_unavailable");
    }
  });

  it("fails closed when KV.put throws", async () => {
    kv.failPut = true;
    const result = await authorizeUpload(
      kv as unknown as KVNamespace,
      { ip: "1.2.3.4", accountId: "GABC" },
      10,
      config(),
    );
    expect(result.allowed).toBe(false);
    if (!result.allowed) {
      expect(result.reason).toBe("storage_unavailable");
    }
  });
});
