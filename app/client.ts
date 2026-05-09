// =====================================================================
// app/client.ts
// ---------------------------------------------------------------------
// Reference TypeScript SDK for the Reactive Chess program. Imports the
// generated IDL from `target/types/anchor_project.ts` so it stays in
// lockstep with the on-chain program — there is no hand-written schema
// drift to chase.
//
// This file is consumed by:
//   * Frontends (React, Svelte, Solid) that need to author txs.
//   * AI commentators that build a context window from the on-chain
//     `Game` account + the live event stream.
//   * Twitch / Discord / YouTube bots that send moves on behalf of a
//     player or post recap embeds when a game ends.
//
// Run end-to-end:
//   ts-node app/client.ts
// =====================================================================

import * as anchor from "@coral-xyz/anchor";
import { AnchorProvider, BN, Program } from "@coral-xyz/anchor";
import { AnchorProject } from "../target/types/anchor_project";
import {
  Connection,
  Keypair,
  LAMPORTS_PER_SOL,
  PublicKey,
  SystemProgram,
} from "@solana/web3.js";

// ---------------------------------------------------------------------
// PDA helpers
// ---------------------------------------------------------------------
export function gamePda(programId: PublicKey, creator: PublicKey, sessionId: BN) {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("game"), creator.toBuffer(), sessionId.toArrayLike(Buffer, "le", 8)],
    programId
  )[0];
}
export function streamPda(programId: PublicKey, game: PublicKey) {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("stream"), game.toBuffer()],
    programId
  )[0];
}
export function vaultPda(programId: PublicKey, game: PublicKey) {
  return PublicKey.findProgramAddressSync(
    [Buffer.from("vault"), game.toBuffer()],
    programId
  )[0];
}

// ---------------------------------------------------------------------
// Square / coordinate helpers — algebraic <-> 0..63 mailbox index.
// ---------------------------------------------------------------------
export function algebraicToSq(s: string): number {
  // "e2" -> 12 (file=4, rank=1)
  const file = s.charCodeAt(0) - "a".charCodeAt(0);
  const rank = parseInt(s[1], 10) - 1;
  return rank * 8 + file;
}
export function sqToAlgebraic(sq: number): string {
  return String.fromCharCode("a".charCodeAt(0) + (sq & 7)) + ((sq >> 3) + 1).toString();
}

// ---------------------------------------------------------------------
// Thin SDK wrapper. Hides the accounts/PDA plumbing from app code.
// ---------------------------------------------------------------------
export class ChessClient {
  constructor(public readonly program: Program<AnchorProject>) {}

  static fromProvider(provider: AnchorProvider): ChessClient {
    const idl = require("../target/idl/anchor_project.json");
    const program = new Program(idl, provider) as unknown as Program<AnchorProject>;
    return new ChessClient(program);
  }

  async createGame(opts: {
    creator: Keypair;
    sessionId: BN;
    initialMs: number;
    incrementMs: number;
    wagerLamports: number;
    streamMeta?: string;
    streamLabel?: string;
    streamer?: PublicKey | null;
  }) {
    const game = gamePda(this.program.programId, opts.creator.publicKey, opts.sessionId);
    const stream = streamPda(this.program.programId, game);
    const vault = vaultPda(this.program.programId, game);
    await this.program.methods
      .createGame({
        sessionId: opts.sessionId,
        timeControl: {
          initialMs: new BN(opts.initialMs),
          incrementMs: new BN(opts.incrementMs),
          delayMs: new BN(0),
        },
        wagerPerSide: new BN(opts.wagerLamports),
        streamMeta: opts.streamMeta ?? "",
        streamLabel: opts.streamLabel ?? "",
        streamer: opts.streamer ?? null,
      })
      .accountsPartial({
        creator: opts.creator.publicKey,
        game, stream, vault,
        systemProgram: SystemProgram.programId,
      })
      .signers([opts.creator])
      .rpc();
    return { game, stream, vault };
  }

  async join(black: Keypair, game: PublicKey) {
    const vault = vaultPda(this.program.programId, game);
    await this.program.methods
      .joinGame()
      .accountsPartial({
        black: black.publicKey, game, vault,
        systemProgram: SystemProgram.programId,
      })
      .signers([black])
      .rpc();
  }

