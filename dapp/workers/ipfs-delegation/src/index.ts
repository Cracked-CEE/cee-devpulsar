/**
 * Cloudflare Worker for delegated IPFS uploads.
 *
 * The dapp sends the CAR payload plus the already-signed transaction that will
 * later be submitted on-chain. The worker verifies the transaction signature,
 * uploads the CAR to Filebase, then optionally pins the resulting CID on
 * Pinata in the background.
 */

import { Keypair, Networks, Transaction } from "@stellar/stellar-sdk";
import { CarReader } from "@ipld/car";
import {
  authorizeUpload,
  loadRateLimitConfig,
  type RateLimitEnv,
} from "./rateLimit";

export interface Env extends RateLimitEnv {
  FILEBASE_TOKEN: string;
  PINATA_JWT?: string;
  PINATA_GROUP_ID?: string;
  ENABLE_PINATA_PINNING?: string;
}

interface WorkerExecutionContext {
  waitUntil(promise: Promise<unknown>): void;
}

interface UploadRequest {
  cid: string;
  signedTxXdr: string;
  car: string;
}

const ALLOWED_ORIGINS = [
  "http://localhost:4321",
  "https://testnet.tansu.dev",
  "https://app.tansu.dev",
  "https://tansu.xlm.sh",
  "https://deploy-preview-*--staging-tansu.netlify.app",
];
const FILEBASE_MAX_ATTEMPTS = 3;
const PINATA_MAX_ATTEMPTS = 3;

function isPinataEnabled(env: Env): boolean {
  return env.ENABLE_PINATA_PINNING === "true";
}

function getCorsHeaders(origin: string | null): Record<string, string> {
  if (!origin) return {};

  const isAllowed = ALLOWED_ORIGINS.some(
    (allowed) =>
      allowed === origin ||
      (allowed.includes("*") &&
        new RegExp(`^${allowed.replace(/\*/g, ".*")}$`).test(origin)),
  );

  if (!isAllowed) return {};

  return {
    "Access-Control-Allow-Origin": origin,
    "Access-Control-Allow-Methods": "POST, OPTIONS",
    "Access-Control-Allow-Headers": "Content-Type",
    "Access-Control-Max-Age": "86400",
  };
}

function decodeBase64(base64: string): Uint8Array {
  return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
}

export function buildUploadBlob(base64Car: string): Blob {
  const bytes = new Uint8Array(decodeBase64(base64Car));
  return new Blob([bytes], {
    type: "application/vnd.ipld.car",
  });
}

function validateUploadRequest(body: UploadRequest): void {
  const { cid, signedTxXdr, car } = body;

  if (!cid || !signedTxXdr || !car) {
    throw new Error("Missing required fields: cid, signedTxXdr and car");
  }
}

export function validateSignedTransaction(signedTxXdr: string): string {
  const passphrases = [Networks.TESTNET, Networks.PUBLIC];
  let verifiedTransaction: Transaction | null = null;

  for (const passphrase of passphrases) {
    try {
      const tx = new Transaction(signedTxXdr, passphrase);
      if (!tx.signatures?.length || !tx.source) {
        continue;
      }

      const sourceKeypair = Keypair.fromPublicKey(tx.source);
      const txHash = tx.hash();

      for (const signature of tx.signatures) {
        if (sourceKeypair.verify(txHash, signature.signature())) {
          verifiedTransaction = tx;
          break;
        }
      }

      if (verifiedTransaction) {
        break;
      }
    } catch {
      continue;
    }
  }

  if (!verifiedTransaction) {
    throw new Error("Transaction signature is invalid for the source account");
  }

  if (!verifiedTransaction.operations?.length) {
    throw new Error("Transaction must have at least one operation");
  }

  return verifiedTransaction.source;
}

export async function calculateCidFromCar(carBlob: Blob): Promise<string> {
  const reader = await CarReader.fromBytes(
    new Uint8Array(await carBlob.arrayBuffer()),
  );
  const roots = await reader.getRoots();
  if (!roots[0]) {
    throw new Error("CAR file has no declared root");
  }
  return roots[0].toString();
}

