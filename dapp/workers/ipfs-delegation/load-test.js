#!/usr/bin/env node
import "dotenv/config";
import {
  Account,
  Keypair,
  Networks,
  Operation,
  TransactionBuilder,
} from "@stellar/stellar-sdk";
import { createDirectoryEncoderStream, CAREncoderStream } from "ipfs-car";

const DEV_URL = "https://ipfs-testnet.tansu.dev";
const PROD_URL = "https://ipfs.tansu.dev";
const ENV = process.env.ENV || "LOCAL";

let WORKER_URL =
  process.env.PUBLIC_DELEGATION_API_URL || "http://localhost:8787";
if (ENV === "DEV") {
  WORKER_URL = DEV_URL;
} else if (ENV === "PROD") {
  WORKER_URL = PROD_URL;
}

// Same account across every attempt on purpose: this drives the
// per-account budget over its limit without needing that many distinct
// IPs, which a single test runner doesn't have.
const SIGNER = process.env.TEST_SIGNER_SECRET
  ? Keypair.fromSecret(process.env.TEST_SIGNER_SECRET)
  : Keypair.random();

// One more than the default ACCOUNT_RATE_LIMIT_MAX_REQUESTS (10), so the
// last attempt is expected to be rejected under default config.
const ATTEMPTS = Number(process.env.LOAD_TEST_ATTEMPTS || 11);

function arrayBufferToBase64(buffer) {
  let binary = "";
  const bytes = new Uint8Array(buffer);
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary);
}

async function packFilesToCar(files) {
  const stream = createDirectoryEncoderStream(files);
  let rootCID;
  const blocks = [];

  await stream.pipeTo(
    new WritableStream({
      write(block) {
        blocks.push(block);
        rootCID = block.cid.toString();
      },
    }),
  );

  if (!rootCID) {
    throw new Error("Failed to compute test CID");
  }

  const carEncoder = new CAREncoderStream([blocks[blocks.length - 1].cid]);
  const chunks = [];
  await new ReadableStream({
    pull(controller) {
      if (blocks.length > 0) {
        controller.enqueue(blocks.shift());
      } else {
        controller.close();
      }
    },
  })
    .pipeThrough(carEncoder)
    .pipeTo(
      new WritableStream({
        write(chunk) {
          chunks.push(chunk);
        },
      }),
    );

  return {
    cid: rootCID,
    car: new Blob(chunks, { type: "application/vnd.ipld.car" }),
  };
}

function buildSignedTestTransaction(signer, seq) {
  const account = new Account(signer.publicKey(), String(seq));
  const transaction = new TransactionBuilder(account, {
    fee: "100",
    networkPassphrase: Networks.TESTNET,
  })
    .addOperation(
      Operation.manageData({
        name: "ipfs-load-test",
        value: "ok",
      }),
    )
    .setTimeout(60)
    .build();

  transaction.sign(signer);
  return transaction.toXDR();
}

async function attemptUpload(seq) {
  const testFile = new File(
    [`Load test payload #${seq}`],
    `load-test-${seq}.txt`,
    { type: "text/plain" },
  );
  const { cid, car } = await packFilesToCar([testFile]);
  const signedTxXdr = buildSignedTestTransaction(SIGNER, seq);

  const res = await fetch(WORKER_URL, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      cid,
      signedTxXdr,
      car: arrayBufferToBase64(await car.arrayBuffer()),
    }),
  });

  let data;
  try {
    data = await res.json();
  } catch {
    data = null;
  }

  return { status: res.status, retryAfter: res.headers.get("Retry-After"), data };
}

async function loadTest() {
  console.log(`Connecting to worker at: ${WORKER_URL}`);
  console.log(`Firing ${ATTEMPTS} uploads from the same signing account...\n`);

  let rejected = false;

  for (let i = 1; i <= ATTEMPTS; i += 1) {
    const { status, retryAfter, data } = await attemptUpload(i);
    console.log(`Attempt ${i}: HTTP ${status}${retryAfter ? ` (Retry-After: ${retryAfter}s)` : ""}`);

    if (status === 429) {
      if (!retryAfter) {
        throw new Error("429 response missing Retry-After header");
      }
      console.log(`\nRejected as expected: ${JSON.stringify(data)}`);
      rejected = true;
      break;
    }

    if (status !== 200) {
      throw new Error(
        `Unexpected non-200/429 status ${status} on attempt ${i}: ${JSON.stringify(data)}`,
      );
    }
  }

  if (!rejected) {
    throw new Error(
      `Expected a 429 within ${ATTEMPTS} attempts but every request succeeded. ` +
        "Either the account budget is misconfigured or rate limiting isn't wired up.",
    );
  }

  console.log("\n✅ Load test confirmed budget enforcement (429 + Retry-After).");
}

loadTest().catch((err) => {
  console.error("\n❌ Load test failed:", err);
  process.exit(1);
});