  async makeMove(opts: {
    mover: Keypair;
    game: PublicKey;
    from: number | string;
    to: number | string;
    promotion?: number;
    expectedNonce: number;
    comment?: string | null;
  }) {
    const g = await this.program.account.game.fetch(opts.game);
    const vault = vaultPda(this.program.programId, opts.game);
    const from = typeof opts.from === "string" ? algebraicToSq(opts.from) : opts.from;
    const to = typeof opts.to === "string" ? algebraicToSq(opts.to) : opts.to;
    await this.program.methods
      .makeMove({
        mv: { from, to, promotion: opts.promotion ?? 0 },
        expectedNonce: opts.expectedNonce,
        comment: opts.comment ?? null,
      })
      .accountsPartial({
        mover: opts.mover.publicKey, game: opts.game, vault,
        white: g.white, black: g.black,
      })
      .signers([opts.mover])
      .rpc();
  }

  async react(opts: {
    spectator: Keypair;
    game: PublicKey;
    kind: 0 | 1 | 2 | 3;
    payload: Uint8Array; // exactly 32 bytes
    registerAsSpectator?: boolean;
  }) {
    const stream = streamPda(this.program.programId, opts.game);
    if (opts.payload.length !== 32) throw new Error("payload must be exactly 32 bytes");
    await this.program.methods
      .recordReaction({
        kind: opts.kind,
        payload: Array.from(opts.payload),
        registerAsSpectator: opts.registerAsSpectator ?? false,
      })
      .accountsPartial({
        spectator: opts.spectator.publicKey,
        game: opts.game,
        stream,
      })
      .signers([opts.spectator])
      .rpc();
  }

  /// Decode the move-history ring buffer into algebraic move strings —
  /// useful for replays and PGN export.
  async decodeMoveHistory(game: PublicKey): Promise<Array<{ from: string; to: string; promotion: number }>> {
    const g = await this.program.account.game.fetch(game);
    return (g.moveHistory as number[]).map((packed) => ({
      from: sqToAlgebraic(packed & 0x3f),
      to: sqToAlgebraic((packed >> 6) & 0x3f),
      promotion: (packed >> 12) & 0x0f,
    }));
  }
}

// ---------------------------------------------------------------------
// Tiny demo program: spin up two keypairs, play 1.e4 e5, react.
// `npm run client` (or `ts-node app/client.ts`) executes this against
// whatever cluster is in ANCHOR_PROVIDER_URL.
// ---------------------------------------------------------------------
async function main() {
  const provider = AnchorProvider.env();
  anchor.setProvider(provider);
  const client = ChessClient.fromProvider(provider);

  const white = Keypair.generate();
  const black = Keypair.generate();
  const fan = Keypair.generate();

  await Promise.all([
    fund(provider.connection, white.publicKey, 5),
    fund(provider.connection, black.publicKey, 5),
    fund(provider.connection, fan.publicKey, 1),
  ]);

  const sessionId = new BN(Date.now());
  const { game } = await client.createGame({
    creator: white,
    sessionId,
    initialMs: 60_000,
    incrementMs: 1_000,
    wagerLamports: 0,
    streamMeta: "demo",
    streamLabel: "demo",
  });
  console.log("game:", game.toBase58());
  await client.join(black, game);

  await client.makeMove({ mover: white, game, from: "e2", to: "e4", expectedNonce: 0 });
  await client.makeMove({ mover: black, game, from: "e7", to: "e5", expectedNonce: 1 });

  const payload = new Uint8Array(32);
  payload.set(Buffer.from("FIRE"));
  await client.react({ spectator: fan, game, kind: 0, payload, registerAsSpectator: true });

  console.log("history:", await client.decodeMoveHistory(game));
}

async function fund(connection: Connection, pk: PublicKey, sol: number) {
  const sig = await connection.requestAirdrop(pk, sol * LAMPORTS_PER_SOL);
  await connection.confirmTransaction(sig, "confirmed");
}

if (require.main === module) {
  main().catch((e) => {
    console.error(e);
    process.exit(1);
  });
}