async function sleep(ms: number): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, ms));
}

async function withExponentialBackoff<T>(
  operation: () => Promise<T>,
  maxAttempts: number,
): Promise<T> {
  let lastError: unknown;

  for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
    try {
      return await operation();
    } catch (error) {
      lastError = error;
      if (attempt < maxAttempts - 1) {
        await sleep(500 * 2 ** attempt);
      }
    }
  }

  throw lastError;
}

export default {
  async fetch(
    request: Request,
    env: Env,
    ctx: WorkerExecutionContext,
  ): Promise<Response> {
    const origin = request.headers.get("Origin");
    const corsHeaders = getCorsHeaders(origin);

    // Preflight
    if (request.method === "OPTIONS") {
      return new Response(null, {
        status: 204,
        headers: corsHeaders,
      });
    }

    if (request.method !== "POST") {
      return new Response(
        JSON.stringify({ success: false, error: "Method not allowed" }),
        {
          status: 405,
          headers: {
            ...corsHeaders,
            "Content-Type": "application/json",
          },
        },
      );
    }

    const rateLimitConfig = loadRateLimitConfig(env);

    // Reject oversized requests from the Content-Length header before
    // buffering/parsing the body. Base64 + JSON framing inflate the raw
    // CAR size by roughly 4/3, so allow generous headroom on top of that.
    const declaredLength = Number(request.headers.get("Content-Length"));
    if (
      Number.isFinite(declaredLength) &&
      declaredLength > rateLimitConfig.maxCarBytes * 2
    ) {
      return new Response(
        JSON.stringify({
          success: false,
          error: `CAR payload exceeds maximum allowed size of ${rateLimitConfig.maxCarBytes} bytes`,
        }),
        {
          status: 413,
          headers: {
            ...corsHeaders,
            "Content-Type": "application/json",
          },
        },
      );
    }

    let body: UploadRequest;
    let sourceAccount: string;
    try {
      body = (await request.json()) as UploadRequest;
      validateUploadRequest(body);

      // Cheap upper-bound check on the declared payload before spending CPU
      // on signature verification or base64 decoding.
      const estimatedBytes = Math.floor((body.car.length * 3) / 4);
      if (estimatedBytes > rateLimitConfig.maxCarBytes) {
        return new Response(
          JSON.stringify({
            success: false,
            error: `CAR payload exceeds maximum allowed size of ${rateLimitConfig.maxCarBytes} bytes`,
          }),
          {
            status: 413,
            headers: {
              ...corsHeaders,
              "Content-Type": "application/json",
            },
          },
        );
      }

      sourceAccount = validateSignedTransaction(body.signedTxXdr);
    } catch (error: any) {
      return new Response(
        JSON.stringify({
          success: false,
          error: error?.message ?? "Invalid upload request",
        }),
        {
          status: 400,
          headers: {
            ...corsHeaders,
            "Content-Type": "application/json",
          },
        },
      );
    }

    const carBlob = buildUploadBlob(body.car);
    if (carBlob.size === 0) {
      return new Response(
        JSON.stringify({ success: false, error: "Invalid CAR body" }),
        {
          status: 400,
          headers: {
            ...corsHeaders,
            "Content-Type": "application/json",
          },
        },
      );
    }

    if (carBlob.size > rateLimitConfig.maxCarBytes) {
      return new Response(
        JSON.stringify({
          success: false,
          error: `CAR payload exceeds maximum allowed size of ${rateLimitConfig.maxCarBytes} bytes`,
        }),
        {
          status: 413,
          headers: {
            ...corsHeaders,
            "Content-Type": "application/json",
          },
        },
      );
    }

    const callerIp = request.headers.get("CF-Connecting-IP") ?? "unknown";
    const rateLimitResult = await authorizeUpload(
      env.RATE_LIMIT_KV,
      { ip: callerIp, accountId: sourceAccount },
      carBlob.size,
      rateLimitConfig,
    );

    if (!rateLimitResult.allowed) {
      const status = rateLimitResult.reason === "storage_unavailable" ? 503 : 429;
      const error =
        rateLimitResult.reason === "storage_unavailable"
          ? "Upload service temporarily unavailable"
          : "Rate limit exceeded. Try again later.";

      return new Response(JSON.stringify({ success: false, error }), {
        status,
        headers: {
          ...corsHeaders,
          "Content-Type": "application/json",
          "Retry-After": String(rateLimitResult.retryAfterSeconds),
        },
      });
    }

    let calculatedCid: string;
    try {
      calculatedCid = await calculateCidFromCar(carBlob);
    } catch (error: any) {
      return new Response(
        JSON.stringify({
          success: false,
          error: error?.message ?? "Failed to calculate CID from CAR",
        }),
        {
          status: 400,
          headers: {
            ...corsHeaders,
            "Content-Type": "application/json",
          },
        },
      );
    }

    if (calculatedCid !== body.cid) {
      return new Response(
        JSON.stringify({
          success: false,
          error: `CID mismatch: expected ${body.cid}, got ${calculatedCid}`,
        }),
        {
          status: 400,
          headers: {
            ...corsHeaders,
            "Content-Type": "application/json",
          },
        },
      );
    }

    async function uploadToFilebase(): Promise<void> {
      await withExponentialBackoff(async () => {
        const formData = new FormData();
        formData.append("file", carBlob, `${body.cid}.car`);

        const res = await fetch("https://rpc.filebase.io/api/v0/dag/import", {
          method: "POST",
          headers: {
            Authorization: `Bearer ${env.FILEBASE_TOKEN}`,
          },
          body: formData,
        });

        if (!res.ok) {
          throw new Error(`Filebase HTTP ${res.status}`);
        }

        const text = await res.text();
        try {
          const json = JSON.parse(text);
          const returnedCid =
            json?.Cid?.["/"] ?? json?.Root?.Cid?.["/"] ?? null;
          if (returnedCid && returnedCid !== body.cid) {
            throw new Error("Filebase CID mismatch");
          }
        } catch {
          if (text && !text.includes(body.cid)) {
            throw new Error("Filebase response does not confirm expected CID");
          }
        }
      }, FILEBASE_MAX_ATTEMPTS);
    }

    async function pinCidOnPinata(): Promise<void> {
      if (!env.PINATA_JWT) {
        throw new Error("Pinata JWT not configured");
      }

      await withExponentialBackoff(async () => {
        const payload: Record<string, unknown> = {
          cid: body.cid,
          name: `${body.cid}.car`,
        };

        if (env.PINATA_GROUP_ID) {
          payload.group_id = env.PINATA_GROUP_ID;
        }

        const res = await fetch(
          "https://api.pinata.cloud/v3/files/public/pin_by_cid",
          {
            method: "POST",
            headers: {
              Authorization: `Bearer ${env.PINATA_JWT}`,
              "Content-Type": "application/json",
            },
            body: JSON.stringify(payload),
          },
        );

        if (!res.ok) {
          throw new Error(`Pinata HTTP ${res.status}`);
        }

        const data: any = await res.json();
        const cid = data?.data?.cid;
        if (cid && cid !== body.cid) {
          throw new Error("Pinata CID mismatch");
        }
      }, PINATA_MAX_ATTEMPTS);
    }

    try {
      await uploadToFilebase();
    } catch (error: any) {
      return new Response(
        JSON.stringify({
          success: false,
          error: error?.message ?? "Filebase upload failed",
          cid: body.cid,
        }),
        {
          status: 502,
          headers: {
            ...corsHeaders,
            "Content-Type": "application/json",
          },
        },
      );
    }

    if (isPinataEnabled(env)) {
      ctx.waitUntil(
        pinCidOnPinata().catch((error) => {
          console.error("Pinata pin by CID failed:", error);
        }),
      );
    }

    return new Response(
      JSON.stringify({
        success: true,
        cid: body.cid,
      }),
      {
        status: 200,
        headers: {
          ...corsHeaders,
          "Content-Type": "application/json",
        },
      },
    );
  },
};
