// =====================================================================
// app/subscribe.ts
// ---------------------------------------------------------------------
// Reference websocket subscriber for the Reactive Chess protocol.
//
// Three subscription patterns are demonstrated, in increasing levels
// of infrastructure / decreasing latency tradeoff:
//
//   1. PUBLIC RPC `connection.onLogs` + Anchor `BorshCoder.events`.
//      Works on any Solana RPC. Latency: ~confirm time of the cluster.
//      Throughput: bounded by the RPC's onLogs fanout.
//
//   2. Helius enhanced websocket (`accountSubscribe` for Game accounts +
//      `transactionSubscribe` filtered to programIds). Sub-second
//      delivery, structured payloads, no parsing needed.
//
//   3. Geyser plug-in -> Kafka -> downstream consumers. Pattern shown
//      below for completeness; deploy a node with the geyser plug-in
//      configured to forward `account_update` and `transaction` events
//      filtered by program id.
//
// Pattern (1) is the runnable demo; (2) and (3) are scaffolded with
// the connection details factored out so production deployments swap
// just the URL.
// =====================================================================

import * as anchor from "@coral-xyz/anchor";
import { AnchorProvider, BorshCoder, EventParser, Program } from "@coral-xyz/anchor";
import { AnchorProject } from "../target/types/anchor_project";
import { Connection, PublicKey } from "@solana/web3.js";

const PROGRAM_ID = new PublicKey("3mCUQGNrzN42qngYE9k6qcF6GqZHMRYJ4MA9TTYcBCdu");

// =====================================================================
// Pattern 1: vanilla `onLogs` -> EventParser
// =====================================================================
export async function subscribePublicRpc(rpcUrl: string) {
  const connection = new Connection(rpcUrl, { commitment: "confirmed" });
  const idl = require("../target/idl/anchor_project.json");
  const provider = new AnchorProvider(connection, {} as any, { commitment: "confirmed" });
  const program = new Program(idl, provider) as unknown as Program<AnchorProject>;
  const coder = new BorshCoder(program.idl);
  const parser = new EventParser(PROGRAM_ID, coder);

  console.log(`[onLogs] subscribing to ${PROGRAM_ID.toBase58()} on ${rpcUrl}`);
  const id = connection.onLogs(
    PROGRAM_ID,
    (logs) => {
      if (logs.err) return;
      for (const ev of parser.parseLogs(logs.logs)) {
        switch (ev.name) {
          case "movePlayed":
            console.log("♟ move:", ev.data);
            break;
          case "checkmateEvent":
            console.log("♚ MATE:", ev.data);
            break;
          case "drawEvent":
            console.log("½ draw:", ev.data);
            break;
          case "streamReactionEvent":
            console.log("✨ reaction:", ev.data);
            break;
          default:
            console.log(`(${ev.name})`, ev.data);
        }
      }
    },
    "confirmed"
  );
  return () => connection.removeOnLogsListener(id);
}

// =====================================================================
// Pattern 2: Helius enhanced websocket (transactionSubscribe)
// ---------------------------------------------------------------------
// Free tier OK for ~1 game; production deployments should use the
// Atlas / Enhanced product with `programIds` filtering.
//
// Set HELIUS_RPC_URL to e.g. `wss://atlas-mainnet.helius-rpc.com/?api-key=…`
// =====================================================================
export async function subscribeHelius(wsUrl: string) {
  const ws = new (require("ws"))(wsUrl);

  const subscribeMsg = {
    jsonrpc: "2.0",
    id: 1,
    method: "transactionSubscribe",
    params: [
      {
        accountInclude: [PROGRAM_ID.toBase58()],
        failed: false,
      },
      {
        commitment: "confirmed",
        encoding: "jsonParsed",
        transactionDetails: "full",
        showRewards: false,
        maxSupportedTransactionVersion: 0,
      },
    ],
  };

  ws.on("open", () => {
    console.log("[helius] open, subscribing…");
    ws.send(JSON.stringify(subscribeMsg));
  });
  ws.on("message", (raw: Buffer) => {
    const msg = JSON.parse(raw.toString());
    if (msg.method !== "transactionNotification") return;
    const logs: string[] = msg.params?.result?.transaction?.meta?.logMessages ?? [];
    // Reuse the same EventParser pipeline as Pattern 1; left as an
    // exercise to wire up here. The point is: Helius gives you a
    // structured tx payload you can route directly to overlays.
    console.log("[helius]", logs.filter((l) => l.includes("Program data")).length, "events in tx");
  });
  ws.on("error", (e: Error) => console.error("[helius]", e));
  return () => ws.close();
}

// =====================================================================
// Pattern 3: Geyser plug-in (production scale)
// ---------------------------------------------------------------------
// Deploy: https://github.com/rpcpool/yellowstone-grpc plus the
// `solana-geyser-plugin` interface. Filter by:
//
//   accounts: { owner: [PROGRAM_ID] }
//   transactions: { account_include: [PROGRAM_ID] }
//
// Bridge the gRPC stream into Kafka / Redpanda; downstream consumers
// can run their own Anchor IDL parser to materialise events into
// per-game streams. This file just documents the shape.
// =====================================================================

if (require.main === module) {
  const rpc = process.env.ANCHOR_PROVIDER_URL ?? "http://127.0.0.1:8899";
  subscribePublicRpc(rpc).then((unsub) => {
    process.on("SIGINT", () => {
      unsub();
      process.exit(0);
    });
  });
}
